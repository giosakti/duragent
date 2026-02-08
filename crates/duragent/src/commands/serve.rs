//! HTTP server command implementation.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::signal;
use tokio::sync::Mutex;
use tracing::{info, warn};

use duragent::agent::{self, AgentStore};
use duragent::background::BackgroundTasks;
use duragent::client::AgentClient;
use duragent::config::{self, Config, ExternalGatewayConfig};
use duragent::gateway::{GatewayManager, SubprocessGateway};
use duragent::llm::ProviderRegistry;
use duragent::sandbox::{Sandbox, TrustSandbox};
use duragent::scheduler::{SchedulerConfig, SchedulerService};
use duragent::server::{self, RuntimeServices};
use duragent::session::{ChatSessionCache, SessionRegistry};
use duragent::store::file::{
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
        config.agents_dir = Some(dir.to_path_buf());
    }

    // Resolve workspace root, then derive paths from it when not explicitly set
    let config_path_ref = Path::new(config_path);
    let workspace_raw = config
        .workspace
        .as_deref()
        .unwrap_or(Path::new(config::DEFAULT_WORKSPACE));
    let workspace = config::resolve_path(config_path_ref, workspace_raw);
    let agents_dir = config
        .agents_dir
        .as_ref()
        .map(|p| config::resolve_path(config_path_ref, p))
        .unwrap_or_else(|| workspace.join(config::DEFAULT_AGENTS_DIR));
    let sessions_path = config
        .services
        .session
        .path
        .as_ref()
        .map(|p| config::resolve_path(config_path_ref, p))
        .unwrap_or_else(|| workspace.join(config::DEFAULT_SESSIONS_DIR));
    let world_memory_path = config
        .world_memory
        .path
        .as_ref()
        .map(|p| config::resolve_path(config_path_ref, p))
        .unwrap_or_else(|| workspace.join(config::DEFAULT_WORLD_MEMORY_DIR));
    let workspace_directives_path = workspace.join(config::DEFAULT_DIRECTIVES_DIR);

    // Load agents, providers, and policy store
    let (store, providers, policy_store) = load_agents(&agents_dir).await;
    info!(agents = store.len(), "Loaded agents");

    // Auto-create memory directive per agent that has memory enabled
    for (_, agent) in store.iter() {
        if agent.memory.is_some() {
            let agent_directives_path = agent.agent_dir.join("directives");
            duragent::context::ensure_memory_directive(
                &agent_directives_path,
                duragent::memory::DEFAULT_MEMORY_DIRECTIVE,
            );
        }
    }

    // Initialize session store and registry, then recover persisted sessions
    let session_store: Arc<dyn duragent::store::SessionStore> =
        Arc::new(FileSessionStore::new(&sessions_path));
    let session_registry = SessionRegistry::new(session_store.clone(), config.sessions.compaction);
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
    let policy_locks = duragent::sync::KeyedLocks::with_cleanup("policy_locks");

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

    // Spawn session TTL expiry loop
    if config.sessions.ttl_hours > 0 {
        let expiry_registry = session_registry.clone();
        let expiry_cache = chat_session_cache.clone();
        let expiry_agents = store.clone();
        let ttl_hours = config.sessions.ttl_hours;
        tokio::spawn(async move {
            let ttl = chrono::Duration::hours(ttl_hours as i64);
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // skip immediate tick
            loop {
                interval.tick().await;
                expiry_registry
                    .expire_inactive_sessions(ttl, &expiry_cache, &expiry_agents)
                    .await;
            }
        });
        info!(
            ttl_hours = config.sessions.ttl_hours,
            "Session TTL expiry enabled"
        );
    }

    // Initialize scheduler service (before gateway handler so it can be passed in)
    let schedules_path = sessions_path
        .parent()
        .unwrap_or(&sessions_path)
        .join("schedules");
    // Build shared RuntimeServices once
    let services = RuntimeServices {
        agents: store.clone(),
        providers: providers.clone(),
        session_registry: session_registry.clone(),
        gateways: gateways.clone(),
        sandbox: sandbox.clone(),
        policy_store: policy_store.clone(),
        world_memory_path: world_memory_path.clone(),
        workspace_directives_path: workspace_directives_path.clone(),
        chat_session_cache: chat_session_cache.clone(),
    };

    let schedule_store = Arc::new(FileScheduleStore::new(&schedules_path));
    let run_log_store = Arc::new(FileRunLogStore::new(schedules_path.join("runs")));
    let scheduler_config = SchedulerConfig {
        services: services.clone(),
        schedule_store,
        run_log_store,
        chat_session_cache: chat_session_cache.clone(),
    };
    let scheduler_service = SchedulerService::new(scheduler_config);
    let scheduler_handle = scheduler_service.start().await;
    info!("Scheduler service started");

    // Set up gateway message handler with sandbox and gateway_manager
    let routing_config = build_routing_config(&config, &store);
    let gateway_handler =
        duragent::gateway::GatewayMessageHandler::new(duragent::gateway::GatewayHandlerConfig {
            services: services.clone(),
            routing_config,
            policy_locks: policy_locks.clone(),
            scheduler: Some(scheduler_handle.clone()),
            chat_session_cache,
        });

    gateways
        .set_handler(std::sync::Arc::new(gateway_handler))
        .await;

    // Start Discord gateway if configured
    #[cfg(feature = "gateway-discord")]
    if let Some(ref discord_config) = config.gateways.discord
        && discord_config.enabled
    {
        start_discord_gateway(&gateways, discord_config.clone()).await;
    }

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
        services,
        scheduler: Some(scheduler_handle.clone()),
        policy_locks,
        admin_token: config.server.admin_token.clone(),
        idle_timeout_seconds: config.server.idle_timeout_seconds,
        keep_alive_interval_seconds: config.server.keep_alive_interval_seconds,
        background_tasks: background_tasks.clone(),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
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

