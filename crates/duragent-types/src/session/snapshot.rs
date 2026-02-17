//! Session snapshot schema for fast resume.
//!
//! Evaluation methods (`is_compatible`, `replay_from_seq`) live in
//! `duragent::session::snapshot_eval`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::Message;

use super::events::PendingApproval;

/// How old events are handled after a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionMode {
    /// Remove old events (default — keeps things simple).
    #[default]
    Discard,
    /// Move old events to events.archive.jsonl before truncating.
    Archive,
    /// No compaction (events.jsonl grows unbounded, existing behavior).
    Disabled,
}

/// A snapshot of session state for fast resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Schema version for forward compatibility.
    pub schema_version: String,
    /// The session ID.
    pub session_id: String,
    /// The agent this session is using.
    pub agent: String,
    /// Current session status.
    pub status: SessionStatus,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When this snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// The sequence number of the last event included in this snapshot.
    pub last_event_seq: u64,
    /// The sequence number up to which messages are checkpointed.
    #[serde(default)]
    pub checkpoint_seq: u64,
    /// The conversation history (up to checkpoint_seq).
    pub conversation: Vec<Message>,
    /// Session configuration.
    pub config: SessionConfig,
}

/// Session configuration stored in the snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Behavior when client disconnects.
    #[serde(default)]
    pub on_disconnect: OnDisconnect,

    /// Gateway this session belongs to (e.g., "telegram").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,

    /// Platform-specific chat identifier for routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_chat_id: Option<String>,

    /// Pending approval waiting for user decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval: Option<PendingApproval>,

    /// Maximum entries in the ephemeral silent message buffer.
    #[serde(default)]
    pub silent_buffer_cap: Option<usize>,

    /// Maximum total messages before trimming oldest.
    #[serde(default)]
    pub actor_message_limit: Option<usize>,
}

/// Checkpoint data for a snapshot: event sequences and conversation state.
pub struct CheckpointState {
    /// The sequence number of the last event included in this snapshot.
    pub last_event_seq: u64,
    /// The sequence number up to which messages are checkpointed.
    pub checkpoint_seq: u64,
    /// The conversation history (up to checkpoint_seq).
    pub conversation: Vec<Message>,
}

impl SessionSnapshot {
    /// Current schema version.
    pub const SCHEMA_VERSION: &'static str = "2";

    /// Create a new snapshot from session state.
    #[must_use]
    pub fn new(
        session_id: String,
        agent: String,
        status: SessionStatus,
        created_at: DateTime<Utc>,
        checkpoint: CheckpointState,
        config: SessionConfig,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            session_id,
            agent,
            status,
            created_at,
            snapshot_at: Utc::now(),
            last_event_seq: checkpoint.last_event_seq,
            checkpoint_seq: checkpoint.checkpoint_seq,
            conversation: checkpoint.conversation,
            config,
        }
    }
}
