//! Session registry for managing actor lifecycles.
//!
//! The registry is responsible for:
//! - Creating new session actors
//! - Looking up existing sessions
//! - Recovering sessions from disk on startup
//! - Graceful shutdown of all actors

use std::sync::Arc;

use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::agent::OnDisconnect;
use crate::api::{SESSION_ID_PREFIX, SessionStatus};
use crate::config::CompactionMode;
use crate::store::SessionStore;

use super::actor::SessionActor;
use super::actor_types::{
    ActorConfig, ActorError, DEFAULT_ACTOR_MESSAGE_LIMIT, DEFAULT_SILENT_BUFFER_CAP, RecoverConfig,
    SessionMetadata,
};
use super::handle::SessionHandle;

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
    /// Session store for persistence.
    store: Arc<dyn SessionStore>,
    /// Event log compaction mode.
    compaction_mode: CompactionMode,
    /// Shutdown signal sender.
    shutdown_tx: Arc<watch::Sender<bool>>,
    /// Shutdown signal receiver (cloned for each actor).
    shutdown_rx: watch::Receiver<bool>,
}

/// Options for creating a new session.
pub struct CreateSessionOpts {
    pub on_disconnect: OnDisconnect,
    pub gateway: Option<String>,
    pub gateway_chat_id: Option<String>,
    pub silent_buffer_cap: usize,
    pub actor_message_limit: usize,
    pub compaction_override: Option<CompactionMode>,
}

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
// Constants
// ============================================================================

/// Maximum concurrent metadata fetches for `list()`.
const LIST_CONCURRENCY: usize = 32;

// ============================================================================
// Implementation
// ============================================================================

impl SessionRegistry {
    // ------------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------------