/// Load agents from the resolved directory and initialize providers.
///
/// Returns the agent store, provider registry, and policy store.
async fn load_agents(
    agents_dir: &Path,
) -> (
    AgentStore,
    ProviderRegistry,
    Arc<dyn duragent::store::PolicyStore>,
) {
    let catalog = FileAgentCatalog::new(agents_dir.to_path_buf());
    let scan = agent::AgentStore::from_catalog(&catalog).await;
    agent::log_scan_warnings(&scan.warnings);

    let providers = ProviderRegistry::from_env();
    let policy_store: Arc<dyn duragent::store::PolicyStore> =
        Arc::new(FilePolicyStore::new(agents_dir.to_path_buf()));

    (scan.store, providers, policy_store)
}

async fn shutdown_signal(http_shutdown: tokio::sync::oneshot::Receiver<()>) {
    let ctrl_c = async {
        if let Err(e) = signal::ctrl_c().await {
            warn!(error = %e, "Failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                warn!(error = %e, "Failed to install SIGTERM handler");
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

/// Start the Discord gateway in a background task.
#[cfg(feature = "gateway-discord")]
async fn start_discord_gateway(
    gateways: &GatewayManager,
    config: duragent::config::DiscordGatewayConfig,
) {
    use duragent::gateway::{DiscordConfig, DiscordGateway};

    let (cmd_rx, evt_tx) = gateways.register("discord", vec![]).await;

    let gateway_config = DiscordConfig::new(&config.bot_token);
    let gateway = DiscordGateway::new(gateway_config);

    tokio::spawn(async move {
        gateway.start(evt_tx, cmd_rx).await;
    });

    info!("Discord gateway started");
}

/// Start the Telegram gateway in a background task.
#[cfg(feature = "gateway-telegram")]
async fn start_telegram_gateway(
    gateways: &GatewayManager,
    config: duragent::config::TelegramGatewayConfig,
) {
    use duragent::gateway::{TelegramConfig, TelegramGateway};

    let (cmd_rx, evt_tx) = gateways.register("telegram", vec![]).await;

    // TelegramGateway only needs the bot token - routing is handled by duragent core
    let gateway_config = TelegramConfig::new(&config.bot_token);
    let gateway = TelegramGateway::new(gateway_config);

    tokio::spawn(async move {
        gateway.start(evt_tx, cmd_rx).await;
    });

    info!("Telegram gateway started");
}

/// Build routing configuration for gateway messages.
fn build_routing_config(
    config: &duragent::config::Config,
    agents: &duragent::agent::AgentStore,
) -> duragent::gateway::RoutingConfig {
    // Validate routing rule agents exist
    for rule in &config.routes {
        if agents.get(&rule.agent).is_none() {
            warn!(agent = %rule.agent, "Routing rule agent not found");
        }
    }

    duragent::gateway::RoutingConfig::new(config.routes.clone())
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
