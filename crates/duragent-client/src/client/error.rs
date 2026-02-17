//! Client error types.

use thiserror::Error;

/// Result type for client operations.
pub type Result<T> = std::result::Result<T, ClientError>;

/// Errors that can occur when communicating with an duragent server.
#[derive(Debug, Error)]
pub enum ClientError {
    /// HTTP request failed.
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// Server returned an error response.
    #[error("api error ({status}): {message}")]
    ApiError { status: u16, message: String },

    /// Server health check failed.
    #[error("server unhealthy (status {status})")]
    ServerUnhealthy { status: u16 },

    /// Failed to parse SSE event.
    #[error("failed to parse sse event: {0}")]
    SseParseError(String),

    /// SSE stream ended unexpectedly.
    #[error("sse stream ended unexpectedly")]
    SseStreamEnded,
}
