mod agent;
mod build_info;
mod config;
mod handlers;
mod response;
mod server;

use clap::{Parser, Subcommand};
use config::Config;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// Agnx - A minimal and fast self-hosted runtime for durable and portable AI agents
#[derive(Parser, Debug)]
#[command(version = build_info::VERSION_STRING, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the HTTP server
    Serve {
        /// Path to configuration file
        #[arg(short, long, default_value = "agnx.yaml")]
        config: String,

        /// Port to listen on (overrides config file)
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to (overrides config file)
        #[arg(long)]
        host: Option<IpAddr>,

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
        Commands::Serve {
            config,
            port,
            host,
            agents_dir,
        } => run_server(config, port, host, agents_dir).await,
    }
}

async fn run_server(
    config_path: String,
    port_override: Option<u16>,
    host_override: Option<IpAddr>,
    agents_dir_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::load(&config_path)?;

    // CLI overrides config
    if let Some(port) = port_override {
        config.server.port = port;
    }
    if let Some(host) = host_override {
        config.server.host = host.to_string();
    }
    if let Some(dir) = agents_dir_override {
        config.agents_dir = dir;
    }

    // Load agents from configured directory
    let agents_dir = agent::resolve_agents_dir(Path::new(&config_path), &config.agents_dir);
    let scan = agent::AgentStore::scan(&agents_dir);
    info!(agents_dir = %agents_dir.display(), agents = scan.store.len(), "Loaded agents");
    agent::log_scan_warnings(&scan.warnings);

    let app = server::build_app(scan.store, config.server.request_timeout);

    let ip: IpAddr = config.server.host.parse()?;
    let addr = SocketAddr::new(ip, config.server.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %addr, "Starting server");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("Server stopped");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down..."),
        _ = terminate => info!("Received SIGTERM, shutting down..."),
    }
}
