//! Process registry implementation.
//!
//! Manages the lifecycle of background processes: spawning, monitoring,
//! completion callbacks, crash recovery, and cleanup.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::context::{ContextBuilder, TokenBudget, load_all_directives};
use crate::gateway::GatewaySender;
use crate::server::RuntimeServices;
use crate::session::{AgenticResult, run_agentic_loop};
use crate::tools::{ReloadDeps, ToolDependencies, build_executor};

use super::monitor;
use super::tmux;
use super::{ProcessEntry, ProcessError, ProcessMeta, ProcessRegistryHandle, ProcessStatus};
use super::{SpawnResult, WaitResult};

/// Maximum chars of log output to include in completion callbacks.
const COMPLETION_LOG_TAIL: usize = 2000;

/// Default cleanup age for completed processes (30 minutes).
const DEFAULT_CLEANUP_AGE_SECS: u64 = 30 * 60;

// ============================================================================
// Public types
// ============================================================================

/// Configuration for spawning a process.
pub struct SpawnConfig<'a> {
    pub command: &'a str,
    pub workdir: Option<&'a str>,
    pub wait: bool,
    pub interactive: bool,
    pub label: Option<&'a str>,
    pub timeout_seconds: u64,
    pub session_id: &'a str,
    pub agent: &'a str,
    pub gateway: Option<&'a str>,
    pub chat_id: Option<&'a str>,
}

/// Result enum for spawn operations.
pub enum SpawnOrWait {
    /// Process was spawned asynchronously.
    Spawned(SpawnResult),
    /// Process was waited on synchronously.
    Waited(WaitResult),
}

/// Spawn a periodic cleanup task. Returns its handle for shutdown.
pub fn spawn_cleanup_task(registry: ProcessRegistryHandle) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5 * 60)); // Every 5 min
        loop {
            interval.tick().await;
            registry.cleanup_old(None).await;
        }
    })
}

// ============================================================================
// ProcessRegistryHandle — constructor & spawn
// ============================================================================

impl ProcessRegistryHandle {
    /// Create a new process registry handle.
    ///
    /// Detects tmux availability at construction time.
    pub async fn new(
        processes_dir: PathBuf,
        services: RuntimeServices,
        gateway_sender: GatewaySender,
    ) -> Self {
        tokio::fs::create_dir_all(&processes_dir)
            .await
            .unwrap_or_else(|e| warn!(error = %e, "Failed to create processes dir"));

        let tmux_available = tmux::detect_tmux().await;
        if tmux_available {
            info!("tmux detected, background processes can use tmux sessions");
        } else {
            info!("tmux not available, background processes will use plain subprocesses");
        }

        Self {
            entries: Arc::new(DashMap::new()),
            processes_dir,
            tmux_available,
            services,
            gateway_sender,
            stdin_locks: crate::sync::KeyedLocks::new(),
        }
    }

