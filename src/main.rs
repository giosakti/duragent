mod commands;

use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing::error;
use tracing_subscriber::EnvFilter;

/// Agnx - A minimal and fast self-hosted runtime for durable and portable AI agents
#[derive(Parser, Debug)]
#[command(version = agnx::build_info::VERSION_STRING, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
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
    },

    /// Start the HTTP server
    Serve {
        /// Path to configuration file
        #[arg(short, long, default_value = "agnx.yaml")]
        config: String,

        /// Host to bind to (overrides config file)
        #[arg(long)]
        host: Option<IpAddr>,

        /// Port to listen on (overrides config file)
        #[arg(short, long)]
        port: Option<u16>,

        /// Agents directory (overrides config file). If relative, it is resolved relative to the config file directory.
        #[arg(long)]
        agents_dir: Option<PathBuf>,
    },
}

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

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Chat {
            agent,
            config,
            agents_dir,
        } => commands::chat::run(agent, config, agents_dir).await,
        Commands::Serve {
            config,
            host,
            port,
            agents_dir,
        } => commands::serve::run(config, host, port, agents_dir).await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
