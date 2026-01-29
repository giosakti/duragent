use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tokio::sync::{Mutex, oneshot};
use tower_http::timeout::TimeoutLayer;

use crate::agent::AgentStore;
use crate::background::BackgroundTasks;
use crate::gateway::GatewayManager;
use crate::handlers;
use crate::llm::ProviderRegistry;
use crate::session::SessionStore;

// ============================================================================
// Application State
// ============================================================================

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub agents: AgentStore,
    pub providers: ProviderRegistry,
    pub sessions: SessionStore,
    pub sessions_path: PathBuf,
    pub idle_timeout_seconds: u64,
    pub keep_alive_interval_seconds: u64,
    /// Registry for background tasks that should be awaited on shutdown.
    pub background_tasks: BackgroundTasks,
    /// Shutdown signal sender (taken when shutdown is triggered).
    pub shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Optional admin API token. If set, admin endpoints require this token.
    /// If not set, admin endpoints only accept requests from localhost.
    pub admin_token: Option<String>,
    /// Gateway manager for platform integrations.
    pub gateways: GatewayManager,
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
        .route("/sessions/{session_id}", get(handlers::v1::get_session))
        .route(
            "/sessions/{session_id}/messages",
            get(handlers::v1::get_messages).post(handlers::v1::send_message),
        )
        .with_state(state.clone())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(request_timeout_seconds),
        ));

    let api_v1 = Router::new().merge(streaming_routes).merge(api_routes);

    // Admin routes (no timeout, state required for shutdown)
    let admin_routes = Router::new()
        .route("/shutdown", post(handlers::shutdown))
        .with_state(state);

    Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .route("/version", get(handlers::version))
        .nest("/api/v1", api_v1)
        .nest("/api/admin/v1", admin_routes)
}