    /// Spawn a new background process.
    pub async fn spawn(&self, cfg: SpawnConfig<'_>) -> Result<SpawnOrWait, ProcessError> {
        let command = cfg.command;
        let workdir = cfg.workdir;
        let wait = cfg.wait;
        let use_tmux = cfg.interactive;
        let label = cfg.label;
        let timeout_seconds = cfg.timeout_seconds;
        let session_id = cfg.session_id;
        let agent = cfg.agent;
        let gateway = cfg.gateway;
        let chat_id = cfg.chat_id;
        let handle_id = generate_handle_id();
        let log_path = self.processes_dir.join(format!("{}.log", handle_id));
        let tmux_session_name = format!("duragent-{}", handle_id);

        let use_tmux = use_tmux && self.tmux_available;

        let mut meta = ProcessMeta {
            handle: handle_id.clone(),
            command: command.to_string(),
            label: label.map(|s| s.to_string()),
            workdir: workdir.map(|s| s.to_string()),
            pid: None,
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            tmux_session: if use_tmux {
                Some(tmux_session_name.clone())
            } else {
                None
            },
            status: ProcessStatus::Running,
            log_path: log_path.clone(),
            spawned_at: Utc::now(),
            completed_at: None,
            timeout_seconds,
            gateway: gateway.map(|s| s.to_string()),
            chat_id: chat_id.map(|s| s.to_string()),
        };

        if use_tmux {
            // Spawn via tmux
            tmux::create_session(&tmux_session_name, command, &log_path, workdir)
                .await
                .map_err(|e| ProcessError::SpawnFailed(e.to_string()))?;

            self.persist_meta(&meta).await;

            if wait {
                let entry = ProcessEntry {
                    meta: meta.clone(),
                    cancel_tx: None,
                    stdin: None,
                };
                self.entries.insert(handle_id.clone(), entry);

                let result = self
                    .wait_for_tmux(&handle_id, &tmux_session_name, &log_path, timeout_seconds)
                    .await;
                return Ok(SpawnOrWait::Waited(result));
            }

            let (cancel_tx, cancel_rx) = oneshot::channel();
            let entry = ProcessEntry {
                meta: meta.clone(),
                cancel_tx: Some(cancel_tx),
                stdin: None,
            };
            self.entries.insert(handle_id.clone(), entry);

            // Start background monitor
            monitor::spawn_tmux_monitor(
                handle_id.clone(),
                tmux_session_name.clone(),
                log_path,
                timeout_seconds,
                self.clone(),
                cancel_rx,
            );

            Ok(SpawnOrWait::Spawned(SpawnResult {
                handle: handle_id,
                tmux_session: Some(tmux_session_name),
                status: "running".to_string(),
                pid: None,
            }))
        } else {
            // Spawn plain subprocess
            let mut cmd = tokio::process::Command::new("bash");
            cmd.args(["-c", command]);

            if let Some(dir) = workdir {
                cmd.current_dir(dir);
            }

            // Pipe stdout/stderr to log file, keep stdin for write action
            let log_file = std::fs::File::create(&log_path)
                .map_err(|e| ProcessError::SpawnFailed(format!("failed to create log: {}", e)))?;
            let log_file_err = log_file
                .try_clone()
                .map_err(|e| ProcessError::SpawnFailed(format!("failed to clone log fd: {}", e)))?;

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

            meta.pid = child.id();
            self.persist_meta(&meta).await;

            let stdin = child.stdin.take();

            if wait {
                let entry = ProcessEntry {
                    meta: meta.clone(),
                    cancel_tx: None,
                    stdin,
                };
                self.entries.insert(handle_id.clone(), entry);

                let result = self
                    .wait_for_child(child, &handle_id, &log_path, timeout_seconds)
                    .await;
                return Ok(SpawnOrWait::Waited(result));
            }

            let pid = meta.pid;
            let (cancel_tx, cancel_rx) = oneshot::channel();

            let entry = ProcessEntry {
                meta: meta.clone(),
                cancel_tx: Some(cancel_tx),
                stdin,
            };
            self.entries.insert(handle_id.clone(), entry);

            // Start background monitor
            monitor::spawn_child_monitor(
                handle_id.clone(),
                child,
                timeout_seconds,
                self.clone(),
                cancel_rx,
            );

            Ok(SpawnOrWait::Spawned(SpawnResult {
                handle: handle_id,
                tmux_session: None,
                status: "running".to_string(),
                pid,
            }))
        }
    }

    // ========================================================================
    // Query methods (public)
    // ========================================================================

    /// List processes for a given session.
    pub fn list_by_session(&self, session_id: &str) -> Vec<ProcessMeta> {
        self.entries
            .iter()
            .filter(|e| e.value().meta.session_id == session_id)
            .map(|e| e.value().meta.clone())
            .collect()
    }

