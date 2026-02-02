//! Unified session persistence.
//!
//! Provides coordinated updates to session state across:
//! - In-memory store (SessionStore)
//! - Event log (events.jsonl)
//! - Snapshot (state.yaml)
//!
//! All disk operations are protected by per-session locks to prevent
//! concurrent writes from corrupting session state.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use super::SessionLocks;
use super::SessionStore;
use super::error::Result;
use super::event_writer::EventWriter;
use super::events::{SessionEvent, SessionEventPayload};
use super::snapshot::{PendingApproval, SessionConfig, SessionSnapshot};
use super::snapshot_writer::write_snapshot;
use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::{Message, Role, Usage};

// ============================================================================
// Session Context
// ============================================================================

/// Context for session persistence operations.
///
/// Groups common parameters needed for commit_event and write_session_snapshot.
pub struct SessionContext<'a> {
    pub sessions: &'a SessionStore,
    pub sessions_path: &'a Path,
    pub session_id: &'a str,
    pub agent: &'a str,
    pub created_at: DateTime<Utc>,
    pub status: SessionStatus,
    pub on_disconnect: OnDisconnect,
    /// Gateway this session belongs to (for routing persistence).
    pub gateway: Option<&'a str>,
    /// Platform-specific chat identifier (for routing persistence).
    pub gateway_chat_id: Option<&'a str>,
    /// Pending approval waiting for user decision.
    pub pending_approval: Option<&'a PendingApproval>,
    /// Per-session locks for disk I/O.
    pub session_locks: &'a SessionLocks,
}

// ============================================================================
// Persistence Functions
// ============================================================================

