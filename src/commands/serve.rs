//! HTTP server command implementation.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use tokio::signal;
use tracing::info;

use agnx::agent;
use agnx::config::Config;
use agnx::llm;
use agnx::server;
use agnx::session;

pub async fn run(
    config_path: String,
    host_override: Option<IpAddr>,
    port_override: Option<u16>,
    agents_dir_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::load(&config_path)?;

    // CLI overrides config
    if let Some(host) = host_override {
        config.server.host = host.to_string();
    }
    if let Some(port) = port_override {
        config.server.port = port;
    }
    if let Some(dir) = agents_dir_override {
        config.agents_dir = dir;
    }

    // Load agents from configured directory
    let agents_dir = agent::resolve_agents_dir(Path::new(&config_path), &config.agents_dir);
    let scan = agent::AgentStore::scan(&agents_dir);
    info!(agents_dir = %agents_dir.display(), agents = scan.store.len(), "Loaded agents");
    agent::log_scan_warnings(&scan.warnings);

    // Initialize LLM providers from environment
    let providers = llm::ProviderRegistry::from_env();

    // Initialize session store
    let sessions = session::SessionStore::new();

    // Build app state
    let state = server::AppState {
        agents: scan.store,
        providers,
        sessions,
    };

    let app = server::build_app(state, config.server.request_timeout);

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
