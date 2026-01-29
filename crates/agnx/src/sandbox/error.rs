use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("command execution failed: {0}")]
    ExecutionFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
