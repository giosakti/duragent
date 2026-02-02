//! Session registry for managing actor lifecycles.
//!
//! The registry is responsible for:
//! - Creating new session actors
//! - Looking up existing sessions
//! - Recovering sessions from disk on startup
//! - Graceful shutdown of all actors

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use tokio::fs;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::agent::OnDisconnect;
use crate::api::{SESSION_ID_PREFIX, SessionStatus};

use super::actor::{ActorConfig, ActorError, RecoverConfig, SessionActor, SessionMetadata};
use super::handle::SessionHandle;
use super::resume::resume_session;

// ============================================================================
// Configuration Constants
// ============================================================================

/// Maximum concurrent metadata fetches for `list()`.
const LIST_CONCURRENCY: usize = 32;

// ============================================================================
// Recovery Result
// ============================================================================

/// Result of session recovery on startup.
#[derive(Debug, Default)]
pub struct RecoveryResult {
    /// Number of sessions successfully recovered.
    pub recovered: usize,
    /// Number of sessions skipped (completed or invalid).
    pub skipped: usize,
    /// Errors encountered during recovery (session_id, error message).
    pub errors: Vec<(String, String)>,
}

// ============================================================================
// Session Registry
// ============================================================================

/// Registry for session actors.
///
/// Manages the lifecycle of session actors: creation, lookup, recovery, and shutdown.
/// Thread-safe and cheap to clone.
#[derive(Clone)]
pub struct SessionRegistry {
    /// Session handles by ID.
    handles: Arc<DashMap<String, SessionHandle>>,
    /// Actor task handles for graceful shutdown.
    task_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Path to sessions directory.
    sessions_path: PathBuf,
    /// Shutdown signal sender.
    shutdown_tx: Arc<watch::Sender<bool>>,
    /// Shutdown signal receiver (cloned for each actor).
    shutdown_rx: watch::Receiver<bool>,
}

