//! Sandbox execution environments for tool isolation.

mod error;
mod trust;

pub use error::SandboxError;
pub use trust::TrustSandbox;

use std::path::Path;

use async_trait::async_trait;

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
    /// Execute a command in the sandbox.
    async fn exec(
        &self,
        cmd: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<ExecResult, SandboxError>;

    /// Get the sandbox mode name.
    fn mode(&self) -> &'static str;
}