    /// Create a new session registry.
    pub fn new(store: Arc<dyn SessionStore>, compaction_mode: CompactionMode) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            handles: Arc::new(DashMap::new()),
            task_handles: Arc::new(Mutex::new(Vec::new())),
            store,
            compaction_mode,
            shutdown_tx: Arc::new(shutdown_tx),
            shutdown_rx,
        }
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

    // ------------------------------------------------------------------------
    // Core API
    // ------------------------------------------------------------------------

    /// Create a new session.
    ///
    /// Spawns a new actor and makes it immediately visible in the registry.
    /// Then waits for the initial snapshot to be persisted for crash safety.
    /// If persistence fails, the session is removed and the actor is stopped.
    pub async fn create(
        &self,
        agent: &str,
        opts: CreateSessionOpts,
    ) -> Result<SessionHandle, ActorError> {
        let id = format!("{}{}", SESSION_ID_PREFIX, Ulid::new());

        let config = ActorConfig {
            id: id.clone(),
            agent: agent.to_string(),
            store: self.store.clone(),
            on_disconnect: opts.on_disconnect,
            gateway: opts.gateway,
            gateway_chat_id: opts.gateway_chat_id,
            silent_buffer_cap: opts.silent_buffer_cap,
            actor_message_limit: opts.actor_message_limit,
            compaction_mode: opts.compaction_override.unwrap_or(self.compaction_mode),
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
        let mut guard = self.task_handles.lock().await;
        guard.retain(|h| !h.is_finished());
        guard.push(task_handle);

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

    /// Remove a session handle from the registry.
    ///
    /// Returns true if a session was removed.
    /// When all clones of the handle are dropped, the actor shuts down naturally.
    pub fn remove(&self, id: &str) -> bool {
        self.handles.remove(id).is_some()
    }

    /// Get a reference to the session store.
    pub fn store(&self) -> &Arc<dyn SessionStore> {
        &self.store
    }

    /// Get the number of active sessions.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    // ------------------------------------------------------------------------
    // Recovery
    // ------------------------------------------------------------------------

    /// Recover sessions from disk on startup.
    ///
    /// Scans the sessions directory and spawns actors for recoverable sessions.
    pub async fn recover(&self) -> Result<RecoveryResult, ActorError> {
        let mut result = RecoveryResult::default();

        // List all session IDs from the store
        let session_ids = self
            .store
            .list()
            .await
            .map_err(|e| ActorError::Persistence(format!("Failed to list sessions: {}", e)))?;

        if session_ids.is_empty() {
            debug!("No sessions to recover");
            return Ok(result);
        }

        // Process each session
        for session_id in session_ids {
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
        // Load snapshot
        let snapshot = match self.store.load_snapshot(session_id).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                debug!(
                    session_id = %session_id,
                    "No snapshot found, skipping"
                );
                return Ok(false);
            }
            Err(e) => {
                return Err(ActorError::Persistence(format!(
                    "Failed to load snapshot: {}",
                    e
                )));
            }
        };

        // Load events after checkpoint for replay.
        // For v1 snapshots, replay_from_seq() returns last_event_seq (no replay needed).
        // For v2 snapshots, it returns checkpoint_seq (replay messages after checkpoint).
        let replay_from = snapshot.replay_from_seq();
        let events = self
            .store
            .load_events(session_id, replay_from)
            .await
            .map_err(|e| ActorError::Persistence(format!("Failed to load events: {}", e)))?;

        // Replay events to rebuild pending messages and status
        let mut pending_messages = Vec::new();
        let mut last_seq = snapshot.last_event_seq;
        let mut status = snapshot.status;

        for event in events {
            last_seq = event.seq;

            match &event.payload {
                super::events::SessionEventPayload::UserMessage { content, .. } => {
                    pending_messages
                        .push(crate::llm::Message::text(crate::llm::Role::User, content));
                }
                super::events::SessionEventPayload::AssistantMessage { content, .. } => {
                    pending_messages.push(crate::llm::Message::text(
                        crate::llm::Role::Assistant,
                        content,
                    ));
                }
                super::events::SessionEventPayload::ToolCall {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    let tool_call = crate::llm::ToolCall {
                        id: call_id.clone(),
                        tool_type: "function".to_string(),
                        function: crate::llm::FunctionCall {
                            name: tool_name.clone(),
                            arguments: serde_json::to_string(arguments).unwrap_or_default(),
                        },
                    };
                    pending_messages
                        .push(crate::llm::Message::assistant_tool_calls(vec![tool_call]));
                }
                super::events::SessionEventPayload::ToolResult { call_id, result } => {
                    pending_messages
                        .push(crate::llm::Message::tool_result(call_id, &result.content));
                }
                super::events::SessionEventPayload::StatusChange { to, .. } => {
                    status = *to;
                }
                super::events::SessionEventPayload::SessionEnd { reason } => {
                    status = match reason {
                        super::events::SessionEndReason::Completed
                        | super::events::SessionEndReason::Terminated
                        | super::events::SessionEndReason::Timeout
                        | super::events::SessionEndReason::Error => SessionStatus::Completed,
                    };
                }
                // Other events don't affect conversation or status
                _ => {}
            }
        }

        // Determine the status to recover with
        let final_status = match status {
            SessionStatus::Active => SessionStatus::Active,
            SessionStatus::Paused => SessionStatus::Paused,
            SessionStatus::Running => {
                // Running sessions were interrupted
                match snapshot.config.on_disconnect {
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

        // Extract config values before moving snapshot.config into the new snapshot.
        let recover_silent_buffer_cap = snapshot
            .config
            .silent_buffer_cap
            .unwrap_or(DEFAULT_SILENT_BUFFER_CAP);
        let recover_actor_message_limit = snapshot
            .config
            .actor_message_limit
            .unwrap_or(DEFAULT_ACTOR_MESSAGE_LIMIT);

        // Build snapshot for actor recovery.
        // Keep the original checkpoint_seq; pending_messages are passed separately.
        let recovered_snapshot = super::snapshot::SessionSnapshot::new(
            snapshot.session_id.clone(),
            snapshot.agent.clone(),
            final_status,
            snapshot.created_at,
            super::snapshot::CheckpointState {
                last_event_seq: last_seq,
                checkpoint_seq: snapshot.checkpoint_seq,
                conversation: snapshot.conversation,
            },
            snapshot.config,
        );

        let config = RecoverConfig {
            snapshot: recovered_snapshot,
            store: self.store.clone(),
            pending_messages,
            silent_buffer_cap: recover_silent_buffer_cap,
            actor_message_limit: recover_actor_message_limit,
            compaction_mode: self.compaction_mode,
        };

        let (tx, task_handle) = SessionActor::spawn_recovered(config, self.shutdown_rx.clone());
        let handle = SessionHandle::new(tx, snapshot.session_id.clone(), snapshot.agent.clone());

        // Store the task handle for graceful shutdown
        let mut guard = self.task_handles.lock().await;
        guard.retain(|h| !h.is_finished());
        guard.push(task_handle);

        self.handles.insert(snapshot.session_id.clone(), handle);

        info!(
            session_id = %snapshot.session_id,
            status = %final_status,
            "Recovered session"
        );

        Ok(true)
    }

    // ------------------------------------------------------------------------
    // TTL / Expiry
    // ------------------------------------------------------------------------

    /// Expire sessions that have been inactive beyond the given TTL.
    ///
    /// Checks per-agent TTL override via `agents`. Falls back to `global_ttl`.
    /// Skips sessions already in `Completed` status.
    /// Returns the number of sessions expired.
    pub async fn expire_inactive_sessions(
        &self,
        global_ttl: chrono::Duration,
        chat_session_cache: &super::ChatSessionCache,
        agents: &crate::agent::AgentStore,
    ) -> usize {
        let now = chrono::Utc::now();

        // Collect handles to avoid holding DashMap ref across await
        let handles: Vec<SessionHandle> = self
            .handles
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        let mut expired_count = 0;

        for handle in handles {
            let Ok(metadata) = handle.get_metadata().await else {
                continue;
            };

            // Skip already completed sessions
            if metadata.status == SessionStatus::Completed {
                continue;
            }

            // Determine TTL: per-agent override or global
            let ttl = agents
                .get(&metadata.agent)
                .and_then(|agent| agent.session.ttl_hours)
                .map(|h| chrono::Duration::hours(h as i64))
                .unwrap_or(global_ttl);

            let inactive_duration = now - metadata.updated_at;
            if inactive_duration < ttl {
                continue;
            }

            info!(
                session_id = %metadata.id,
                agent = %metadata.agent,
                inactive_hours = inactive_duration.num_hours(),
                "Expiring inactive session"
            );

            let _ = handle.set_status(SessionStatus::Completed).await;
            chat_session_cache.remove_by_session_id(&metadata.id).await;
            self.handles.remove(&metadata.id);
            expired_count += 1;
        }

        if expired_count > 0 {
            info!(expired = expired_count, "Session expiry sweep complete");
        }

        expired_count
    }

    // ------------------------------------------------------------------------
    // Special
    // ------------------------------------------------------------------------

    /// Register an existing handle (for testing or special cases).
    pub fn register(&self, handle: SessionHandle) {
        self.handles.insert(handle.id().to_string(), handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Message, Role};
    use crate::session::{CheckpointState, SessionConfig, SessionSnapshot};
    use crate::store::file::FileSessionStore;
    use chrono::Utc;
    use tempfile::TempDir;

    fn create_test_registry(temp_dir: &TempDir) -> (SessionRegistry, Arc<FileSessionStore>) {
        let store = Arc::new(FileSessionStore::new(temp_dir.path()));
        let registry = SessionRegistry::new(store.clone(), CompactionMode::Disabled);
        (registry, store)
    }

    async fn write_test_snapshot(
        store: &Arc<FileSessionStore>,
        session_id: &str,
        status: SessionStatus,
        on_disconnect: OnDisconnect,
    ) {
        let snapshot = SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            status,
            Utc::now(),
            CheckpointState {
                last_event_seq: 1,
                checkpoint_seq: 1,
                conversation: vec![
                    Message::text(Role::User, "Hello"),
                    Message::text(Role::Assistant, "Hi there!"),
                ],
            },
            SessionConfig {
                on_disconnect,
                ..Default::default()
            },
        );
        store.save_snapshot(session_id, &snapshot).await.unwrap();
    }

    #[tokio::test]
    async fn create_session_returns_handle() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        let handle = registry
            .create(
                "test-agent",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
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
        let (registry, _store) = create_test_registry(&temp_dir);

        assert!(registry.get("session_unknown").is_none());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn list_returns_all_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        registry
            .create(
                "agent1",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
            .await
            .unwrap();
        registry
            .create(
                "agent2",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Continue,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
            .await
            .unwrap();

        let sessions = registry.list().await;
        assert_eq!(sessions.len(), 2);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn len_and_is_empty() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry
            .create(
                "agent1",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
            .await
            .unwrap();

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn contains_session() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        let handle = registry
            .create(
                "test-agent",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
            .await
            .unwrap();

        assert!(registry.contains(handle.id()));
        assert!(!registry.contains("session_unknown"));

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn remove_session() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        let handle = registry
            .create(
                "test-agent",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
            .await
            .unwrap();
        let id = handle.id().to_string();

        assert!(registry.contains(&id));
        assert!(registry.remove(&id));
        assert!(!registry.contains(&id));
        assert!(!registry.remove(&id)); // second remove returns false

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, _store) = create_test_registry(&temp_dir);

        let result = registry.recover().await.unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());

        registry.shutdown().await;
    }

    #[tokio::test]
    async fn recover_active_session() {
        let temp_dir = TempDir::new().unwrap();
        let (registry, store) = create_test_registry(&temp_dir);

        write_test_snapshot(
            &store,
            "session_active",
            SessionStatus::Active,
            OnDisconnect::Pause,
        )
        .await;

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
        let (registry, store) = create_test_registry(&temp_dir);

        write_test_snapshot(
            &store,
            "session_completed",
            SessionStatus::Completed,
            OnDisconnect::Pause,
        )
        .await;

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
        let (registry, store) = create_test_registry(&temp_dir);

        write_test_snapshot(
            &store,
            "session_running",
            SessionStatus::Running,
            OnDisconnect::Continue,
        )
        .await;

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
        let (registry, _store) = create_test_registry(&temp_dir);

        let handle = registry
            .create(
                "test-agent",
                CreateSessionOpts {
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                    silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
                    actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
                    compaction_override: None,
                },
            )
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
        let snapshot_file = temp_dir.path().join(&session_id).join("state.json");
        assert!(snapshot_file.exists());
    }
}
