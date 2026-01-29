//! LLM error types.

use thiserror::Error;

/// Errors that can occur when making LLM API calls.
#[derive(Debug, Error)]
pub enum LLMError {
    /// HTTP request failed
    #[error("http request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// API returned an error response
    #[error("api error (status {status}): {message}")]
    Api { status: u16, message: String },
}
