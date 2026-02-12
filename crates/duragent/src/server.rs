use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tokio::sync::{Mutex, oneshot};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::agent::{AgentStore, PolicyLocks};
use crate::background::BackgroundTasks;
use crate::gateway::GatewayManager;
use crate::handlers;
use crate::llm::ProviderRegistry;
use crate::sandbox::Sandbox;
use crate::scheduler::SchedulerHandle;
use crate::session::{ChatSessionCache, SessionRegistry};
use crate::store::PolicyStore;

// ============================================================================
// Runtime Services
// ============================================================================

/// Shared runtime services used across AppState, gateway handler, and scheduler.
#[derive(Clone)]
pub struct RuntimeServices {
    pub agents: AgentStore,
    pub providers: ProviderRegistry,
    pub session_registry: SessionRegistry,
    pub gateways: GatewayManager,
    pub sandbox: Arc<dyn Sandbox>,
    pub policy_store: Arc<dyn PolicyStore>,
    pub world_memory_path: PathBuf,
    pub workspace_directives_path: PathBuf,
    pub chat_session_cache: ChatSessionCache,
    pub workspace_hash: String,
}

// ============================================================================
// Application State
// ============================================================================

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub services: RuntimeServices,
    pub scheduler: Option<SchedulerHandle>,
    pub policy_locks: PolicyLocks,
    pub admin_token: Option<String>,
    pub api_token: Option<String>,
    pub idle_timeout_seconds: u64,
    pub keep_alive_interval_seconds: u64,
    pub max_connections: usize,
    pub background_tasks: BackgroundTasks,
    pub shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

// ============================================================================
// Server Setup
// ============================================================================

/// Create a shutdown channel pair.
///
/// Returns (sender for AppState, receiver for shutdown_signal).
pub fn shutdown_channel() -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
    oneshot::channel()
}

pub fn build_app(state: AppState, request_timeout_seconds: u64) -> Router {
    let max_connections = state.max_connections;

    // SSE streaming routes - no request timeout (uses idle timeout internally)
    let streaming_routes = Router::new()
        .route(
            "/sessions/{session_id}/stream",
            post(handlers::v1::stream_session),
        )
        .with_state(state.clone());

    // Regular API routes - with request timeout
    let api_routes = Router::new()
        .route("/agents", get(handlers::v1::list_agents))
        .route("/agents/{name}", get(handlers::v1::get_agent))
        .route(
            "/sessions",
            get(handlers::v1::list_sessions).post(handlers::v1::create_session),
        )
        .route(
            "/sessions/{session_id}",
            get(handlers::v1::get_session).delete(handlers::v1::delete_session),
        )
        .route(
            "/sessions/{session_id}/messages",
            get(handlers::v1::get_messages).post(handlers::v1::send_message),
        )
        .route(
            "/sessions/{session_id}/approve",
            post(handlers::v1::approve_command),
        )
        .with_state(state.clone())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(request_timeout_seconds),
        ));

    let api_v1 = Router::new()
        .merge(streaming_routes)
        .merge(api_routes)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2 MB
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            handlers::api_auth::require_api_token,
        ))
        .layer(ConcurrencyLimitLayer::new(max_connections));

    // Admin routes (no timeout, state required for shutdown)
    let admin_routes = Router::new()
        .route("/shutdown", post(handlers::shutdown))
        .with_state(state.clone());

    Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .route("/version", get(handlers::version))
        .with_state(state)
        .nest("/api/v1", api_v1)
        .nest("/api/admin/v1", admin_routes)
}
