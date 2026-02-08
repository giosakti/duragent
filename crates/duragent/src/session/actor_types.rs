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
use crate::llm::{Message, Usage};
use crate::store::SessionStore;

use super::events::ApprovalDecisionType;
use super::snapshot::PendingApproval;

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

// ============================================================================
// Error Types
// ============================================================================

/// Errors from actor operations.
#[derive(Debug, Error)]
pub enum ActorError {
    /// The actor has shut down.
    #[error("actor has shut down")]
    ActorShutdown,

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

/// Configuration for spawning a new actor.
pub struct ActorConfig {
    pub id: String,
    pub agent: String,
    pub store: Arc<dyn SessionStore>,
    pub on_disconnect: OnDisconnect,
    pub gateway: Option<String>,
    pub gateway_chat_id: Option<String>,
}

/// Configuration for recovering an actor from a snapshot.
pub struct RecoverConfig {
    pub snapshot: super::snapshot::SessionSnapshot,
    pub store: Arc<dyn SessionStore>,
    /// Messages reconstructed from events after checkpoint_seq.
    pub pending_messages: Vec<Message>,
}

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
