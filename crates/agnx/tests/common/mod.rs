//! Common test utilities.

use std::sync::Arc;

use axum::Router;
use tokio::sync::Mutex;

use agnx::agent::AgentStore;
use agnx::background::BackgroundTasks;
use agnx::gateway::GatewayManager;
use agnx::llm::ProviderRegistry;
use agnx::sandbox::TrustSandbox;
use agnx::server::{self, AppState};
use agnx::session::SessionStore;

/// Create a test app with empty state.
pub async fn test_app() -> Router {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let sessions_path = tmp.path().join("sessions");
    std::fs::create_dir(&sessions_path).unwrap();

    // Leak the TempDir so it doesn't get cleaned up during the test.
    let tmp = Box::leak(Box::new(tmp));
    let sessions_path = tmp.path().join("sessions");

    let (shutdown_tx, _shutdown_rx) = server::shutdown_channel();
    let state = AppState {
        agents: empty_agent_store().await,
        providers: ProviderRegistry::new(),
        sessions: SessionStore::new(),
        sessions_path,
        idle_timeout_seconds: 60,
        keep_alive_interval_seconds: 15,
        background_tasks: BackgroundTasks::new(),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        admin_token: None,
        gateways: GatewayManager::new(),
        sandbox: Arc::new(TrustSandbox::new()),
        policy_locks: Arc::new(dashmap::DashMap::new()),
        session_locks: Arc::new(dashmap::DashMap::new()),
        scheduler: None,
    };

    server::build_app(state, 300)
}

/// Create an empty agent store.
pub async fn empty_agent_store() -> AgentStore {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    // Leak the TempDir so it doesn't get cleaned up during the test.
    // This is fine for tests - the OS will clean up on process exit.
    let tmp = Box::leak(Box::new(tmp));
    let agents_dir = tmp.path().join("agents");

    AgentStore::scan(&agents_dir).await.store
}
