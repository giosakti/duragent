use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tower_http::timeout::TimeoutLayer;

use crate::agent::AgentStore;
use crate::handlers;
use crate::llm::ProviderRegistry;
use crate::session::SessionStore;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub agents: AgentStore,
    pub providers: ProviderRegistry,
    pub sessions: SessionStore,
    pub idle_timeout_seconds: u64,
    pub keep_alive_interval_seconds: u64,
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
        .route("/sessions", post(handlers::v1::create_session))
        .route("/sessions/{session_id}", get(handlers::v1::get_session))
        .route(
            "/sessions/{session_id}/messages",
            post(handlers::v1::send_message),
        )
        .with_state(state)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(request_timeout_seconds),
        ));

    let api_v1 = Router::new().merge(streaming_routes).merge(api_routes);

    Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .route("/version", get(handlers::version))
        .nest("/api/v1", api_v1)
        .route("/example-bad-request", get(handlers::example_bad_request))
        .route("/example-not-found", get(handlers::example_not_found))
        .route(
            "/example-internal-error",
            get(handlers::example_internal_error),
        )
}
