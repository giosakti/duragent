use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use super::{DEFAULT_EXEC_TIMEOUT, ExecResult, Sandbox, SandboxError};

/// No isolation — commands run directly in the host environment.
#[derive(Debug)]
pub struct TrustSandbox;

impl TrustSandbox {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TrustSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Sandbox for TrustSandbox {
    async fn exec(
        &self,
        cmd: &str,
        args: &[String],
        cwd: Option<&Path>,
        timeout: Option<Duration>,
    ) -> Result<ExecResult, SandboxError> {
        let timeout = timeout.unwrap_or(DEFAULT_EXEC_TIMEOUT);

        let mut command = Command::new(cmd);
        command.args(args);

        if let Some(dir) = cwd {
            command.current_dir(dir);
        }

        let output = match tokio::time::timeout(timeout, command.output()).await {
            Ok(result) => result?,
            Err(_) => return Err(SandboxError::Timeout(timeout)),
        };

        Ok(ExecResult {
            // Returns -1 if killed by signal (SIGKILL, SIGSEGV, etc.)
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn mode(&self) -> &'static str {
        "trust"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_exec_simple_command() {
        let sandbox = TrustSandbox::new();
        let result = sandbox
            .exec("echo", &["hello".to_string()], None, None)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.stderr.is_empty());
    }

    #[tokio::test]
    async fn test_exec_with_cwd() {
        let tmp_dir = TempDir::new().unwrap();
        let sandbox = TrustSandbox::new();
        let result = sandbox
            .exec("pwd", &[], Some(tmp_dir.path()), None)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), tmp_dir.path().to_str().unwrap());
    }

    #[tokio::test]
    async fn test_exec_failing_command() {
        let sandbox = TrustSandbox::new();
        let result = sandbox.exec("false", &[], None, None).await.unwrap();

        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_exec_command_not_found() {
        let sandbox = TrustSandbox::new();
        let result = sandbox
            .exec("nonexistent_command_12345", &[], None, None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_exec_timeout() {
        let sandbox = TrustSandbox::new();
        let result = sandbox
            .exec(
                "sleep",
                &["10".to_string()],
                None,
                Some(Duration::from_millis(100)),
            )
            .await;

        assert!(matches!(result, Err(SandboxError::Timeout(_))));
    }

    #[test]
    fn test_mode() {
        let sandbox = TrustSandbox::new();
        assert_eq!(sandbox.mode(), "trust");
    }

    #[test]
    fn test_default() {
        let sandbox: TrustSandbox = Default::default();
        assert_eq!(sandbox.mode(), "trust");
    }
}
