//! Shared bearer token authentication.
//!
//! Used by both API and admin route middleware/handlers.
//!
//! Behavior:
//! - Token configured: requires `Authorization: Bearer <token>` header
//! - Token not configured: only accepts requests from loopback addresses

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::server::AppState;

/// Check if a request is authorized against an optional token.
///
/// - If token is `Some`: requires matching `Authorization: Bearer <token>` header (constant-time via SHA-256)
/// - If token is `None`: only allows requests from loopback addresses
pub fn is_authorized(token: &Option<String>, addr: &SocketAddr, headers: &HeaderMap) -> bool {
    match token {
        Some(expected) => headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|provided| {
                let a = Sha256::digest(provided.as_bytes());
                let b = Sha256::digest(expected.as_bytes());
                a == b
            }),
        None => addr.ip().is_loopback(),
    }
}

/// Middleware that guards API routes (`/api/v1/*`).
///
/// Uses `api_token` from `AppState`. Always installed — falls back to
/// localhost-only when no token is configured.
pub async fn require_api_token(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if is_authorized(&state.api_token, &addr, request.headers()) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
