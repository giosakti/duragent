//! Admin handlers for server management.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

use super::api_auth;
use crate::server::AppState;

/// POST /api/admin/v1/shutdown
///
/// Triggers a graceful server shutdown.
///
/// Authorization:
/// - If `admin_token` is configured: requires `Authorization: Bearer <token>` header
/// - If `admin_token` is not configured: only accepts requests from localhost
pub async fn shutdown(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !api_auth::is_authorized(&state.admin_token, &addr, &headers) {
        return (StatusCode::FORBIDDEN, "Admin access denied").into_response();
    }

    if let Some(tx) = state.shutdown_tx.lock().await.take() {
        let _ = tx.send(());
        (StatusCode::OK, "Shutdown initiated").into_response()
    } else {
        (StatusCode::CONFLICT, "Shutdown already in progress").into_response()
    }
}
