//! Background process management for long-running external commands.
//!
//! Provides infrastructure for spawning, monitoring, and interacting with
//! background processes. Supports optional tmux integration for human
//! observation and agent interaction.

pub mod backend;
pub mod monitor;
pub mod registry;
pub mod tmux;
pub mod watcher;

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::gateway::GatewaySender;
use crate::process::backend::ProcessBackends;
use crate::scheduler::SchedulerHandle;
use crate::server::RuntimeServices;
use crate::sync::KeyedLocks;

// ============================================================================
// ProcessRegistryHandle (primary public type)
// ============================================================================

/// Handle to the global process registry.
///
/// Uses `DashMap` for concurrent access (same pattern as session registry).
/// No mpsc service loop needed — processes are independent.
#[derive(Clone)]
pub struct ProcessRegistryHandle {
    pub(crate) entries: Arc<DashMap<String, ProcessEntry>>,
    pub(crate) processes_dir: PathBuf,
    pub(crate) services: RuntimeServices,
    pub(crate) gateway_sender: GatewaySender,
    /// Per-process lock for serializing stdin writes.
    pub(crate) stdin_locks: KeyedLocks,
    /// Backend implementations for process management.
    pub(crate) backends: Arc<ProcessBackends>,
    /// Scheduler handle for auto-cancelling linked schedules on process exit.
    pub(crate) scheduler: Option<SchedulerHandle>,
    /// Queue for callback tasks (completion + screen-halted).
    pub(crate) callback_tx: mpsc::Sender<CallbackTask>,
}

// ============================================================================
// Result types (returned to callers)
// ============================================================================

/// Result from spawning a process (async mode).
#[derive(Debug, Clone, Serialize)]
pub struct SpawnResult {
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Result from a synchronous (`wait: true`) spawn.
#[derive(Debug, Clone, Serialize)]
pub struct WaitResult {
    pub status: String,
    pub exit_code: i32,
    pub output: String,
    pub duration_seconds: u64,
}

// ============================================================================
// ProcessError
// ============================================================================

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("process not found: {0}")]
    NotFound(String),

    #[error("process belongs to a different session")]
    WrongSession,

    #[error("process is not running")]
    NotRunning,

    #[error("tmux is not available")]
    TmuxUnavailable,

    #[error("process does not have a tmux session")]
    NotTmuxProcess,

    #[error("process does not have stdin")]
    NoStdin,

    #[error("stdin write timed out")]
    StdinTimeout,

    #[error("spawn failed: {0}")]
    SpawnFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ============================================================================
// ProcessStatus
// ============================================================================

/// Status of a background process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Completed { exit_code: i32 },
    Failed { exit_code: i32 },
    Lost,
    TimedOut,
    Killed,
}

// ============================================================================
// ProcessBackendKind
// ============================================================================

/// Backend type for a process.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessBackendKind {
    #[default]
    Local,
    Tmux,
}

impl ProcessStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, ProcessStatus::Running)
    }
}

impl std::fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessStatus::Running => write!(f, "running"),
            ProcessStatus::Completed { exit_code } => write!(f, "completed (exit {})", exit_code),
            ProcessStatus::Failed { exit_code } => write!(f, "failed (exit {})", exit_code),
            ProcessStatus::Lost => write!(f, "lost"),
            ProcessStatus::TimedOut => write!(f, "timed out"),
            ProcessStatus::Killed => write!(f, "killed"),
        }
    }
}

// ============================================================================
// ProcessMeta (persisted to .meta.json)
// ============================================================================

/// Metadata for a background process, persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMeta {
    pub handle: String,
    pub command: String,
    pub label: Option<String>,
    pub workdir: Option<String>,
    pub pid: Option<u32>,
    pub session_id: String,
    pub agent: String,
    pub tmux_session: Option<String>,
    #[serde(default)]
    pub backend: ProcessBackendKind,
    pub status: ProcessStatus,
    pub log_path: PathBuf,
    pub spawned_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub timeout_seconds: u64,
    /// Gateway to send completion callback to.
    pub gateway: Option<String>,
    /// Chat ID to send completion callback to.
    pub chat_id: Option<String>,
}

// ============================================================================
// ProcessEntry (in-memory, includes cancel handle)
// ============================================================================

/// In-memory entry for a tracked process.
pub struct ProcessEntry {
    pub meta: ProcessMeta,
    /// Cancel sender for the monitor task. None if process already completed.
    pub cancel_tx: Option<oneshot::Sender<()>>,
    /// Stdin handle for non-tmux processes. None if tmux or already closed.
    pub stdin: Option<tokio::process::ChildStdin>,
    /// Cancel sender for the screen watcher. None if no watcher is active.
    pub watcher_cancel_tx: Option<oneshot::Sender<()>>,
}

// ============================================================================
// Callback task queue
// ============================================================================

#[derive(Debug)]
pub(crate) enum CallbackTask {
    Completion { handle_id: String },
    ScreenHalted { handle_id: String },
}
