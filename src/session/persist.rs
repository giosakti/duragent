//! Unified session persistence.
//!
//! Provides coordinated updates to session state across:
//! - In-memory store (SessionStore)
//! - Event log (events.jsonl)
//! - Snapshot (state.yaml)

use std::path::Path;

use chrono::{DateTime, Utc};

use super::SessionStore;
use super::error::Result;
use super::event_writer::EventWriter;
use super::events::{SessionEvent, SessionEventPayload};
use super::snapshot::{SessionConfig, SessionSnapshot};
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
        },
    )
}

// ============================================================================
// Persistence Functions
// ============================================================================

/// Record an event and write a snapshot atomically.
///
/// This is the primary persistence operation for session state changes.
/// It ensures the event log and snapshot stay in sync.
///
/// Uses peek/commit pattern to avoid sequence drift: the sequence number is only
/// committed to memory after successful disk writes.
pub async fn commit_event(ctx: &SessionContext<'_>, payload: SessionEventPayload) -> Result<u64> {
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
) -> Result<u64> {
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
    content: String,
    usage: Option<Usage>,
) -> Result<u64> {
    let assistant_message = Message {
        role: Role::Assistant,
        content: content.clone(),
    };
    sessions.add_message(session_id, assistant_message).await?;

    record_event(
        sessions,
        sessions_path,
        session_id,
        SessionEventPayload::AssistantMessage { content, usage },
    )
    .await
}

/// Write a snapshot without recording an event.
///
/// Use this when session state changes without a new event (e.g., status updates).
pub async fn write_session_snapshot(ctx: &SessionContext<'_>) -> Result<()> {
    let last_event_seq = ctx.sessions.last_event_seq(ctx.session_id).await?;
    let conversation = ctx
        .sessions
        .get_messages(ctx.session_id)
        .await
        .ok_or_else(|| super::error::SessionError::NotFound(ctx.session_id.to_string()))?;
    let snapshot = build_snapshot(ctx, last_event_seq, conversation);
    write_snapshot(ctx.sessions_path, ctx.session_id, &snapshot).await
}
