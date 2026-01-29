//! Session error types.

use std::path::PathBuf;

use thiserror::Error;

/// Errors that can occur during session operations.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Failed to read or write a file.
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to serialize data to JSON.
    #[error("json serialization error: {0}")]
    JsonSerialize(#[from] serde_json::Error),

    /// Failed to parse YAML data.
    #[error("yaml parse error: {0}")]
    YamlParse(#[from] serde_saphyr::Error),

    /// Failed to serialize data to YAML.
    #[error("yaml serialization error: {0}")]
    YamlSerialize(#[from] serde_saphyr::ser::Error),

    /// Session not found.
    #[error("session not found: {0}")]
    NotFound(String),

    /// Incompatible snapshot schema version.
    #[error("incompatible snapshot schema version: expected {expected}, got {actual}")]
    IncompatibleSchema { expected: String, actual: String },

    /// LLM provider error during chat.
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LLMError),
}

/// Result type for session operations.
pub type Result<T> = std::result::Result<T, SessionError>;

impl SessionError {
    /// Create an IO error with the given path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
