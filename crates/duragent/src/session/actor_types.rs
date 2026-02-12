//! Session actor types and protocol.
//!
//! This module defines the command protocol for communicating with session actors,
//! along with configuration and error types.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::config::CompactionMode;
use crate::llm::{Message, Usage};
use crate::store::SessionStore;

use super::agentic_loop::PendingApproval;
use super::events::ApprovalDecisionType;

// ============================================================================
// Session Command
// ============================================================================

/// Commands that can be sent to a session actor.
pub enum SessionCommand {
    // Write operations
    AddUserMessage {
        content: String,
        sender_id: Option<String>,
        sender_name: Option<String>,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    AddAssistantMessage {
        content: String,
        usage: Option<Usage>,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    AddSilentMessage {
        content: String,
        sender_id: String,
        sender_name: Option<String>,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    RecordToolCall {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    RecordToolResult {
        call_id: String,
        success: bool,
        content: String,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    RecordApprovalRequired {
        call_id: String,
        command: String,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    RecordApprovalDecision {
        call_id: String,
        decision: ApprovalDecisionType,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    SetPendingApproval {
        pending: PendingApproval,
        reply: oneshot::Sender<Result<(), ActorError>>,
    },
    ClearPendingApproval {
        reply: oneshot::Sender<Result<(), ActorError>>,
    },
    SetStatus {
        status: SessionStatus,
        reply: oneshot::Sender<Result<(), ActorError>>,
    },
    RecordError {
        code: String,
        message: String,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },

    // Read operations
    GetMessages {
        reply: oneshot::Sender<Result<Vec<Message>, ActorError>>,
    },
    GetRecentSilentMessages {
        max_messages: usize,
        max_age: chrono::Duration,
        reply: oneshot::Sender<Result<Vec<SilentMessageEntry>, ActorError>>,
    },
    GetMetadata {
        reply: oneshot::Sender<Result<SessionMetadata, ActorError>>,
    },
    GetPendingApproval {
        reply: oneshot::Sender<Result<Option<PendingApproval>, ActorError>>,
    },

    // Stream/Flush
    FinalizeStream {
        content: String,
        usage: Option<Usage>,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    ForceFlush {
        reply: oneshot::Sender<Result<(), ActorError>>,
    },
    ForceSnapshot {
        reply: oneshot::Sender<Result<(), ActorError>>,
    },
}

/// Entry in the ephemeral silent message buffer for context injection.
#[derive(Debug, Clone)]
pub struct SilentMessageEntry {
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors from actor operations.
#[derive(Debug, Error)]
pub enum ActorError {
    /// The actor has shut down.
    #[error("actor has shut down")]
    ActorShutdown,

    /// The actor did not reply within the timeout.
    #[error("actor reply timed out")]
    ActorTimeout,

    /// Session not found.
    #[error("session not found: {0}")]
    NotFound(String),

    /// IO error during persistence.
    #[error("persistence error: {0}")]
    Persistence(String),
}

// ============================================================================
// Metadata
// ============================================================================

/// Metadata about a session (returned by GetMetadata).
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_event_seq: u64,
    pub on_disconnect: OnDisconnect,
    pub gateway: Option<String>,
    pub gateway_chat_id: Option<String>,
}

// ============================================================================
// Configuration
// ============================================================================

/// Default silent buffer capacity when no agent config is available.
pub const DEFAULT_SILENT_BUFFER_CAP: usize = 200;

/// Configuration for spawning a new actor.
pub struct ActorConfig {
    pub id: String,
    pub agent: String,
    pub store: Arc<dyn SessionStore>,
    pub on_disconnect: OnDisconnect,
    pub gateway: Option<String>,
    pub gateway_chat_id: Option<String>,
    /// Maximum entries in the ephemeral silent message buffer.
    pub silent_buffer_cap: usize,
    /// Maximum total messages before trimming oldest checkpointed messages.
    pub actor_message_limit: usize,
    /// Event log compaction mode.
    pub compaction_mode: CompactionMode,
}

/// Configuration for recovering an actor from a snapshot.
pub struct RecoverConfig {
    pub snapshot: super::snapshot::SessionSnapshot,
    pub store: Arc<dyn SessionStore>,
    /// Messages reconstructed from events after checkpoint_seq.
    pub pending_messages: Vec<Message>,
    /// Maximum entries in the ephemeral silent message buffer.
    pub silent_buffer_cap: usize,
    /// Maximum total messages before trimming oldest checkpointed messages.
    pub actor_message_limit: usize,
    /// Event log compaction mode.
    pub compaction_mode: CompactionMode,
}

/// Derive actor_message_limit from max_input_tokens.
///
/// Heuristic: `(max_input_tokens / 200) * 2`, clamped to [100, 2000].
/// ~200 tokens per message pair × 2 for user+assistant.
pub fn actor_message_limit(max_input_tokens: u32) -> usize {
    (((max_input_tokens / 200) * 2) as usize).clamp(100, 2000)
}

/// Default actor message limit when agent spec isn't available (e.g., recovery).
pub const DEFAULT_ACTOR_MESSAGE_LIMIT: usize = 1000;

// ============================================================================
// Constants
// ============================================================================

/// Maximum events to batch before forcing a flush.
pub const BATCH_SIZE: usize = 10;

/// Interval at which pending events are flushed to disk.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Number of events between snapshots.
pub const SNAPSHOT_INTERVAL: u64 = 50;

/// Number of pending messages before rolling checkpoint.
///
/// When pending_messages exceeds this threshold, messages are moved
/// to checkpointed_messages and the checkpoint is advanced.
pub const CHECKPOINT_THRESHOLD: usize = 50;

/// Channel capacity for commands.
///
/// Sized to handle burst traffic during agentic loops with many tool calls.
/// If this fills up, callers will block on send(), causing backpressure.
pub const CHANNEL_CAPACITY: usize = 256;
