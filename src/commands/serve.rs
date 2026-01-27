//! HTTP server command implementation.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::signal;
use tokio::sync::Mutex;
use tracing::{info, warn};

use agnx::agent::{self, AgentStore};
use agnx::background::BackgroundTasks;
use agnx::client::AgentClient;
use agnx::config::Config;
use agnx::llm::ProviderRegistry;
use agnx::server;
use agnx::session;

pub async fn run(
    config_path: &str,
    host_override: Option<IpAddr>,
    port_override: Option<u16>,
    agents_dir_override: Option<&Path>,
) -> Result<()> {
    let mut config = Config::load(config_path).await?;

    // CLI overrides config
    if let Some(host) = host_override {
        config.server.host = host.to_string();
    }
    if let Some(port) = port_override {
        config.server.port = port;
    }
    if let Some(dir) = agents_dir_override {
        config.agents_dir = dir.to_path_buf();
    }

    // Load agents and providers
    let (store, providers) = load_agents(config_path, &config).await;
    info!(agents = store.len(), "Loaded agents");

    // Initialize session store and recover persisted sessions
    let sessions = session::SessionStore::new();
    let recovery = session::recover_sessions(&config.services.session.path, &sessions).await?;
    if recovery.recovered > 0 {
        info!(
            recovered = recovery.recovered,
            skipped = recovery.skipped,
            errors = recovery.errors.len(),
            "Recovered sessions from disk"
        );
    }

    // Create shutdown channel for HTTP-triggered shutdown
    let (shutdown_tx, shutdown_rx) = server::shutdown_channel();

    // Build app state
    let background_tasks = BackgroundTasks::new();
    let state = server::AppState {
        agents: store,
        providers,
        sessions,
        sessions_path: config.services.session.path.clone(),
        idle_timeout_seconds: config.server.idle_timeout_seconds,
        keep_alive_interval_seconds: config.server.keep_alive_interval_seconds,
        background_tasks: background_tasks.clone(),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        admin_token: config.server.admin_token.clone(),
    };

    let app = server::build_app(state, config.server.request_timeout_seconds);

    let ip: IpAddr = config.server.host.parse()?;
    let addr = SocketAddr::new(ip, config.server.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %addr, "Starting server");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_rx))
    .await?;

    // Wait for background tasks to complete before exiting
    background_tasks.shutdown().await;

    info!("Server stopped");
    Ok(())
}

/// Stop a running server by calling the shutdown endpoint.
pub async fn stop(config_path: &str, port_override: Option<u16>) -> Result<()> {
    let config = Config::load(config_path).await?;
    let port = port_override.unwrap_or(config.server.port);

    let client = AgentClient::new(&format!("http://127.0.0.1:{}", port));

    // Check if server is running
    if client.health().await.is_err() {
        anyhow::bail!("No server running on port {}", port);
    }

    // Call shutdown endpoint
    client.shutdown().await.context("Failed to stop server")?;

    println!("Shutdown initiated for server on port {}", port);
    Ok(())
}

/// Load agents from the configured directory and initialize providers.
async fn load_agents(config_path: &str, config: &Config) -> (AgentStore, ProviderRegistry) {
    let agents_dir = agent::resolve_agents_dir(Path::new(config_path), &config.agents_dir);
    let scan = agent::AgentStore::scan(&agents_dir).await;
    agent::log_scan_warnings(&scan.warnings);

    let providers = ProviderRegistry::from_env();

    (scan.store, providers)
}

async fn shutdown_signal(http_shutdown: tokio::sync::oneshot::Receiver<()>) {
    let ctrl_c = async {
        if let Err(e) = signal::ctrl_c().await {
            warn!("Failed to install Ctrl+C handler: {}", e);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                warn!("Failed to install SIGTERM handler: {}", e);
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down..."),
        _ = terminate => info!("Received SIGTERM, shutting down..."),
        _ = http_shutdown => info!("Received shutdown request via HTTP, shutting down..."),
    }
}
