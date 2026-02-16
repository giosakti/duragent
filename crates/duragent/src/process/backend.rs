//! Backend implementations for process management.
//!
//! Encapsulates how processes are spawned and interacted with across backends
//! (e.g. local subprocesses vs tmux sessions).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::{Child, ChildStdin, Command};

use super::registry::SpawnConfig;
use super::tmux;
use super::{ProcessBackendKind, ProcessError};

// ============================================================================
// Backend types
// ============================================================================

pub enum BackendSpawn {
    Local {
        child: Child,
        stdin: Option<ChildStdin>,
    },
    Tmux {
        tmux_session: String,
    },
}

#[async_trait]
pub trait ProcessBackend: Send + Sync {
    fn kind(&self) -> ProcessBackendKind;

    async fn spawn(
        &self,
        cfg: &SpawnConfig<'_>,
        handle_id: &str,
        log_path: &Path,
    ) -> Result<BackendSpawn, ProcessError>;

    async fn capture(&self, tmux_session: Option<&str>) -> Result<String, ProcessError>;

    async fn send_keys(
        &self,
        tmux_session: Option<&str>,
        keys: &str,
        press_enter: bool,
    ) -> Result<(), ProcessError>;
}

pub struct ProcessBackends {
    local: Arc<dyn ProcessBackend>,
    tmux: Option<Arc<dyn ProcessBackend>>,
}

impl ProcessBackends {
    pub fn new(tmux_available: bool) -> Self {
        let local: Arc<dyn ProcessBackend> = Arc::new(LocalBackend);
        let tmux = if tmux_available {
            Some(Arc::new(TmuxBackend) as Arc<dyn ProcessBackend>)
        } else {
            None
        };
        Self { local, tmux }
    }

    pub fn select(&self, interactive: bool) -> Arc<dyn ProcessBackend> {
        if interactive && let Some(tmux) = &self.tmux {
            return Arc::clone(tmux);
        }
        Arc::clone(&self.local)
    }

    pub fn get(&self, kind: ProcessBackendKind) -> Option<Arc<dyn ProcessBackend>> {
        match kind {
            ProcessBackendKind::Local => Some(Arc::clone(&self.local)),
            ProcessBackendKind::Tmux => self.tmux.as_ref().map(Arc::clone),
        }
    }
}

// ============================================================================
// Local backend (subprocess)
// ============================================================================

struct LocalBackend;

#[async_trait]
impl ProcessBackend for LocalBackend {
    fn kind(&self) -> ProcessBackendKind {
        ProcessBackendKind::Local
    }

    async fn spawn(
        &self,
        cfg: &SpawnConfig<'_>,
        _handle_id: &str,
        log_path: &Path,
    ) -> Result<BackendSpawn, ProcessError> {
        let mut cmd = Command::new("bash");
        cmd.args(["-c", cfg.command]);

        if let Some(dir) = cfg.workdir {
            cmd.current_dir(dir);
        }

        let (log_file, log_file_err) = tokio::task::spawn_blocking({
            let log_path = log_path.to_path_buf();
            move || -> Result<(std::fs::File, std::fs::File), ProcessError> {
                let log_file = std::fs::File::create(&log_path).map_err(|e| {
                    ProcessError::SpawnFailed(format!("failed to create log: {}", e))
                })?;
                let log_file_err = log_file.try_clone().map_err(|e| {
                    ProcessError::SpawnFailed(format!("failed to clone log fd: {}", e))
                })?;
                Ok((log_file, log_file_err))
            }
        })
        .await
        .map_err(|e| ProcessError::SpawnFailed(format!("log file setup join error: {}", e)))??;

        cmd.stdout(log_file);
        cmd.stderr(log_file_err);
        cmd.stdin(std::process::Stdio::piped());

        // SAFETY: pre_exec runs in the forked child before exec. PR_SET_PDEATHSIG
        // configures the child to receive SIGTERM when the parent dies. This is safe
        // because we're in the pre-exec callback with no shared mutable state.
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ProcessError::SpawnFailed(e.to_string()))?;

        let stdin = child.stdin.take();

        Ok(BackendSpawn::Local { child, stdin })
    }

    async fn capture(&self, _tmux_session: Option<&str>) -> Result<String, ProcessError> {
        Err(ProcessError::NotTmuxProcess)
    }

    async fn send_keys(
        &self,
        _tmux_session: Option<&str>,
        _keys: &str,
        _press_enter: bool,
    ) -> Result<(), ProcessError> {
        Err(ProcessError::NotTmuxProcess)
    }
}

// ============================================================================
// Tmux backend
// ============================================================================

struct TmuxBackend;

#[async_trait]
impl ProcessBackend for TmuxBackend {
    fn kind(&self) -> ProcessBackendKind {
        ProcessBackendKind::Tmux
    }

    async fn spawn(
        &self,
        cfg: &SpawnConfig<'_>,
        handle_id: &str,
        log_path: &Path,
    ) -> Result<BackendSpawn, ProcessError> {
        let tmux_session_name = format!("duragent-{}", handle_id);
        tmux::create_session(
            &tmux_session_name,
            cfg.command,
            log_path,
            cfg.workdir,
            cfg.interactive,
        )
        .await
        .map_err(|e| ProcessError::SpawnFailed(e.to_string()))?;

        Ok(BackendSpawn::Tmux {
            tmux_session: tmux_session_name,
        })
    }

    async fn capture(&self, tmux_session: Option<&str>) -> Result<String, ProcessError> {
        let tmux_session = tmux_session.ok_or(ProcessError::NotTmuxProcess)?;
        tmux::capture_pane(tmux_session)
            .await
            .map_err(ProcessError::Io)
    }

    async fn send_keys(
        &self,
        tmux_session: Option<&str>,
        keys: &str,
        press_enter: bool,
    ) -> Result<(), ProcessError> {
        let tmux_session = tmux_session.ok_or(ProcessError::NotTmuxProcess)?;
        tmux::send_keys(tmux_session, keys, press_enter)
            .await
            .map_err(ProcessError::Io)
    }
}
