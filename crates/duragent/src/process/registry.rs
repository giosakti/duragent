//! Process registry implementation.
//!
//! Manages the lifecycle of background processes: spawning, monitoring,
//! completion callbacks, crash recovery, and cleanup.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

use crate::api::SessionStatus;
use crate::context::{ContextBuilder, TokenBudget, load_all_directives};
use crate::gateway::{GatewaySender, build_approval_keyboard};
use crate::server::RuntimeServices;
use crate::session::{AgenticResult, STEERING_CHANNEL_CAPACITY, run_agentic_loop};
use crate::tools::{ReloadDeps, ToolDependencies, build_executor};

use super::backend::{BackendSpawn, ProcessBackends};
use super::monitor;
use super::tmux;
use super::{ProcessEntry, ProcessError, ProcessMeta, ProcessRegistryHandle, ProcessStatus};
use super::{SpawnResult, WaitResult};

mod callbacks;
mod io;
mod recovery;

pub use recovery::spawn_cleanup_task;

/// Maximum bytes of log output to include in completion callbacks.
const COMPLETION_LOG_TAIL_BYTES: usize = 2000;
/// Maximum bytes of log output to include in wait results.
const WAIT_LOG_TAIL_BYTES: usize = 8 * 1024;

/// Default cleanup age for completed processes (30 minutes).
const DEFAULT_CLEANUP_AGE_SECS: u64 = 30 * 60;
/// Default timeout for stdin writes to child processes (seconds).
const STDIN_WRITE_TIMEOUT_SECS: u64 = 10;
/// Maximum concurrent recovery callbacks for lost processes.
const RECOVERY_CALLBACK_CONCURRENCY: usize = 4;

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
        scheduler: Option<crate::scheduler::SchedulerHandle>,
    ) -> Self {
        tokio::fs::create_dir_all(&processes_dir)
            .await
            .unwrap_or_else(|e| warn!(error = %e, "Failed to create processes dir"));

        // Canonicalize so log paths are absolute — tmux sessions run in a
        // different cwd, so relative paths would resolve to the wrong place.
        let processes_dir = processes_dir.canonicalize().unwrap_or(processes_dir);

        let tmux_available = tmux::detect_tmux().await;
        if tmux_available {
            info!("tmux detected, background processes can use tmux sessions");
        } else {
            info!("tmux not available, background processes will use plain subprocesses");
        }

        Self {
            entries: Arc::new(DashMap::new()),
            processes_dir,
            services,
            gateway_sender,
            stdin_locks: crate::sync::KeyedLocks::new(),
            backends: Arc::new(ProcessBackends::new(tmux_available)),
            scheduler,
        }
    }

    /// Spawn a new background process.
    pub async fn spawn(&self, cfg: SpawnConfig<'_>) -> Result<SpawnOrWait, ProcessError> {
        let command = cfg.command;
        let workdir = cfg.workdir;
        let wait = cfg.wait;
        let label = cfg.label;
        let timeout_seconds = cfg.timeout_seconds;
        let session_id = cfg.session_id;
        let agent = cfg.agent;
        let gateway = cfg.gateway;
        let chat_id = cfg.chat_id;
        let handle_id = generate_handle_id();
        let log_path = self.processes_dir.join(format!("{}.log", handle_id));
        let backend = self.backends.select(cfg.interactive);
        let backend_kind = backend.kind();

        let mut meta = ProcessMeta {
            handle: handle_id.clone(),
            command: command.to_string(),
            label: label.map(|s| s.to_string()),
            workdir: workdir.map(|s| s.to_string()),
            pid: None,
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            tmux_session: None,
            backend: backend_kind,
            status: ProcessStatus::Running,
            log_path: log_path.clone(),
            spawned_at: Utc::now(),
            completed_at: None,
            timeout_seconds,
            gateway: gateway.map(|s| s.to_string()),
            chat_id: chat_id.map(|s| s.to_string()),
        };

        match backend.spawn(&cfg, &handle_id, &log_path).await? {
            BackendSpawn::Tmux { tmux_session } => {
                meta.tmux_session = Some(tmux_session.clone());
                self.persist_meta(&meta).await;

                if wait {
                    let entry = ProcessEntry {
                        meta: meta.clone(),
                        cancel_tx: None,
                        stdin: None,
                        watcher_cancel_tx: None,
                    };
                    self.entries.insert(handle_id.clone(), entry);

                    let result = self
                        .wait_for_tmux(&handle_id, &tmux_session, &log_path, timeout_seconds)
                        .await;
                    return Ok(SpawnOrWait::Waited(result));
                }

                let (cancel_tx, cancel_rx) = oneshot::channel();
                let entry = ProcessEntry {
                    meta: meta.clone(),
                    cancel_tx: Some(cancel_tx),
                    stdin: None,
                    watcher_cancel_tx: None,
                };
                self.entries.insert(handle_id.clone(), entry);

                // Start background monitor
                monitor::spawn_tmux_monitor(
                    handle_id.clone(),
                    tmux_session.clone(),
                    log_path,
                    timeout_seconds,
                    self.clone(),
                    cancel_rx,
                );

                Ok(SpawnOrWait::Spawned(SpawnResult {
                    handle: handle_id,
                    tmux_session: Some(tmux_session),
                    status: "running".to_string(),
                    pid: None,
                }))
            }
            BackendSpawn::Local { child, stdin } => {
                meta.pid = child.id();
                self.persist_meta(&meta).await;

                if wait {
                    let entry = ProcessEntry {
                        meta: meta.clone(),
                        cancel_tx: None,
                        stdin,
                        watcher_cancel_tx: None,
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
                    watcher_cancel_tx: None,
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
    }

    /// Kill a process.
    ///
    /// Sets status to `Killed` and persists immediately, then sends the cancel
    /// signal. The monitor's `cancel_rx` branch will call `mark_killed`, but
    /// `update_status` will no-op because the status is already terminal.
    pub async fn kill(&self, handle_id: &str, session_id: &str) -> Result<(), ProcessError> {
        // Atomically validate and take cancel senders in a single get_mut block
        let (cancel_tx, watcher_tx) = {
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
            (entry.cancel_tx.take(), entry.watcher_cancel_tx.take())
            // DashMap RefMut dropped here
        };

        // Send cancel signals outside the lock
        if let Some(tx) = cancel_tx {
            let _ = tx.send(());
        }
        if let Some(tx) = watcher_tx {
            let _ = tx.send(());
        }

        // Persist updated meta
        if let Some(entry) = self.entries.get(handle_id) {
            self.persist_meta(&entry.meta).await;
        }

        Ok(())
    }

    // ========================================================================
    // Screen watcher management
    // ========================================================================

    /// Start a stream-based screen watcher for an interactive tmux process.
    ///
    /// Monitors the process log for output flow. Fires a callback when
    /// output has been silent for `silence_timeout_secs` seconds.
    pub fn start_watcher(
        &self,
        handle_id: &str,
        session_id: &str,
        silence_timeout_secs: u64,
    ) -> Result<(), ProcessError> {
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
        let _tmux_session = entry
            .meta
            .tmux_session
            .clone()
            .ok_or(ProcessError::NotTmuxProcess)?;
        let log_path = entry.meta.log_path.clone();

        // Stop existing watcher if any
        if let Some(tx) = entry.watcher_cancel_tx.take() {
            let _ = tx.send(());
        }

        let (cancel_tx, cancel_rx) = oneshot::channel();
        entry.watcher_cancel_tx = Some(cancel_tx);

        // Drop entry ref before spawning (releases DashMap lock)
        drop(entry);

        super::watcher::spawn_screen_watcher(
            handle_id.to_string(),
            std::time::Duration::from_secs(2), // poll file size every 2s
            std::time::Duration::from_secs(silence_timeout_secs),
            self.clone(),
            cancel_rx,
            log_path,
        );

        Ok(())
    }

    /// Stop the screen watcher for a process.
    pub fn stop_watcher(&self, handle_id: &str, session_id: &str) -> Result<(), ProcessError> {
        let mut entry = self
            .entries
            .get_mut(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;

        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }

        if let Some(tx) = entry.watcher_cancel_tx.take() {
            let _ = tx.send(());
        }

        Ok(())
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
        let output = Self::read_log_tail(log_path, WAIT_LOG_TAIL_BYTES)
            .await
            .map(|(content, truncated)| {
                if truncated {
                    format!("...\n{}", content)
                } else {
                    content
                }
            })
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
        let output = Self::read_log_tail(log_path, WAIT_LOG_TAIL_BYTES)
            .await
            .map(|(content, truncated)| {
                if truncated {
                    format!("...\n{}", content)
                } else {
                    content
                }
            })
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

    /// Run the agentic loop for a process callback.
    ///
    /// When `process_registry` is `Some`, the agent will have access to
    /// process tools (needed for screen-halted callbacks where the agent
    /// must interact with the process). Completion callbacks pass `None`.
    /// `persist_message` skips re-persisting if the callback was already stored.
    async fn run_callback(
        &self,
        meta: &ProcessMeta,
        gateway: &str,
        chat_id: &str,
        message: &str,
        process_registry: Option<ProcessRegistryHandle>,
        persist_message: bool,
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

        // Persist the callback message (unless already persisted upstream)
        if persist_message {
            handle.add_user_message(message.to_string()).await?;
        }

        // Acquire per-session agentic loop lock (blocks until available).
        // We acquire BEFORE building context so we always have fresh state —
        // if another loop was running, we'll see its events in our context.
        let loop_lock = self.services.agentic_loop_locks.get(&meta.session_id);
        let _loop_guard = loop_lock.lock().await;

        // Build tool executor (after lock to pick up latest policy)
        let policy = self.services.policy_store.load(&meta.agent).await;
        let deps = ToolDependencies {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
            process_registry,
            session_id: Some(meta.session_id.clone()),
            agent_name: Some(meta.agent.clone()),
            session_registry: Some(self.services.session_registry.clone()),
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

        // Create steering channel so user messages can be injected mid-loop
        let (steering_tx, steering_rx) = mpsc::channel(STEERING_CHANNEL_CAPACITY);
        self.services
            .steering_channels
            .insert(meta.session_id.clone(), steering_tx);

        // Build messages (after lock — always reflects latest state)
        let history = handle.get_messages().await.unwrap_or_default();
        let directives = load_all_directives(
            &self.services.workspace_directives_path,
            &agent.agent_dir,
            &agent,
        );
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
        let result = run_agentic_loop(
            provider,
            &mut executor,
            &agent,
            messages,
            &handle,
            None,
            Some(steering_rx),
        )
        .await;

        self.services.steering_channels.remove(&meta.session_id);

        let result = result?;

        match result {
            AgenticResult::Complete { content, usage, .. } => {
                if !content.trim().is_empty() {
                    let _ = handle.add_assistant_message(content.clone(), usage).await;
                }

                // Send response via gateway (skip empty)
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    self.gateway_sender
                        .send_message(gateway, chat_id, trimmed, None)
                        .await
                        .map_err(|e| anyhow::anyhow!("Gateway send failed: {}", e))?;
                }
            }
            AgenticResult::AwaitingApproval {
                pending,
                partial_content,
                ..
            } => {
                // Persist pending approval and pause the session so it can be resumed
                if let Err(e) = handle.set_pending_approval(pending.clone()).await {
                    warn!(error = %e, "Failed to persist pending approval");
                }
                if let Err(e) = handle.set_status(SessionStatus::Paused).await {
                    warn!(error = %e, "Failed to set session status to Paused");
                }

                // Send any partial content before the tool call
                let trimmed = partial_content.trim();
                if !trimmed.is_empty()
                    && let Err(e) = self
                        .gateway_sender
                        .send_message(gateway, chat_id, trimmed, None)
                        .await
                {
                    warn!(error = %e, "Failed to send partial content");
                }

                // Send approval keyboard to let the user approve/deny
                let keyboard = build_approval_keyboard();
                let approval_msg =
                    format!("Command requires approval:\n```\n{}\n```", pending.command);

                if let Err(e) = self
                    .gateway_sender
                    .send_message_with_keyboard(
                        gateway,
                        chat_id,
                        &approval_msg,
                        None,
                        Some(keyboard),
                    )
                    .await
                {
                    warn!(error = %e, "Failed to send approval keyboard");
                }
            }
        }

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
    use crate::process::ProcessBackendKind;

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
            backend: ProcessBackendKind::Tmux,
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
