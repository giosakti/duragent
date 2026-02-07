//! Local server launcher for CLI commands.
//!
//! Auto-starts a local duragent server process when none is running.
//! Used by CLI commands to ensure a server is available before making requests.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, info};

use crate::client::AgentClient;
use crate::config::Config;

// ============================================================================
// Public Types
// ============================================================================

/// Result type for launcher operations.
pub type Result<T> = std::result::Result<T, LauncherError>;

/// Errors that can occur when launching or connecting to a server.
#[derive(Debug, Error)]
pub enum LauncherError {
    /// Failed to spawn server process.
    #[error("failed to spawn server: {0}")]
    SpawnFailed(#[from] std::io::Error),

    /// Server failed to become ready within timeout.
    #[error("server failed to start within {timeout_secs}s; logs: {log_path}")]
    StartupTimeout {
        timeout_secs: u64,
        log_path: PathBuf,
    },

    /// Server health check failed after startup.
    #[error("server started but health check failed: {0}")]
    HealthCheckFailed(String),
}

/// Options for launching a server.
pub struct LaunchOptions<'a> {
    /// Optional explicit server URL (e.g., from --server flag).
    pub server_url: Option<&'a str>,
    /// Path to config file.
    pub config_path: &'a Path,
    /// Loaded configuration.
    pub config: &'a Config,
    /// Optional agents directory override.
    pub agents_dir: Option<&'a Path>,
}

// ============================================================================
// Public API
// ============================================================================

/// Ensure a server is running and return a client connected to it.
///
/// If `server_url` is provided, connects to that server without auto-starting.
/// Otherwise, checks if a server is running on the configured port and starts one if needed.
///
/// # Returns
///
/// An `AgentClient` connected to the running server.
pub async fn ensure_server_running(opts: LaunchOptions<'_>) -> Result<AgentClient> {
    // If explicit server URL provided, just use it (no auto-start)
    if let Some(url) = opts.server_url {
        debug!(url = %url, "Using explicit server URL");
        return Ok(AgentClient::new(url));
    }

    // Build local server URL (always 127.0.0.1 for security)
    let port = opts.config.server.port;
    let local_url = format!("http://127.0.0.1:{}", port);
    let client = AgentClient::new(&local_url);

    // Check if server is already running
    if client.health().await.is_ok() {
        debug!(url = %local_url, "Server already running");
        return Ok(client);
    }

    // Server not running, start it
    info!(port = port, "Starting local server");
    let log_path = server_log_path(opts.config_path, port);
    spawn_server(opts.config_path, port, opts.agents_dir, &log_path).await?;

    // Wait for server to become ready
    wait_for_ready(&client, DEFAULT_STARTUP_TIMEOUT_SECS, &log_path).await?;

    info!(url = %local_url, "Server ready");
    Ok(client)
}

// ============================================================================
// Internal Constants
// ============================================================================

/// Default timeout for waiting for server to become ready.
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 10;

/// Initial polling interval.
const INITIAL_POLL_INTERVAL_MS: u64 = 100;

/// Maximum polling interval.
const MAX_POLL_INTERVAL_MS: u64 = 1000;

// ============================================================================
// Internal Helpers
// ============================================================================

/// Spawn the duragent server as a background process.
async fn spawn_server(
    config_path: &Path,
    port: u16,
    agents_dir: Option<&Path>,
    log_path: &Path,
) -> Result<()> {
    let exe = std::env::current_exe()?;

    let mut cmd = Command::new(&exe);
    cmd.arg("serve")
        .arg("--config")
        .arg(config_path)
        .arg("--host")
        .arg("127.0.0.1") // Always bind to localhost for security
        .arg("--port")
        .arg(port.to_string());

    // Pass agents-dir if specified
    if let Some(dir) = agents_dir {
        cmd.arg("--agents-dir").arg(dir);
    }

    if let Some(log_dir) = log_path.parent() {
        tokio::fs::create_dir_all(log_dir).await?;
    }

    // Note: OpenOptions uses std::fs::File because Stdio::from requires it.
    // This is a quick operation so blocking is acceptable here.
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let log_file_err = log_file.try_clone()?;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err));

    // Detach the process so it survives if the CLI exits
    #[cfg(unix)]
    {
        // Create new process group so server isn't killed when CLI exits
        // Note: tokio::process::Command provides process_group directly on Unix
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;

    info!(log_path = %log_path.display(), "Server logs will be written to file");
    debug!(pid = ?child.id(), exe = ?exe, "Spawned server process");
    Ok(())
}

/// Wait for the server to become ready by polling /readyz.
async fn wait_for_ready(client: &AgentClient, timeout_secs: u64, log_path: &Path) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut interval_ms = INITIAL_POLL_INTERVAL_MS;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(LauncherError::StartupTimeout {
                timeout_secs,
                log_path: log_path.to_path_buf(),
            });
        }

        match client.health().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                debug!(error = %e, interval_ms = interval_ms, "Server not ready yet, retrying");
            }
        }

        sleep(Duration::from_millis(interval_ms)).await;

        // Exponential backoff with cap
        interval_ms = (interval_ms * 2).min(MAX_POLL_INTERVAL_MS);
    }
}

fn server_log_path(config_path: &Path, port: u16) -> PathBuf {
    let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    base_dir
        .join(".duragent")
        .join("logs")
        .join(format!("server-{}.log", port))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_launcher_error_display() {
        let err = LauncherError::StartupTimeout {
            timeout_secs: 10,
            log_path: PathBuf::from("/tmp/duragent-server.log"),
        };
        assert!(err.to_string().contains("10s"));
        assert!(err.to_string().contains("server failed to start"));

        let err = LauncherError::HealthCheckFailed("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }

    #[tokio::test]
    async fn test_ensure_server_running_with_explicit_url() {
        // When explicit URL is provided, should return client without checking health
        let config = Config::default();
        let result = ensure_server_running(LaunchOptions {
            server_url: Some("http://example.com:8080"),
            config_path: Path::new("duragent.yaml"),
            config: &config,
            agents_dir: None,
        })
        .await;

        // Should succeed (just creates client, doesn't verify connection)
        assert!(result.is_ok());
    }
}