impl SessionRegistry {
    /// Create a new session registry.
    pub fn new(sessions_path: PathBuf) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            handles: Arc::new(DashMap::new()),
            task_handles: Arc::new(Mutex::new(Vec::new())),
            sessions_path,
            shutdown_tx: Arc::new(shutdown_tx),
            shutdown_rx,
        }
    }

    /// Create a new session.
    ///
    /// Spawns a new actor and makes it immediately visible in the registry.
    /// Then waits for the initial snapshot to be persisted for crash safety.
    /// If persistence fails, the session is removed and the actor is stopped.
    pub async fn create(
        &self,
        agent: &str,
        on_disconnect: OnDisconnect,
        gateway: Option<String>,
        gateway_chat_id: Option<String>,
    ) -> Result<SessionHandle, ActorError> {
        let id = format!("{}{}", SESSION_ID_PREFIX, Ulid::new());

        let config = ActorConfig {
            id: id.clone(),
            agent: agent.to_string(),
            sessions_path: self.sessions_path.clone(),
            on_disconnect,
            gateway,
            gateway_chat_id,
        };

        let (tx, task_handle) = SessionActor::spawn(config, self.shutdown_rx.clone());
        let handle = SessionHandle::new(tx, id.clone(), agent.to_string());

        // Insert into registry FIRST - makes session visible immediately for concurrent lookups.
        // The actor is already running and can accept commands.
        self.handles.insert(id.clone(), handle.clone());

        // Wait for SessionStart + initial snapshot to be persisted (crash safety).
        // If this fails, remove the session from the registry and stop the actor.
        if let Err(e) = handle.force_snapshot().await {
            warn!(
                session_id = %id,
                error = %e,
                "Failed to persist session initialization, rolling back"
            );
            self.handles.remove(&id);
            drop(handle);
            task_handle.abort();
            return Err(e);
        }

        // Store the task handle for graceful shutdown after durability is confirmed
        self.task_handles.lock().await.push(task_handle);

        Ok(handle)
    }

    /// Get a session handle by ID.
    pub fn get(&self, id: &str) -> Option<SessionHandle> {
        self.handles.get(id).map(|r| r.clone())
    }

    /// Check if a session exists.
    pub fn contains(&self, id: &str) -> bool {
        self.handles.contains_key(id)
    }

    /// List all sessions.
    ///
    /// Returns metadata for all active sessions. Fetches metadata in parallel
    /// to avoid O(n) sequential latency with many sessions.
    pub async fn list(&self) -> Vec<SessionMetadata> {
        // Collect handles first to avoid holding DashMap references across await
        let handles: Vec<_> = self
            .handles
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        // Fetch metadata in parallel with bounded concurrency
        stream::iter(handles)
            .map(|handle| async move { handle.get_metadata().await })
            .buffer_unordered(LIST_CONCURRENCY)
            .filter_map(|result| async move { result.ok() })
            .collect()
            .await
    }

    /// Get the number of active sessions.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Recover sessions from disk on startup.
    ///
    /// Scans the sessions directory and spawns actors for recoverable sessions.
    pub async fn recover(&self) -> Result<RecoveryResult, ActorError> {
        let mut result = RecoveryResult::default();

        // Check if sessions directory exists
        if fs::metadata(&self.sessions_path).await.is_err() {
            debug!(
                sessions_dir = %self.sessions_path.display(),
                "Sessions directory does not exist, nothing to recover"
            );
            return Ok(result);
        }

        // Read the sessions directory
        let mut entries = fs::read_dir(&self.sessions_path).await.map_err(|e| {
            ActorError::Persistence(format!("Failed to read sessions directory: {}", e))
        })?;

        // Process each entry
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            // Skip non-directories
            if !path.is_dir() {
                continue;
            }

            // Get the session ID from the directory name
            let session_id = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            // Try to recover this session
            match self.recover_single_session(&session_id).await {
                Ok(true) => {
                    result.recovered += 1;
                }
                Ok(false) => {
                    result.skipped += 1;
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to recover session"
                    );
                    result.errors.push((session_id, e.to_string()));
                }
            }
        }

        if result.recovered > 0 || result.skipped > 0 || !result.errors.is_empty() {
            info!(
                recovered = result.recovered,
                skipped = result.skipped,
                errors = result.errors.len(),
                "Session recovery complete"
            );
        }

        Ok(result)
    }

    /// Recover a single session from disk.
    async fn recover_single_session(&self, session_id: &str) -> Result<bool, ActorError> {
        // Load and replay events to get current state
        let resumed = match resume_session(&self.sessions_path, session_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                debug!(
                    session_id = %session_id,
                    "No snapshot found, skipping"
                );
                return Ok(false);
            }
            Err(e) => {
                return Err(ActorError::Persistence(format!(
                    "Failed to resume session: {}",
                    e
                )));
            }
        };

        // Determine the status to recover with
        let status = match resumed.status {
            SessionStatus::Active => SessionStatus::Active,
            SessionStatus::Paused => SessionStatus::Paused,
            SessionStatus::Running => {
                // Running sessions were interrupted
                match resumed.config.on_disconnect {
                    OnDisconnect::Continue => {
                        debug!(
                            session_id = %session_id,
                            "Recovering interrupted background session as Active"
                        );
                        SessionStatus::Active
                    }
                    OnDisconnect::Pause => {
                        debug!(
                            session_id = %session_id,
                            "Recovering Running session with pause mode as Paused"
                        );
                        SessionStatus::Paused
                    }
                }
            }
            SessionStatus::Completed => {
                debug!(
                    session_id = %session_id,
                    "Skipping completed session"
                );
                return Ok(false);
            }
        };

        // Build snapshot for actor recovery
        let snapshot = super::snapshot::SessionSnapshot::new(
            resumed.session_id.clone(),
            resumed.agent.clone(),
            status,
            resumed.created_at,
            resumed.last_event_seq,
            resumed.conversation,
            resumed.config,
        );

        let config = RecoverConfig {
            snapshot,
            sessions_path: self.sessions_path.clone(),
        };

        let (tx, task_handle) = SessionActor::spawn_recovered(config, self.shutdown_rx.clone());
        let handle = SessionHandle::new(tx, resumed.session_id.clone(), resumed.agent.clone());

        // Store the task handle for graceful shutdown
        self.task_handles.lock().await.push(task_handle);

        self.handles.insert(resumed.session_id.clone(), handle);

        info!(
            session_id = %resumed.session_id,
            status = %status,
            "Recovered session"
        );

        Ok(true)
    }

    /// Register an existing handle (for testing or special cases).
    pub fn register(&self, handle: SessionHandle) {
        self.handles.insert(handle.id().to_string(), handle);
    }

    /// Gracefully shutdown all session actors.
    ///
    /// Sends shutdown signal and waits for all actors to complete.
    pub async fn shutdown(&self) {
        info!("Shutting down session registry");

        // Send shutdown signal to all actors
        if self.shutdown_tx.send(true).is_err() {
            warn!("Failed to send shutdown signal");
            return;
        }

        // Take all task handles and wait for them to complete
        let task_handles = {
            let mut handles = self.task_handles.lock().await;
            std::mem::take(&mut *handles)
        };

        // Wait for all actors to finish (they flush and snapshot on shutdown signal)
        for task_handle in task_handles {
            if let Err(e) = task_handle.await {
                warn!(error = ?e, "Actor task panicked during shutdown");
            }
        }

        info!("Session registry shutdown complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Message, Role};
    use crate::session::{SessionConfig, SessionSnapshot, write_snapshot};
    use chrono::Utc;
    use tempfile::TempDir;

    fn create_test_registry(temp_dir: &TempDir) -> SessionRegistry {
        SessionRegistry::new(temp_dir.path().to_path_buf())
    }

    async fn write_test_snapshot(
        temp_dir: &TempDir,
        session_id: &str,
        status: SessionStatus,
        on_disconnect: OnDisconnect,
    ) {
        let snapshot = SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            status,
            Utc::now(),
            1,
            vec![
                Message::text(Role::User, "Hello"),
                Message::text(Role::Assistant, "Hi there!"),
            ],
            SessionConfig {
                on_disconnect,
                ..Default::default()
            },
        );
        write_snapshot(temp_dir.path(), session_id, &snapshot)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_session_returns_handle() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        let handle = registry
            .create("test-agent", OnDisconnect::Pause, None, None)
            .await
            .unwrap();

        assert!(handle.id().starts_with("session_"));
        assert_eq!(handle.agent(), "test-agent");

        // Handle should be in registry
        assert!(registry.get(handle.id()).is_some());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_session() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        assert!(registry.get("session_unknown").is_none());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn list_returns_all_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        registry
            .create("agent1", OnDisconnect::Pause, None, None)
            .await
            .unwrap();
        registry
            .create("agent2", OnDisconnect::Continue, None, None)
            .await
            .unwrap();

        let sessions = registry.list().await;
        assert_eq!(sessions.len(), 2);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        let result = registry.recover().await.unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_active_session() {
        let temp_dir = TempDir::new().unwrap();

        write_test_snapshot(
            &temp_dir,
            "session_active",
            SessionStatus::Active,
            OnDisconnect::Pause,
        )
        .await;

        let registry = create_test_registry(&temp_dir);
        let result = registry.recover().await.unwrap();

        assert_eq!(result.recovered, 1);
        assert_eq!(result.skipped, 0);

        // Session should be in registry
        let handle = registry.get("session_active").unwrap();
        let metadata = handle.get_metadata().await.unwrap();
        assert_eq!(metadata.status, SessionStatus::Active);
        assert_eq!(metadata.agent, "test-agent");

        // Messages should be recovered
        let messages = handle.get_messages().await.unwrap();
        assert_eq!(messages.len(), 2);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_skips_completed_session() {
        let temp_dir = TempDir::new().unwrap();

        write_test_snapshot(
            &temp_dir,
            "session_completed",
            SessionStatus::Completed,
            OnDisconnect::Pause,
        )
        .await;

        let registry = create_test_registry(&temp_dir);
        let result = registry.recover().await.unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 1);

        // Session should not be in registry
        assert!(registry.get("session_completed").is_none());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_running_session_continue_mode() {
        let temp_dir = TempDir::new().unwrap();

        write_test_snapshot(
            &temp_dir,
            "session_running",
            SessionStatus::Running,
            OnDisconnect::Continue,
        )
        .await;

        let registry = create_test_registry(&temp_dir);
        let result = registry.recover().await.unwrap();

        assert_eq!(result.recovered, 1);

        let handle = registry.get("session_running").unwrap();
        let metadata = handle.get_metadata().await.unwrap();
        assert_eq!(metadata.status, SessionStatus::Active);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_flushes_all_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        let handle = registry
            .create("test-agent", OnDisconnect::Pause, None, None)
            .await
            .unwrap();
        let session_id = handle.id().to_string();

        // Add a message
        handle
            .add_user_message("Test message".to_string())
            .await
            .unwrap();

        // Shutdown
        registry.shutdown().await;

        // Verify snapshot was written
        let snapshot_file = temp_dir.path().join(&session_id).join("state.yaml");
        assert!(snapshot_file.exists());
    }

    #[tokio::test]
    async fn len_and_is_empty() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry
            .create("agent1", OnDisconnect::Pause, None, None)
            .await
            .unwrap();

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn contains_session() {
        let temp_dir = TempDir::new().unwrap();
        let registry = create_test_registry(&temp_dir);

        let handle = registry
            .create("test-agent", OnDisconnect::Pause, None, None)
            .await
            .unwrap();

        assert!(registry.contains(handle.id()));
        assert!(!registry.contains("session_unknown"));

        registry.shutdown().await;
    }
}
