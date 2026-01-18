use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::agent::AgentStore;
use crate::handlers;

pub fn build_app(agent_store: AgentStore, request_timeout_secs: u64) -> Router {
    let api_v1 = Router::new()
        .route("/agents", get(handlers::list_agents))
        .route("/agents/{name}", get(handlers::get_agent))
        .with_state(agent_store);

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
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(request_timeout_secs),
        ))
}
