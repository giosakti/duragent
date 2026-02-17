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

    /// Rate limited (429)
    #[error("rate limited (retry after {retry_after:?}s)")]
    RateLimit { retry_after: Option<u64> },
}

/// Check an HTTP response for rate-limit errors, returning `RateLimit` for 429.
pub fn check_response_error(response: &reqwest::Response) -> Option<LLMError> {
    if response.status().is_success() {
        return None;
    }
    if response.status().as_u16() == 429 {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        return Some(LLMError::RateLimit { retry_after });
    }
    None
}
