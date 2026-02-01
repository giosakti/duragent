//! Scheduler error types.

use thiserror::Error;

/// Errors that can occur in the scheduler.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Schedule not found.
    #[error("schedule not found: {0}")]
    NotFound(String),

    /// Invalid schedule configuration.
    #[error("invalid schedule: {0}")]
    InvalidSchedule(String),

    /// Invalid cron expression.
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),

    /// Timestamp is in the past.
    #[error("timestamp is in the past")]
    PastTimestamp,

    /// Storage error.
    #[error("storage error: {0}")]
    Storage(String),

    /// Schedule execution failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// Gateway not available.
    #[error("gateway not available: {0}")]
    GatewayUnavailable(String),

    /// Agent not found.
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// Not authorized to modify this schedule.
    #[error("not authorized: schedule belongs to agent '{0}'")]
    NotAuthorized(String),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for scheduler operations.
pub type Result<T> = std::result::Result<T, SchedulerError>;
