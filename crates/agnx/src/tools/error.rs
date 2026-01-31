//! Tool execution errors.

use thiserror::Error;

/// Errors that can occur during tool execution.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool not found in configuration.
    #[error("tool not found: {0}")]
    NotFound(String),

    /// Command requires human approval before execution.
    #[error("approval required for: {command}")]
    ApprovalRequired {
        /// The tool call ID.
        call_id: String,
        /// The command or tool invocation that needs approval.
        command: String,
    },

    /// Command was denied by policy.
    #[error("denied by policy: {0}")]
    PolicyDenied(String),

    /// Failed to parse tool arguments.
    #[error("failed to parse tool arguments: {0}")]
    InvalidArguments(String),

    /// Tool execution failed.
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),

    /// Sandbox execution error.
    #[error("sandbox error: {0}")]
    Sandbox(#[from] crate::sandbox::SandboxError),

    /// I/O error (e.g., reading README).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
