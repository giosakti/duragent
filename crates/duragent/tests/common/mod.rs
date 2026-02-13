#![cfg(feature = "server")]
//! Common test utilities.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::connect_info::MockConnectInfo;
use tokio::sync::Mutex;

use duragent::agent::AgentStore;
use duragent::background::BackgroundTasks;
use duragent::config::CompactionMode;
use duragent::llm::ProviderRegistry;
use duragent::sandbox::TrustSandbox;
use duragent::server::{self, AppState, RuntimeServices};
use duragent::session::{ChatSessionCache, SessionRegistry};
use duragent::store::file::{FileAgentCatalog, FilePolicyStore, FileSessionStore};

/// Create a test `AppState` with sensible defaults.
pub async fn test_app_state() -> AppState {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let sessions_path = tmp.path().join("sessions");
    std::fs::create_dir(&sessions_path).unwrap();

    // Leak the TempDir so it doesn't get cleaned up during the test.
    let tmp = Box::leak(Box::new(tmp));
    let sessions_path = tmp.path().join("sessions");

    let session_store = Arc::new(FileSessionStore::new(&sessions_path));
    let agents_dir = tmp.path().join("agents");
    let policy_store: Arc<dyn duragent::store::PolicyStore> =
        Arc::new(FilePolicyStore::new(&agents_dir));
    let (shutdown_tx, _shutdown_rx) = server::shutdown_channel();
    AppState {
        services: RuntimeServices {
            agents: empty_agent_store().await,
            providers: ProviderRegistry::new(),
            session_registry: SessionRegistry::new(session_store, CompactionMode::Disabled),
            sandbox: Arc::new(TrustSandbox::new()),
            policy_store,
            world_memory_path: tmp.path().join("memory/world"),
            workspace_directives_path: tmp.path().join("directives"),
            workspace_tools_path: tmp.path().join("tools"),
        },
        scheduler: None,
        policy_locks: duragent::sync::KeyedLocks::new(),
        admin_token: None,
        api_token: None,
        idle_timeout_seconds: 60,
        keep_alive_interval_seconds: 15,
        max_connections: 1024,
        background_tasks: BackgroundTasks::new(),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        workspace_hash: "test".to_string(),
        chat_session_cache: ChatSessionCache::new(),
        agents_dir,
    }
}

/// Create a test app with empty state.
///
/// Adds a `MockConnectInfo` layer with a loopback address so auth
/// middleware works in tests (localhost passes the "no token" fallback).
pub async fn test_app() -> Router {
    let state = test_app_state().await;
    let loopback: SocketAddr = ([127, 0, 0, 1], 0).into();
    server::build_app(state, 300).layer(MockConnectInfo(loopback))
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

    let catalog = FileAgentCatalog::new(&agents_dir);
    AgentStore::from_catalog(&catalog).await.store
}
