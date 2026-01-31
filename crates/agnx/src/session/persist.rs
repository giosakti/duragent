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
fn get_session_lock(locks: &SessionLocks, session_id: &str) -> Arc<Mutex<()>> {
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