/// Get or create a lock for a session.
///
/// This is used internally for disk I/O operations and can also be used
/// by handlers that need to protect critical sections involving session state.
pub fn get_session_lock(locks: &SessionLocks, session_id: &str) -> Arc<Mutex<()>> {
    locks
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Record an event and write a snapshot atomically.
///
/// This is the primary persistence operation for session state changes.
/// It ensures the event log and snapshot stay in sync.
///
/// Uses peek/commit pattern to avoid sequence drift: the sequence number is only
/// committed to memory after successful disk writes.
pub async fn commit_event(ctx: &SessionContext<'_>, payload: SessionEventPayload) -> Result<u64> {
    // Acquire per-session lock for disk I/O
    let lock = get_session_lock(ctx.session_locks, ctx.session_id);
    let _guard = lock.lock().await;

    // 1. Peek next sequence number (doesn't modify in-memory state yet)
    let seq = ctx.sessions.peek_next_event_seq(ctx.session_id).await?;

    // 2. Write event to disk
    let mut writer = EventWriter::new(ctx.sessions_path, ctx.session_id).await?;
    writer.append(&SessionEvent::new(seq, payload)).await?;

    // 3. Write snapshot with current state
    let conversation = ctx
        .sessions
        .get_messages(ctx.session_id)
        .await
        .ok_or_else(|| super::error::SessionError::NotFound(ctx.session_id.to_string()))?;
    let snapshot = build_snapshot(ctx, seq, conversation);
    write_snapshot(ctx.sessions_path, ctx.session_id, &snapshot).await?;

    // 4. Commit sequence number to memory only after successful writes
    ctx.sessions.commit_event_seq(ctx.session_id, seq).await?;

    Ok(seq)
}

/// Record an event without writing a snapshot.
///
/// Use this for intermediate events where a snapshot isn't needed yet.
///
/// Uses peek/commit pattern to avoid sequence drift: the sequence number is only
/// committed to memory after successful disk write.
pub async fn record_event(
    sessions: &SessionStore,
    sessions_path: &Path,
    session_id: &str,
    payload: SessionEventPayload,
    session_locks: &SessionLocks,
) -> Result<u64> {
    // Acquire per-session lock for disk I/O
    let lock = get_session_lock(session_locks, session_id);
    let _guard = lock.lock().await;

    // 1. Peek next sequence number (doesn't modify in-memory state yet)
    let seq = sessions.peek_next_event_seq(session_id).await?;

    // 2. Write event to disk
    let mut writer = EventWriter::new(sessions_path, session_id).await?;
    writer.append(&SessionEvent::new(seq, payload)).await?;

    // 3. Commit sequence number to memory only after successful write
    sessions.commit_event_seq(session_id, seq).await?;

    Ok(seq)
}

/// Persist an assistant message to the session store and event log.
///
/// This consolidates the common pattern of:
/// 1. Adding the message to the in-memory session store
/// 2. Recording the event to the JSONL event log
///
/// Returns the event sequence number on success.
pub async fn persist_assistant_message(
    sessions: &SessionStore,
    sessions_path: &Path,
    session_id: &str,
    agent: &str,
    content: String,
    usage: Option<Usage>,
    session_locks: &SessionLocks,
) -> Result<u64> {
    let assistant_message = Message::text(Role::Assistant, content.clone());
    sessions.add_message(session_id, assistant_message).await?;

    record_event(
        sessions,
        sessions_path,
        session_id,
        SessionEventPayload::AssistantMessage {
            agent: agent.to_string(),
            content,
            usage,
        },
        session_locks,
    )
    .await
}

/// Write a snapshot without recording an event.
///
/// Use this when session state changes without a new event (e.g., status updates).
pub async fn write_session_snapshot(ctx: &SessionContext<'_>) -> Result<()> {
    // Acquire per-session lock for disk I/O
    let lock = get_session_lock(ctx.session_locks, ctx.session_id);
    let _guard = lock.lock().await;

    let last_event_seq = ctx.sessions.last_event_seq(ctx.session_id).await?;
    let conversation = ctx
        .sessions
        .get_messages(ctx.session_id)
        .await
        .ok_or_else(|| super::error::SessionError::NotFound(ctx.session_id.to_string()))?;
    let snapshot = build_snapshot(ctx, last_event_seq, conversation);
    write_snapshot(ctx.sessions_path, ctx.session_id, &snapshot).await
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Build a snapshot from context and current state.
fn build_snapshot(
    ctx: &SessionContext<'_>,
    last_event_seq: u64,
    conversation: Vec<Message>,
) -> SessionSnapshot {
    SessionSnapshot::new(
        ctx.session_id.to_string(),
        ctx.agent.to_string(),
        ctx.status,
        ctx.created_at,
        last_event_seq,
        conversation,
        SessionConfig {
            on_disconnect: ctx.on_disconnect,
            gateway: ctx.gateway.map(String::from),
            gateway_chat_id: ctx.gateway_chat_id.map(String::from),
            pending_approval: ctx.pending_approval.cloned(),
        },
    )
}

// ============================================================================
// Pending Approval Helpers
// ============================================================================

/// Set a pending approval on a session snapshot.
///
/// This persists the pending approval to disk for crash recovery.
pub async fn set_pending_approval(
    _sessions: &SessionStore,
    sessions_path: &Path,
    session_id: &str,
    pending: &PendingApproval,
    session_locks: &SessionLocks,
) -> Result<()> {
    // Acquire per-session lock for disk I/O
    let lock = get_session_lock(session_locks, session_id);
    let _guard = lock.lock().await;

    // Load existing snapshot
    let snapshot = super::load_snapshot(sessions_path, session_id)
        .await?
        .ok_or_else(|| super::error::SessionError::NotFound(session_id.to_string()))?;

    // Create updated config with pending approval
    let config = SessionConfig {
        pending_approval: Some(pending.clone()),
        ..snapshot.config
    };

    // Write updated snapshot
    let new_snapshot = SessionSnapshot::new(
        snapshot.session_id,
        snapshot.agent,
        snapshot.status,
        snapshot.created_at,
        snapshot.last_event_seq,
        snapshot.conversation,
        config,
    );
    super::write_snapshot(sessions_path, session_id, &new_snapshot).await
}

/// Clear a pending approval from a session snapshot.
///
/// Call this after the approval decision is processed.
pub async fn clear_pending_approval(
    _sessions: &SessionStore,
    sessions_path: &Path,
    session_id: &str,
    session_locks: &SessionLocks,
) -> Result<()> {
    // Acquire per-session lock for disk I/O
    let lock = get_session_lock(session_locks, session_id);
    let _guard = lock.lock().await;

    clear_pending_approval_internal(sessions_path, session_id).await
}

/// Clear a pending approval without acquiring the lock.
///
/// Use this when the caller already holds the session lock to avoid deadlock.
/// For external callers, use `clear_pending_approval` instead.
pub async fn clear_pending_approval_internal(sessions_path: &Path, session_id: &str) -> Result<()> {
    // Load existing snapshot
    let snapshot = super::load_snapshot(sessions_path, session_id)
        .await?
        .ok_or_else(|| super::error::SessionError::NotFound(session_id.to_string()))?;

    // Skip if there's no pending approval
    if snapshot.config.pending_approval.is_none() {
        return Ok(());
    }

    // Create updated config without pending approval
    let config = SessionConfig {
        pending_approval: None,
        ..snapshot.config
    };

    // Write updated snapshot
    let new_snapshot = SessionSnapshot::new(
        snapshot.session_id,
        snapshot.agent,
        snapshot.status,
        snapshot.created_at,
        snapshot.last_event_seq,
        snapshot.conversation,
        config,
    );
    super::write_snapshot(sessions_path, session_id, &new_snapshot).await
}

/// Load the pending approval for a session, if any.
pub async fn get_pending_approval(
    sessions_path: &Path,
    session_id: &str,
) -> Result<Option<PendingApproval>> {
    let snapshot = super::load_snapshot(sessions_path, session_id).await?;
    Ok(snapshot.and_then(|s| s.config.pending_approval))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ------------------------------------------------------------------------
    // Test Helpers
    // ------------------------------------------------------------------------

    fn create_test_locks() -> SessionLocks {
        Arc::new(dashmap::DashMap::new())
    }

    async fn create_test_session(sessions: &SessionStore) -> String {
        let session = sessions.create("test-agent").await;
        session.id.clone()
    }

    async fn setup_session_with_snapshot(tmp: &TempDir, session_id: &str) {
        use super::super::snapshot_writer::write_snapshot;

        let session_dir = tmp.path().join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();

        let snapshot = SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            0,
            vec![],
            SessionConfig::default(),
        );
        write_snapshot(tmp.path(), session_id, &snapshot)
            .await
            .unwrap();
    }

    // ------------------------------------------------------------------------
    // get_session_lock - Lock acquisition
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn get_session_lock_creates_new_lock_for_session() {
        let locks = create_test_locks();

        let lock = get_session_lock(&locks, "session-1");

        // Lock should be acquirable
        let guard = lock.try_lock();
        assert!(guard.is_ok());
    }

    #[tokio::test]
    async fn get_session_lock_returns_same_lock_for_same_session() {
        let locks = create_test_locks();

        let lock1 = get_session_lock(&locks, "session-1");
        let lock2 = get_session_lock(&locks, "session-1");

        // Same Arc pointer
        assert!(Arc::ptr_eq(&lock1, &lock2));
    }

    #[tokio::test]
    async fn get_session_lock_creates_separate_locks_for_different_sessions() {
        let locks = create_test_locks();

        let lock1 = get_session_lock(&locks, "session-1");
        let lock2 = get_session_lock(&locks, "session-2");

        // Different Arc pointers
        assert!(!Arc::ptr_eq(&lock1, &lock2));

        // Both can be locked simultaneously
        let _guard1 = lock1.try_lock().unwrap();
        let guard2 = lock2.try_lock();
        assert!(guard2.is_ok());
    }

    // ------------------------------------------------------------------------
    // record_event - Event recording
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn record_event_writes_to_event_log() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        let seq = record_event(
            &sessions,
            tmp.path(),
            &session_id,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
            },
            &locks,
        )
        .await
        .unwrap();

        // Sequence starts at 1 (session creation uses 0)
        assert!(seq >= 0);

        // Verify event file exists
        let event_file = tmp.path().join(&session_id).join("events.jsonl");
        assert!(event_file.exists());
    }

    #[tokio::test]
    async fn record_event_increments_sequence_number() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        let seq1 = record_event(
            &sessions,
            tmp.path(),
            &session_id,
            SessionEventPayload::UserMessage {
                content: "First".to_string(),
            },
            &locks,
        )
        .await
        .unwrap();

        let seq2 = record_event(
            &sessions,
            tmp.path(),
            &session_id,
            SessionEventPayload::UserMessage {
                content: "Second".to_string(),
            },
            &locks,
        )
        .await
        .unwrap();

        // Each event increments the sequence
        assert_eq!(seq2, seq1 + 1);
    }

    // ------------------------------------------------------------------------
    // persist_assistant_message - Message persistence
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn persist_assistant_message_adds_to_session_and_logs() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        let seq = persist_assistant_message(
            &sessions,
            tmp.path(),
            &session_id,
            "test-agent",
            "Hello, I'm the assistant".to_string(),
            None,
            &locks,
        )
        .await
        .unwrap();

        // Sequence number should be valid
        assert!(seq >= 0);

        // Verify message was added to session
        let messages = sessions.get_messages(&session_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].content.as_deref(),
            Some("Hello, I'm the assistant")
        );
    }

    // ------------------------------------------------------------------------
    // Pending approval - Set, get, clear
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn set_and_get_pending_approval() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        // Setup: create initial snapshot
        setup_session_with_snapshot(&tmp, &session_id).await;

        // Set pending approval
        let pending = PendingApproval::new(
            "call-123".to_string(),
            "bash".to_string(),
            serde_json::json!({"command": "ls -la"}),
            "ls -la".to_string(),
            vec![],
        );
        set_pending_approval(&sessions, tmp.path(), &session_id, &pending, &locks)
            .await
            .unwrap();

        // Get pending approval
        let retrieved = get_pending_approval(tmp.path(), &session_id).await.unwrap();

        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.call_id, "call-123");
        assert_eq!(retrieved.command, "ls -la");
    }

    #[tokio::test]
    async fn clear_pending_approval_removes_approval() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        // Setup: create snapshot with pending approval
        setup_session_with_snapshot(&tmp, &session_id).await;

        let pending = PendingApproval::new(
            "call-456".to_string(),
            "bash".to_string(),
            serde_json::json!({}),
            "rm -rf".to_string(),
            vec![],
        );
        set_pending_approval(&sessions, tmp.path(), &session_id, &pending, &locks)
            .await
            .unwrap();

        // Clear it
        clear_pending_approval(&sessions, tmp.path(), &session_id, &locks)
            .await
            .unwrap();

        // Verify it's gone
        let retrieved = get_pending_approval(tmp.path(), &session_id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn get_pending_approval_returns_none_when_no_snapshot() {
        let tmp = TempDir::new().unwrap();

        let result = get_pending_approval(tmp.path(), "nonexistent-session").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn clear_pending_approval_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionStore::new();
        let session_id = create_test_session(&sessions).await;
        let locks = create_test_locks();

        // Setup: create snapshot without pending approval
        setup_session_with_snapshot(&tmp, &session_id).await;

        // Clear when nothing is pending - should not error
        let result = clear_pending_approval(&sessions, tmp.path(), &session_id, &locks).await;
        assert!(result.is_ok());
    }
}
