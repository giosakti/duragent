mod commands;

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::error;
use tracing_subscriber::EnvFilter;

// ============================================================================
// CLI Types
// ============================================================================

/// Agnx - A minimal and fast self-hosted runtime for durable and portable AI agents
#[derive(Parser, Debug)]
#[command(version = agnx::build_info::VERSION, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Attach to an existing session
    Attach {
        /// Session ID to attach to (omit to list attachable sessions)
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,

        /// List all attachable sessions
        #[arg(short, long)]
        list: bool,

        /// Path to configuration file
        #[arg(short, long, default_value = "agnx.yaml")]
        config: String,

        /// Agents directory (overrides config file)
        #[arg(long)]
        agents_dir: Option<PathBuf>,

        /// Connect to a specific server URL instead of auto-starting
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Start an interactive chat session with an agent
    Chat {
        /// Name of the agent to chat with
        #[arg(short, long)]
        agent: String,

        /// Path to configuration file
        #[arg(short, long, default_value = "agnx.yaml")]
        config: String,

        /// Agents directory (overrides config file)
        #[arg(long)]
        agents_dir: Option<PathBuf>,

        /// Connect to a specific server URL instead of auto-starting
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Manage the HTTP server
    Serve {
        #[command(subcommand)]
        action: Option<ServeAction>,

        /// Path to configuration file
        #[arg(short, long, default_value = "agnx.yaml", global = true)]
        config: String,

        /// Host to bind to (overrides config file)
        #[arg(long, global = true)]
        host: Option<IpAddr>,

        /// Port to listen on (overrides config file)
        #[arg(short, long, global = true)]
        port: Option<u16>,

        /// Agents directory (overrides config file). If relative, it is resolved relative to the config file directory.
        #[arg(long, global = true)]
        agents_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum ServeAction {
    /// Stop a running server
    Stop,
}

// ============================================================================
// Entry Point
// ============================================================================

#[tokio::main]
async fn main() -> std::process::ExitCode {
    init_tracing();

    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Attach {
            session_id,
            list,
            config,
            agents_dir,
            server,
        } => match session_id {
            Some(id) if !list => {
                commands::attach::run(&id, &config, agents_dir.as_deref(), server.as_deref()).await
            }
            _ => commands::attach::list(&config, agents_dir.as_deref(), server.as_deref()).await,
        },
        Commands::Chat {
            agent,
            config,
            agents_dir,
            server,
        } => commands::chat::run(&agent, &config, agents_dir.as_deref(), server.as_deref()).await,
        Commands::Serve {
            action,
            config,
            host,
            port,
            agents_dir,
        } => match action {
            Some(ServeAction::Stop) => commands::serve::stop(&config, port).await,
            None => commands::serve::run(&config, host, port, agents_dir.as_deref()).await,
        },
    }
}

// ============================================================================
// Initialization
// ============================================================================

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
