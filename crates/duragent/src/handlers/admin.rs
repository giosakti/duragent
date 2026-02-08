//! Admin handlers for server management.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

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
    if !is_admin_authorized(&state.admin_token, &addr, &headers) {
        return (StatusCode::FORBIDDEN, "Admin access denied").into_response();
    }

    if let Some(tx) = state.shutdown_tx.lock().await.take() {
        let _ = tx.send(());
        (StatusCode::OK, "Shutdown initiated").into_response()
    } else {
        (StatusCode::CONFLICT, "Shutdown already in progress").into_response()
    }
}

/// Check if a request is authorized for admin access.
///
/// - If `admin_token` is configured: requires matching `Authorization: Bearer <token>` header
/// - If `admin_token` is not configured: only accepts requests from loopback addresses
fn is_admin_authorized(
    admin_token: &Option<String>,
    addr: &SocketAddr,
    headers: &HeaderMap,
) -> bool {
    match admin_token {
        Some(expected_token) => {
            // Token configured: require Authorization header
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|token| {
                    use sha2::{Digest, Sha256};
                    let a = Sha256::digest(token.as_bytes());
                    let b = Sha256::digest(expected_token.as_bytes());
                    a == b
                })
                .unwrap_or(false)
        }
        None => {
            // No token configured: only allow localhost
            addr.ip().is_loopback()
        }
    }
}
