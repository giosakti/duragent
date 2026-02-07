//! Discord gateway subprocess binary.
//!
//! This binary runs the Discord gateway as a subprocess, communicating with
//! the parent Duragent process via JSON Lines over stdio.
//!
//! The subprocess will exit when:
//! - stdin is closed (parent died)
//! - A Shutdown command is received
//! - An unrecoverable error occurs

use std::io::IsTerminal;

use duragent_gateway_discord::{DiscordConfig, DiscordGateway};
use duragent_gateway_protocol::{GatewayCommand, GatewayEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging to stderr (stdout is reserved for protocol)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("duragent_gateway_discord=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Check if running as subprocess (stdin is not a terminal)
    if std::io::stdin().is_terminal() {
        eprintln!("Error: duragent-discord is designed to run as a subprocess of duragent.");
        eprintln!("It communicates via stdin/stdout and should not be run directly.");
        eprintln!();
        eprintln!("To use Discord gateway, configure it in your duragent.yaml:");
        eprintln!();
        eprintln!("  gateways:");
        eprintln!("    external:");
        eprintln!("      - name: discord");
        eprintln!("        command: duragent-discord");
        eprintln!("        env:");
        eprintln!("          DISCORD_BOT_TOKEN: ${{DISCORD_BOT_TOKEN}}");
        std::process::exit(1);
    }

    // Get bot token from environment
    let bot_token = std::env::var("DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DISCORD_BOT_TOKEN environment variable not set"))?;

    info!("Starting Discord gateway subprocess");

    // Create channels for communication
    let (evt_tx, mut evt_rx) = mpsc::channel::<GatewayEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(100);

    // Create and start the Discord gateway
    let config = DiscordConfig::new(bot_token);
    let gateway = DiscordGateway::new(config);

    // Spawn the gateway task
    tokio::spawn(async move {
        gateway.start(evt_tx, cmd_rx).await;
    });

    // Spawn stdin reader task
    let cmd_tx_clone = cmd_tx.clone();
    let stdin_handle = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        while let Ok(Some(line)) = reader.next_line().await {
            match serde_json::from_str::<GatewayCommand>(&line) {
                Ok(command) => {
                    let is_shutdown = matches!(command, GatewayCommand::Shutdown);
                    if cmd_tx_clone.send(command).await.is_err() {
                        debug!("Command channel closed");
                        break;
                    }
                    if is_shutdown {
                        break;
                    }
                }
                Err(e) => {
                    warn!(line = %line, error = %e, "Failed to parse command from stdin");
                }
            }
        }

        // stdin closed = parent died, trigger shutdown
        debug!("Stdin closed, shutting down");
        let _ = cmd_tx_clone.send(GatewayCommand::Shutdown).await;
    });

    // Main loop: forward events to stdout
    let mut stdout = tokio::io::stdout();
    while let Some(event) = evt_rx.recv().await {
        let is_shutdown = matches!(event, GatewayEvent::Shutdown { .. });

        match serde_json::to_string(&event) {
            Ok(json) => {
                let line = format!("{}\n", json);
                if let Err(e) = stdout.write_all(line.as_bytes()).await {
                    error!(error = %e, "Failed to write to stdout");
                    break;
                }
                if let Err(e) = stdout.flush().await {
                    error!(error = %e, "Failed to flush stdout");
                    break;
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to serialize event");
            }
        }

        if is_shutdown {
            break;
        }
    }

    // Clean up
    stdin_handle.abort();
    info!("Discord gateway subprocess stopped");

    Ok(())
}
