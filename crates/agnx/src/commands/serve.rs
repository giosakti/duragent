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
use agnx::config::{self, Config, ExternalGatewayConfig};
use agnx::gateway::{GatewayManager, SubprocessGateway};
use agnx::llm::ProviderRegistry;
use agnx::sandbox::{Sandbox, TrustSandbox};
use agnx::scheduler::{SchedulerConfig, SchedulerService};
use agnx::server;
use agnx::session::{ChatSessionCache, SessionRegistry};
use agnx::store::file::{
    FileAgentCatalog, FilePolicyStore, FileRunLogStore, FileScheduleStore, FileSessionStore,
};

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

    // Resolve paths relative to config file
    let config_path_ref = Path::new(config_path);
    let sessions_path = config::resolve_path(config_path_ref, &config.services.session.path);

    // Load agents, providers, and policy store
    let (store, providers, policy_store) = load_agents(config_path, &config).await;
    info!(agents = store.len(), "Loaded agents");

    // Initialize session store and registry, then recover persisted sessions
    let session_store: Arc<dyn agnx::store::SessionStore> =
        Arc::new(FileSessionStore::new(&sessions_path));
    let session_registry = SessionRegistry::new(session_store.clone());
    let recovery = session_registry.recover().await?;
    if recovery.recovered > 0 {
        info!(
            recovered = recovery.recovered,
            skipped = recovery.skipped,
            errors = recovery.errors.len(),
            "Recovered sessions from disk"
        );
    }

    // Initialize sandbox based on config (needed for gateway handler)
    let sandbox: Arc<dyn Sandbox> = match config.sandbox.mode.as_str() {
        "trust" => Arc::new(TrustSandbox::new()),
        other => {
            warn!(mode = %other, "Unknown sandbox mode, falling back to trust");
            Arc::new(TrustSandbox::new())
        }
    };
    info!(mode = %sandbox.mode(), "Sandbox initialized");

    // Initialize gateway manager with configured timeout
    let gateways = GatewayManager::new(std::time::Duration::from_secs(
        config.server.request_timeout_seconds,
    ));

    // Per-agent locks for policy file writes (shared with app state)
    let policy_locks = agnx::sync::KeyedLocks::with_cleanup("policy_locks");

    // Create shared chat session cache for gateway/scheduler session reuse
    let chat_session_cache = ChatSessionCache::new();

    // Rebuild cache from recovered sessions
    let recovered_session_ids: Vec<String> = session_registry
        .list()
        .await
        .into_iter()
        .map(|m| m.id)
        .collect();
    chat_session_cache
        .rebuild_from_sessions(&session_store, &recovered_session_ids)
        .await;

    // Initialize scheduler service (before gateway handler so it can be passed in)
    let schedules_path = sessions_path
        .parent()
        .unwrap_or(&sessions_path)
        .join("schedules");
    let schedule_store = Arc::new(FileScheduleStore::new(&schedules_path));
    let run_log_store = Arc::new(FileRunLogStore::new(schedules_path.join("runs")));
    let scheduler_config = SchedulerConfig {
        schedule_store,
        run_log_store,
        agents: store.clone(),
        providers: providers.clone(),
        session_registry: session_registry.clone(),
        gateways: gateways.clone(),
        sandbox: sandbox.clone(),
        policy_store: policy_store.clone(),
        chat_session_cache: chat_session_cache.clone(),
    };
    let scheduler_service = SchedulerService::new(scheduler_config);
    let scheduler_handle = scheduler_service.start().await;
    info!("Scheduler service started");

    // Set up gateway message handler with sandbox and gateway_manager
    let routing_config = build_routing_config(&config, &store);
    let gateway_handler =
        agnx::gateway::GatewayMessageHandler::new(agnx::gateway::GatewayHandlerConfig {
            agents: store.clone(),
            providers: providers.clone(),
            session_registry: session_registry.clone(),
            routing_config,
            sandbox: sandbox.clone(),
            gateway_manager: gateways.clone(),
            policy_store: policy_store.clone(),
            policy_locks: policy_locks.clone(),
            scheduler: Some(scheduler_handle.clone()),
            chat_session_cache,
        });

    gateways
        .set_handler(std::sync::Arc::new(gateway_handler))
        .await;

    // Start Telegram gateway if configured
    #[cfg(feature = "gateway-telegram")]
    if let Some(ref telegram_config) = config.gateways.telegram
        && telegram_config.enabled
    {
        start_telegram_gateway(&gateways, telegram_config.clone()).await;
    }

    // Start external gateways from config
    for gateway_config in &config.gateways.external {
        let mut resolved_config = gateway_config.clone();
        // Resolve command path relative to config file
        let command_path =
            config::resolve_path(config_path_ref, Path::new(&gateway_config.command));
        resolved_config.command = command_path.to_string_lossy().to_string();
        start_subprocess_gateway(&gateways, resolved_config).await;
    }

    // Create shutdown channel for HTTP-triggered shutdown
    let (shutdown_tx, shutdown_rx) = server::shutdown_channel();

    // Build app state
    let background_tasks = BackgroundTasks::new();
    let state = server::AppState {
        agents: store,
        providers,
        session_registry: session_registry.clone(),
        idle_timeout_seconds: config.server.idle_timeout_seconds,
        keep_alive_interval_seconds: config.server.keep_alive_interval_seconds,
        background_tasks: background_tasks.clone(),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        admin_token: config.server.admin_token.clone(),
        gateways: gateways.clone(),
        sandbox,
        policy_store,
        policy_locks,
        scheduler: Some(scheduler_handle.clone()),
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

    // Shutdown scheduler gracefully
    scheduler_handle.shutdown().await;

    // Shutdown session registry (flush all pending events and snapshots)
    session_registry.shutdown().await;

    // Shutdown gateways gracefully
    gateways.shutdown().await;

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
///
/// Returns the agent store, provider registry, and policy store.
async fn load_agents(
    config_path: &str,
    config: &Config,
) -> (
    AgentStore,
    ProviderRegistry,
    Arc<dyn agnx::store::PolicyStore>,
) {
    let agents_dir = config::resolve_path(Path::new(config_path), &config.agents_dir);
    let catalog = FileAgentCatalog::new(&agents_dir);
    let scan = agent::AgentStore::from_catalog(&catalog).await;
    agent::log_scan_warnings(&scan.warnings);

    let providers = ProviderRegistry::from_env();
    let policy_store: Arc<dyn agnx::store::PolicyStore> =
        Arc::new(FilePolicyStore::new(&agents_dir));

    (scan.store, providers, policy_store)
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

/// Start the Telegram gateway in a background task.
#[cfg(feature = "gateway-telegram")]
async fn start_telegram_gateway(
    gateways: &GatewayManager,
    config: agnx::config::TelegramGatewayConfig,
) {
    use agnx::gateway::{TelegramConfig, TelegramGateway};

    let (cmd_rx, evt_tx) = gateways.register("telegram", vec![]).await;

    // TelegramGateway only needs the bot token - routing is handled by agnx core
    let gateway_config = TelegramConfig::new(&config.bot_token);
    let gateway = TelegramGateway::new(gateway_config);

    tokio::spawn(async move {
        gateway.start(evt_tx, cmd_rx).await;
    });

    info!("Telegram gateway started");
}

/// Build routing configuration for gateway messages.
fn build_routing_config(
    config: &agnx::config::Config,
    agents: &agnx::agent::AgentStore,
) -> agnx::gateway::RoutingConfig {
    // Validate routing rule agents exist
    for rule in &config.routes {
        if agents.get(&rule.agent).is_none() {
            warn!(agent = %rule.agent, "Routing rule agent not found");
        }
    }

    agnx::gateway::RoutingConfig::new(config.routes.clone())
}

/// Start an external subprocess gateway.
async fn start_subprocess_gateway(gateways: &GatewayManager, config: ExternalGatewayConfig) {
    let gateway_name = config.name.clone();
    let (cmd_rx, evt_tx) = gateways.register(&gateway_name, vec![]).await;

    let gateway = SubprocessGateway::new(config);

    tokio::spawn(async move {
        gateway.run(evt_tx, cmd_rx).await;
    });

    info!(gateway = %gateway_name, "Subprocess gateway started");
}
