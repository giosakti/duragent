//! Per-process exit monitors.
//!
//! Each spawned process gets a dedicated tokio task that waits for exit
//! (or timeout/cancel) and updates the registry.

use std::time::Duration;

use tokio::process::Child;
use tokio::sync::oneshot;
use tracing::{debug, error, warn};

use super::ProcessRegistryHandle;
use super::tmux;

/// Spawn a monitor for a non-tmux child process.
///
/// Waits for the child to exit, timeout, or cancel signal.
pub fn spawn_child_monitor(
    handle_id: String,
    mut child: Child,
    timeout_secs: u64,
    registry: ProcessRegistryHandle,
    cancel_rx: oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let timeout = Duration::from_secs(timeout_secs);

        tokio::select! {
            result = child.wait() => {
                match result {
                    Ok(status) => {
                        let code = status.code().unwrap_or(-1);
                        if code == 0 {
                            registry.mark_completed(&handle_id, code).await;
                        } else {
                            registry.mark_failed(&handle_id, code).await;
                        }
                    }
                    Err(e) => {
                        error!(handle = %handle_id, error = %e, "Failed to wait on child");
                        registry.mark_lost(&handle_id).await;
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                warn!(handle = %handle_id, timeout_secs, "Process timed out");
                graceful_kill(&mut child).await;
                registry.mark_timed_out(&handle_id).await;
            }
            _ = cancel_rx => {
                debug!(handle = %handle_id, "Process cancelled");
                graceful_kill(&mut child).await;
                registry.mark_killed(&handle_id).await;
            }
        }

        registry.fire_completion_callback(&handle_id).await;
    });
}

/// Spawn a monitor for a tmux-managed process.
///
/// Polls `tmux has-session` every 2 seconds to detect exit.
pub fn spawn_tmux_monitor(
    handle_id: String,
    tmux_session: String,
    log_path: std::path::PathBuf,
    timeout_secs: u64,
    registry: ProcessRegistryHandle,
    cancel_rx: oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let timeout = Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_secs(2);

        tokio::select! {
            _ = async {
                loop {
                    tokio::time::sleep(poll_interval).await;
                    if !tmux::has_session(&tmux_session).await {
                        break;
                    }
                }
            } => {
                // tmux session ended — read exit code from log
                let exit_code = read_exit_code_from_log(&log_path).await;
                match exit_code {
                    Some(0) => registry.mark_completed(&handle_id, 0).await,
                    Some(code) => registry.mark_failed(&handle_id, code).await,
                    None => registry.mark_completed(&handle_id, 0).await, // assume success if no marker
                }
            }
            _ = tokio::time::sleep(timeout) => {
                warn!(handle = %handle_id, timeout_secs, "tmux process timed out");
                let _ = tmux::kill_session(&tmux_session).await;
                registry.mark_timed_out(&handle_id).await;
            }
            _ = cancel_rx => {
                debug!(handle = %handle_id, "tmux process cancelled");
                let _ = tmux::kill_session(&tmux_session).await;
                registry.mark_killed(&handle_id).await;
            }
        }

        registry.fire_completion_callback(&handle_id).await;
    });
}

/// SIGTERM, wait 5s, SIGKILL.
pub(super) async fn graceful_kill(child: &mut Child) {
    if let Some(pid) = child.id() {
        // SAFETY: libc::kill with a valid pid from Child::id() is safe.
        // The pid comes from the kernel and is guaranteed to be valid
        // while the Child handle exists.
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }

        // Wait up to 5 seconds for graceful exit
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(_) => return,
            Err(_) => {
                debug!(pid, "Process didn't exit after SIGTERM, sending SIGKILL");
            }
        }
    }

    // Force kill
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Read exit code from log file's EXIT_CODE marker.
async fn read_exit_code_from_log(log_path: &std::path::Path) -> Option<i32> {
    let content = tokio::fs::read_to_string(log_path).await.ok()?;
    tmux::parse_exit_code_from_log(&content)
}