    /// Get status of a specific process.
    pub fn get_status(
        &self,
        handle_id: &str,
        session_id: &str,
    ) -> Result<ProcessMeta, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        Ok(entry.meta.clone())
    }

    /// Read log output for a process.
    pub async fn read_log(
        &self,
        handle_id: &str,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        let log_path = entry.meta.log_path.clone();
        drop(entry);

        let content = tokio::fs::read_to_string(&log_path)
            .await
            .unwrap_or_default();

        let selected: String = content
            .lines()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(selected)
    }

    /// Capture interactive (tmux) pane content.
    pub async fn capture(&self, handle_id: &str, session_id: &str) -> Result<String, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        let tmux_session = entry
            .meta
            .tmux_session
            .clone()
            .ok_or(ProcessError::NotTmuxProcess)?;
        drop(entry);

        tmux::capture_pane(&tmux_session)
            .await
            .map_err(ProcessError::Io)
    }

    /// Send keystrokes to an interactive (tmux) process.
    pub async fn send_keys(
        &self,
        handle_id: &str,
        session_id: &str,
        keys: &str,
        press_enter: bool,
    ) -> Result<(), ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        if entry.meta.status.is_terminal() {
            return Err(ProcessError::NotRunning);
        }
        let tmux_session = entry
            .meta
            .tmux_session
            .clone()
            .ok_or(ProcessError::NotTmuxProcess)?;
        drop(entry);

        tmux::send_keys(&tmux_session, keys, press_enter)
            .await
            .map_err(ProcessError::Io)
    }

    /// Write to process stdin (non-interactive processes).
    pub async fn write_stdin(
        &self,
        handle_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<(), ProcessError> {
        // Serialize concurrent writes to the same process stdin
        let lock = self.stdin_locks.get(handle_id);
        let _guard = lock.lock().await;

        // Take stdin out of the entry to avoid holding DashMap lock across await
        let mut stdin_handle = {
            let mut entry = self
                .entries
                .get_mut(handle_id)
                .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
            if entry.meta.session_id != session_id {
                return Err(ProcessError::WrongSession);
            }
            if entry.meta.status.is_terminal() {
                return Err(ProcessError::NotRunning);
            }
            entry.stdin.take().ok_or(ProcessError::NoStdin)?
            // DashMap RefMut dropped here
        };

        use tokio::io::AsyncWriteExt;
        let result = async {
            stdin_handle
                .write_all(input.as_bytes())
                .await
                .map_err(ProcessError::Io)?;
            stdin_handle.flush().await.map_err(ProcessError::Io)?;
            Ok::<(), ProcessError>(())
        }
        .await;

        // Return stdin handle to the entry
        if let Some(mut entry) = self.entries.get_mut(handle_id) {
            entry.stdin = Some(stdin_handle);
        }

        result
    }

    /// Kill a process.
    ///
    /// Sets status to `Killed` and persists immediately, then sends the cancel
    /// signal. The monitor's `cancel_rx` branch will call `mark_killed`, but
    /// `update_status` will no-op because the status is already terminal.
    pub async fn kill(&self, handle_id: &str, session_id: &str) -> Result<(), ProcessError> {
        // Atomically validate and take cancel_tx in a single get_mut block
        let cancel_tx = {
            let mut entry = self
                .entries
                .get_mut(handle_id)
                .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
            if entry.meta.session_id != session_id {
                return Err(ProcessError::WrongSession);
            }
            if entry.meta.status.is_terminal() {
                return Err(ProcessError::NotRunning);
            }
            entry.meta.status = ProcessStatus::Killed;
            entry.meta.completed_at = Some(Utc::now());
            entry.cancel_tx.take()
            // DashMap RefMut dropped here
        };

        // Send cancel signal outside the lock
        if let Some(tx) = cancel_tx {
            let _ = tx.send(());
        }

        // Persist updated meta
        if let Some(entry) = self.entries.get(handle_id) {
            self.persist_meta(&entry.meta).await;
        }

        Ok(())
    }

    // ========================================================================
    // Lifecycle (public)
    // ========================================================================

    /// Recover processes from disk after restart.
    ///
    /// Scans `.meta.json` files, re-adopts tmux sessions, marks others as lost.
    pub async fn recover(&self) {
        let mut entries = match tokio::fs::read_dir(&self.processes_dir).await {
            Ok(e) => e,
            Err(e) => {
                debug!(error = %e, "No processes dir to recover from");
                return;
            }
        };

        let mut recovered = 0u32;
        let mut lost = 0u32;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".meta.json"))
            {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => match serde_json::from_str::<ProcessMeta>(&content) {
                        Ok(mut meta) => {
                            if !meta.status.is_terminal() {
                                // Check if tmux session still exists
                                if let Some(ref tmux_name) = meta.tmux_session
                                    && tmux::has_session(tmux_name).await
                                {
                                    // Re-adopt: start a new monitor
                                    let (cancel_tx, cancel_rx) = oneshot::channel();
                                    let entry = ProcessEntry {
                                        meta: meta.clone(),
                                        cancel_tx: Some(cancel_tx),
                                        stdin: None,
                                    };
                                    self.entries.insert(meta.handle.clone(), entry);

                                    monitor::spawn_tmux_monitor(
                                        meta.handle.clone(),
                                        tmux_name.clone(),
                                        meta.log_path.clone(),
                                        meta.timeout_seconds,
                                        self.clone(),
                                        cancel_rx,
                                    );

                                    info!(
                                        handle = %meta.handle,
                                        tmux = %tmux_name,
                                        "Re-adopted tmux process"
                                    );
                                    recovered += 1;
                                    continue;
                                }

                                // Non-tmux or tmux session gone → mark as lost
                                meta.status = ProcessStatus::Lost;
                                meta.completed_at = Some(Utc::now());
                                self.persist_meta(&meta).await;
                                let entry = ProcessEntry {
                                    meta: meta.clone(),
                                    cancel_tx: None,
                                    stdin: None,
                                };
                                self.entries.insert(meta.handle.clone(), entry);
                                lost += 1;

                                // Fire completion callback for lost processes
                                self.fire_completion_callback(&meta.handle).await;
                            } else {
                                // Already terminal — just load into registry for queries
                                let entry = ProcessEntry {
                                    meta: meta.clone(),
                                    cancel_tx: None,
                                    stdin: None,
                                };
                                self.entries.insert(meta.handle.clone(), entry);
                            }
                        }
                        Err(e) => {
                            warn!(path = %path.display(), error = %e, "Failed to parse meta.json");
                        }
                    },
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to read meta.json");
                    }
                }
            }
        }

        if recovered > 0 || lost > 0 {
            info!(recovered, lost, "Process recovery complete");
        }
    }

    /// Remove old completed entries and their files.
    pub async fn cleanup_old(&self, max_age_secs: Option<u64>) {
        let max_age = Duration::from_secs(max_age_secs.unwrap_or(DEFAULT_CLEANUP_AGE_SECS));
        let now = Utc::now();
        let mut to_remove = Vec::new();

        for entry in self.entries.iter() {
            if entry.meta.status.is_terminal()
                && let Some(completed_at) = entry.meta.completed_at
            {
                let age = (now - completed_at).to_std().unwrap_or(Duration::ZERO);
                if age > max_age {
                    to_remove.push(entry.key().clone());
                }
            }
        }

        for handle_id in to_remove {
            if let Some((_, entry)) = self.entries.remove(&handle_id) {
                // Remove log file
                let _ = tokio::fs::remove_file(&entry.meta.log_path).await;
                // Remove meta file
                let meta_path = self
                    .processes_dir
                    .join(format!("{}.meta.json", entry.meta.handle));
                let _ = tokio::fs::remove_file(&meta_path).await;
                debug!(handle = %handle_id, "Cleaned up old process");
            }
        }
    }

    /// Send cancel signal to all active monitors.
    pub fn shutdown(&self) {
        let mut cancelled = 0u32;

        for mut entry in self.entries.iter_mut() {
            if let Some(tx) = entry.cancel_tx.take() {
                let _ = tx.send(());
                cancelled += 1;
            }
        }

        if cancelled > 0 {
            info!(cancelled, "Sent cancel to active process monitors");
        }
    }

    // ========================================================================
    // Status updates (pub(crate), called by monitors)
    // ========================================================================

    pub(crate) async fn mark_completed(&self, handle_id: &str, exit_code: i32) {
        self.update_status(handle_id, ProcessStatus::Completed { exit_code })
            .await;
    }

    pub(crate) async fn mark_failed(&self, handle_id: &str, exit_code: i32) {
        self.update_status(handle_id, ProcessStatus::Failed { exit_code })
            .await;
    }

    pub(crate) async fn mark_timed_out(&self, handle_id: &str) {
        self.update_status(handle_id, ProcessStatus::TimedOut).await;
    }

    pub(crate) async fn mark_killed(&self, handle_id: &str) {
        self.update_status(handle_id, ProcessStatus::Killed).await;
    }

    pub(crate) async fn mark_lost(&self, handle_id: &str) {
        self.update_status(handle_id, ProcessStatus::Lost).await;
    }

    /// Fire a completion callback for a finished process.
    ///
    /// Follows the same pattern as `scheduler/service.rs:execute_task_payload`:
    /// inject user message into session, build executor, run agentic loop,
    /// send response via gateway.
    pub(crate) async fn fire_completion_callback(&self, handle_id: &str) {
        let meta = match self.entries.get(handle_id) {
            Some(entry) => entry.meta.clone(),
            None => return,
        };

        // Only fire callback for async processes (not wait:true)
        // Check if we have gateway info to respond to
        let (gateway, chat_id) = match (&meta.gateway, &meta.chat_id) {
            (Some(gw), Some(cid)) => (gw.clone(), cid.clone()),
            _ => {
                debug!(handle = %handle_id, "No gateway/chat_id for callback, skipping");
                return;
            }
        };

        // Read tail of log for the completion message
        let log_tail = match tokio::fs::read_to_string(&meta.log_path).await {
            Ok(content) => {
                if content.len() > COMPLETION_LOG_TAIL {
                    let start = content.len().saturating_sub(COMPLETION_LOG_TAIL);
                    let start = content.floor_char_boundary(start);
                    format!("...\n{}", &content[start..])
                } else {
                    content
                }
            }
            Err(_) => "(log unavailable)".to_string(),
        };

        let duration = meta
            .completed_at
            .map(|c| (c - meta.spawned_at).num_seconds())
            .unwrap_or(0);

        let label_str = meta
            .label
            .as_deref()
            .map(|l| format!("\nLabel: {}", l))
            .unwrap_or_default();

        let completion_message = format!(
            "[Background Process Completed]\n\n\
             Handle: {}\
             {}\n\
             Command: {}\n\
             Status: {}\n\
             Duration: {}s\n\n\
             Output (last {} chars):\n{}",
            meta.handle,
            label_str,
            meta.command,
            meta.status,
            duration,
            COMPLETION_LOG_TAIL,
            log_tail,
        );

        if let Err(e) = self
            .run_callback(&meta, &gateway, &chat_id, &completion_message)
            .await
        {
            error!(
                handle = %handle_id,
                error = %e,
                "Failed to fire completion callback"
            );
        }
    }

    /// Persist process metadata to disk (atomic write via temp + rename).
    pub(crate) async fn persist_meta(&self, meta: &ProcessMeta) {
        let meta_path = self
            .processes_dir
            .join(format!("{}.meta.json", meta.handle));
        let tmp_path = self
            .processes_dir
            .join(format!("{}.meta.json.tmp", meta.handle));

        match serde_json::to_string_pretty(meta) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&tmp_path, json.as_bytes()).await {
                    error!(error = %e, "Failed to write process meta tmp");
                    return;
                }
                if let Err(e) = tokio::fs::rename(&tmp_path, &meta_path).await {
                    error!(error = %e, "Failed to rename process meta");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to serialize process meta");
            }
        }
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    async fn update_status(&self, handle_id: &str, status: ProcessStatus) {
        if let Some(mut entry) = self.entries.get_mut(handle_id) {
            // Skip if already finalized (e.g., kill() set Killed before monitor ran)
            if entry.meta.status.is_terminal() {
                return;
            }
            entry.meta.status = status;
            entry.meta.completed_at = Some(Utc::now());
            entry.cancel_tx = None;
            let meta = entry.meta.clone();
            drop(entry);
            self.persist_meta(&meta).await;
        }
    }

    async fn wait_for_child(
        &self,
        mut child: tokio::process::Child,
        handle_id: &str,
        log_path: &Path,
        timeout_secs: u64,
    ) -> WaitResult {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);

        let status = tokio::select! {
            result = child.wait() => {
                match result {
                    Ok(s) => s.code().unwrap_or(-1),
                    Err(e) => {
                        error!(handle = %handle_id, error = %e, "wait failed");
                        -1
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                warn!(handle = %handle_id, "sync wait timed out");
                monitor::graceful_kill(&mut child).await;
                -1
            }
        };

        let duration = start.elapsed().as_secs();
        let output = tokio::fs::read_to_string(log_path)
            .await
            .unwrap_or_default();

        // Update registry
        if status == 0 {
            self.mark_completed(handle_id, status).await;
        } else {
            self.mark_failed(handle_id, status).await;
        }

        let status_str = if status == 0 { "completed" } else { "failed" };

        WaitResult {
            status: status_str.to_string(),
            exit_code: status,
            output,
            duration_seconds: duration,
        }
    }

    async fn wait_for_tmux(
        &self,
        handle_id: &str,
        tmux_session: &str,
        log_path: &Path,
        timeout_secs: u64,
    ) -> WaitResult {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_secs(2);

        let timed_out = tokio::select! {
            _ = async {
                loop {
                    tokio::time::sleep(poll_interval).await;
                    if !tmux::has_session(tmux_session).await {
                        break;
                    }
                }
            } => false,
            _ = tokio::time::sleep(timeout) => {
                let _ = tmux::kill_session(tmux_session).await;
                true
            },
        };

        let duration = start.elapsed().as_secs();
        let output = tokio::fs::read_to_string(log_path)
            .await
            .unwrap_or_default();

        let exit_code = if timed_out {
            self.mark_timed_out(handle_id).await;
            -1
        } else {
            let code = tmux::parse_exit_code_from_log(&output).unwrap_or(0);
            if code == 0 {
                self.mark_completed(handle_id, code).await;
            } else {
                self.mark_failed(handle_id, code).await;
            }
            code
        };

        let status_str = if timed_out {
            "timed_out"
        } else if exit_code == 0 {
            "completed"
        } else {
            "failed"
        };

        WaitResult {
            status: status_str.to_string(),
            exit_code,
            output,
            duration_seconds: duration,
        }
    }

    /// Run the agentic loop for a completion callback.
    async fn run_callback(
        &self,
        meta: &ProcessMeta,
        gateway: &str,
        chat_id: &str,
        message: &str,
    ) -> anyhow::Result<()> {
        // Get agent
        let agent = self
            .services
            .agents
            .get(&meta.agent)
            .ok_or_else(|| anyhow::anyhow!("Agent not found: {}", meta.agent))?;

        // Get provider
        let provider = self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())
            .await
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", agent.model.provider))?;

        // Get session handle
        let handle = self
            .services
            .session_registry
            .get(&meta.session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", meta.session_id))?;

        // Persist the completion message
        handle.add_user_message(message.to_string()).await?;

        // Build tool executor
        let policy = self.services.policy_store.load(&meta.agent).await;
        let deps = ToolDependencies {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
            process_registry: None, // Don't allow nested process spawning from callbacks
            session_id: Some(meta.session_id.clone()),
            agent_name: Some(meta.agent.clone()),
        };
        let mut executor = build_executor(
            &agent,
            &meta.agent,
            &meta.session_id,
            policy,
            deps,
            &self.services.world_memory_path,
        )
        .with_reload_deps(ReloadDeps {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
            agent_tool_configs: agent.tools.clone(),
        });

        // Build messages
        let history = handle.get_messages().await.unwrap_or_default();
        let directives =
            load_all_directives(&self.services.workspace_directives_path, &agent.agent_dir);
        let budget = TokenBudget {
            max_input_tokens: agent.model.effective_max_input_tokens(),
            max_output_tokens: agent.model.max_output_tokens.unwrap_or(4096),
            max_history_tokens: agent.session.context.max_history_tokens,
        };
        let messages = ContextBuilder::new()
            .from_agent_spec(&agent)
            .with_messages(history)
            .with_directives(directives)
            .build()
            .render_with_budget(
                &agent.model.name,
                agent.model.temperature,
                agent.model.max_output_tokens,
                vec![],
                &budget,
            )
            .messages;

        // Run agentic loop
        let result =
            run_agentic_loop(provider, &mut executor, &agent, messages, &handle, None).await?;

        let response = match result {
            AgenticResult::Complete { content, usage, .. } => {
                let _ = handle.add_assistant_message(content.clone(), usage).await;
                content
            }
            AgenticResult::AwaitingApproval { pending, .. } => {
                format!(
                    "Background process completed but requires approval for: `{}`",
                    pending.command
                )
            }
        };

        // Send response via gateway
        self.gateway_sender
            .send_message(gateway, chat_id, &response, None)
            .await
            .map_err(|e| anyhow::anyhow!("Gateway send failed: {}", e))?;

        Ok(())
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Generate a unique process handle ID.
fn generate_handle_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_handle_id_is_ulid() {
        let id = generate_handle_id();
        assert_eq!(id.len(), 26); // ULID is 26 chars
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn process_meta_serde_roundtrip() {
        let meta = ProcessMeta {
            handle: "01hqxyz123abc".to_string(),
            command: "echo hello".to_string(),
            label: Some("test".to_string()),
            workdir: Some("/tmp".to_string()),
            pid: Some(1234),
            session_id: "sess-abc".to_string(),
            agent: "test-agent".to_string(),
            tmux_session: Some("duragent-01hqxyz123abc".to_string()),
            status: ProcessStatus::Running,
            log_path: PathBuf::from("/tmp/01hqxyz123abc.log"),
            spawned_at: Utc::now(),
            completed_at: None,
            timeout_seconds: 1800,
            gateway: Some("telegram".to_string()),
            chat_id: Some("123".to_string()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ProcessMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.handle, meta.handle);
        assert_eq!(parsed.command, meta.command);
        assert_eq!(parsed.label, meta.label);
        assert_eq!(parsed.pid, meta.pid);
        assert!(matches!(parsed.status, ProcessStatus::Running));
    }

    #[test]
    fn process_status_serde() {
        let status = ProcessStatus::Completed { exit_code: 0 };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("completed"));
        assert!(json.contains("\"exit_code\":0"));

        let parsed: ProcessStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ProcessStatus::Completed { exit_code: 0 });
    }

    #[test]
    fn process_status_is_terminal() {
        assert!(!ProcessStatus::Running.is_terminal());
        assert!(ProcessStatus::Completed { exit_code: 0 }.is_terminal());
        assert!(ProcessStatus::Failed { exit_code: 1 }.is_terminal());
        assert!(ProcessStatus::Lost.is_terminal());
        assert!(ProcessStatus::TimedOut.is_terminal());
        assert!(ProcessStatus::Killed.is_terminal());
    }
}
