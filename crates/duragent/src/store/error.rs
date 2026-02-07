//! Unified error types for storage operations.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    // ========================================================================
    // File-based backend errors
    // ========================================================================
    /// I/O error during file operations.
    #[error("I/O error at {path}: {source}")]
    FileIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Error deserializing file contents.
    #[error("deserialization error at {path}: {message}")]
    FileDeserialization { path: PathBuf, message: String },

    /// Schema version mismatch in file.
    #[error("incompatible schema version {found} at {path}, expected {expected}")]
    FileIncompatibleSchema {
        path: PathBuf,
        expected: String,
        found: String,
    },

    // ========================================================================
    // Generic errors (any backend)
    // ========================================================================
    /// Error serializing data.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Entity not found.
    #[error("{entity_type} not found: {id}")]
    NotFound {
        entity_type: &'static str,
        id: String,
    },
}

impl StorageError {
    // ========================================================================
    // File-based backend helpers
    // ========================================================================
    /// Create a file I/O error with path context.
    pub fn file_io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::FileIo {
            path: path.into(),
            source,
        }
    }

    /// Create a file deserialization error with path context.
    pub fn file_deserialization(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::FileDeserialization {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create a file schema incompatibility error.
    pub fn file_incompatible_schema(
        path: impl Into<PathBuf>,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        Self::FileIncompatibleSchema {
            path: path.into(),
            expected: expected.into(),
            found: found.into(),
        }
    }

    // ========================================================================
    // Generic helpers (any backend)
    // ========================================================================
    /// Create a serialization error.
    pub fn serialization(message: impl Into<String>) -> Self {
        Self::Serialization(message.into())
    }

    /// Create a not found error.
    pub fn not_found(entity_type: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity_type,
            id: id.into(),
        }
    }
}

/// Convenience type alias for storage results.
pub type StorageResult<T> = Result<T, StorageError>;
