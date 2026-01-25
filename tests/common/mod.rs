//! Common test utilities.

use axum::Router;

use agnx::agent::AgentStore;
use agnx::llm::ProviderRegistry;
use agnx::server::{self, AppState};
use agnx::session::SessionStore;

/// Create a test app with empty state.
pub fn test_app() -> Router {
    let state = AppState {
        agents: empty_agent_store(),
        providers: ProviderRegistry::new(),
        sessions: SessionStore::new(),
    };

    server::build_app(state, 30)
}

/// Create an empty agent store.
fn empty_agent_store() -> AgentStore {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    // Leak the TempDir so it doesn't get cleaned up during the test.
    // This is fine for tests - the OS will clean up on process exit.
    let tmp = Box::leak(Box::new(tmp));
    let agents_dir = tmp.path().join("agents");

    AgentStore::scan(&agents_dir).store
}
