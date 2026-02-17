//! Shared API types.

use serde::{Deserialize, Serialize};

/// Session status.
///
/// Used in session responses and client-side session handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is active and ready for messages.
    Active,
    /// Session is paused (client disconnected with on_disconnect: pause).
    Paused,
    /// Session is running in background (client disconnected with on_disconnect: continue).
    Running,
    /// Session has completed.
    Completed,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
            SessionStatus::Paused => write!(f, "paused"),
            SessionStatus::Running => write!(f, "running"),
            SessionStatus::Completed => write!(f, "completed"),
        }
    }
}
