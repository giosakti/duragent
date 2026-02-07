//! Sandbox execution environments for tool isolation.

mod error;
mod trust;

pub use error::SandboxError;
pub use trust::TrustSandbox;

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

/// Default timeout for command execution (2 minutes).
pub const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(120);

/// Result of command execution.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Sandbox execution environment for tools.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Execute a command in the sandbox with optional timeout.
    ///
    /// If `timeout` is `None`, uses `DEFAULT_EXEC_TIMEOUT`.
    async fn exec(
        &self,
        cmd: &str,
        args: &[String],
        cwd: Option<&Path>,
        timeout: Option<Duration>,
    ) -> Result<ExecResult, SandboxError>;

    /// Get the sandbox mode name.
    fn mode(&self) -> &'static str;
}
