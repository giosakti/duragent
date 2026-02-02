//! Per-session actor for serialized state mutations.
//!
//! Each session gets a dedicated actor task that:
//! - Serializes all mutations via message passing (no locks)
//! - Owns both in-memory state and disk persistence
//! - Batches WAL writes and debounces snapshots
//!
//! This eliminates re-entrant deadlocks and improves throughput by
//! reducing per-event fsync overhead.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, interval_at};
use tracing::{debug, warn};

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::{Message, Role, Usage};

use super::event_writer::EventWriter;
use super::events::{SessionEvent, SessionEventPayload, ToolResultData};
use super::snapshot::{PendingApproval, SessionConfig, SessionSnapshot};
use super::snapshot_writer::write_snapshot;

// ============================================================================
// Configuration Constants
// ============================================================================

/// Maximum events to batch before forcing a flush.
const BATCH_SIZE: usize = 10;

/// Interval at which pending events are flushed to disk.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Number of events between snapshots.
const SNAPSHOT_INTERVAL: u64 = 50;

/// Channel capacity for commands.
///
/// Sized to handle burst traffic during agentic loops with many tool calls.
/// If this fills up, callers will block on send(), causing backpressure.
const CHANNEL_CAPACITY: usize = 256;

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

impl From<super::error::SessionError> for ActorError {
    fn from(e: super::error::SessionError) -> Self {
        ActorError::Persistence(e.to_string())
    }
}

// ============================================================================
// Session Command
// ============================================================================

/// Commands that can be sent to a session actor.
pub enum SessionCommand {
    // Write operations
    AddUserMessage {
        content: String,
        reply: oneshot::Sender<Result<u64, ActorError>>,
    },
    AddAssistantMessage {
        content: String,
        usage: Option<Usage>,
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
        decision: super::events::ApprovalDecisionType,
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
// Session Actor
// ============================================================================

/// Per-session actor that owns state and handles mutations.
pub struct SessionActor {
    // Identity
    id: String,
    agent: String,

    // State
    status: SessionStatus,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    messages: Vec<Message>,

    // Event sequencing
    last_event_seq: u64,
    last_flushed_seq: u64,
    last_snapshot_seq: u64,

    // Approval state
    pending_approval: Option<PendingApproval>,

    // Configuration
    on_disconnect: OnDisconnect,
    gateway: Option<String>,
    gateway_chat_id: Option<String>,

    // Persistence
    sessions_path: PathBuf,
    pending_events: VecDeque<SessionEvent>,

    // Communication
    command_rx: mpsc::Receiver<SessionCommand>,
    shutdown_rx: watch::Receiver<bool>,
}

/// Configuration for spawning a new actor.
pub struct ActorConfig {
    pub id: String,
    pub agent: String,
    pub sessions_path: PathBuf,
    pub on_disconnect: OnDisconnect,
    pub gateway: Option<String>,
    pub gateway_chat_id: Option<String>,
}

/// Configuration for recovering an actor from a snapshot.
pub struct RecoverConfig {
    pub snapshot: SessionSnapshot,
    pub sessions_path: PathBuf,
}

impl SessionActor {
    /// Spawn a new session actor for a fresh session.
    ///
    /// Returns the command sender and a JoinHandle for the actor task.
    /// The actor writes a SessionStart event and initial snapshot before
    /// processing any commands (crash safety).
    pub fn spawn(
        config: ActorConfig,
        shutdown_rx: watch::Receiver<bool>,
    ) -> (mpsc::Sender<SessionCommand>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let now = Utc::now();

        let actor = Self {
            id: config.id.clone(),
            agent: config.agent,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            last_event_seq: 0,
            last_flushed_seq: 0,
            last_snapshot_seq: 0,
            pending_approval: None,
            on_disconnect: config.on_disconnect,
            gateway: config.gateway,
            gateway_chat_id: config.gateway_chat_id,
            sessions_path: config.sessions_path,
            pending_events: VecDeque::new(),
            command_rx: rx,
            shutdown_rx,
        };

        let handle = tokio::spawn(actor.run());
        (tx, handle)
    }

    /// Spawn an actor recovered from a snapshot.
    ///
    /// Returns the command sender and a JoinHandle for the actor task.
    pub fn spawn_recovered(
        config: RecoverConfig,
        shutdown_rx: watch::Receiver<bool>,
    ) -> (mpsc::Sender<SessionCommand>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let snapshot = config.snapshot;

        let actor = Self {
            id: snapshot.session_id.clone(),
            agent: snapshot.agent,
            status: snapshot.status,
            created_at: snapshot.created_at,
            updated_at: snapshot.snapshot_at,
            messages: snapshot.conversation,
            last_event_seq: snapshot.last_event_seq,
            last_flushed_seq: snapshot.last_event_seq,
            last_snapshot_seq: snapshot.last_event_seq,
            pending_approval: snapshot.config.pending_approval,
            on_disconnect: snapshot.config.on_disconnect,
            gateway: snapshot.config.gateway,
            gateway_chat_id: snapshot.config.gateway_chat_id,
            sessions_path: config.sessions_path,
            pending_events: VecDeque::new(),
            command_rx: rx,
            shutdown_rx,
        };

        let handle = tokio::spawn(actor.run_recovered());
        (tx, handle)
    }

    /// Main actor loop for new sessions.
    ///
    /// Writes SessionStart event and initial snapshot before processing commands.
    async fn run(mut self) {
        debug!(session_id = %self.id, "Session actor started (new session)");

        // Write SessionStart event and initial snapshot (crash safety)
        self.write_session_start().await;

        // Enter the main command loop
        self.command_loop().await;
    }

    /// Main actor loop for recovered sessions.
    ///
    /// State is already persisted, so just process commands.
    async fn run_recovered(mut self) {
        debug!(session_id = %self.id, "Session actor started (recovered)");

        // Enter the main command loop
        self.command_loop().await;
    }

    /// Write SessionStart event and initial snapshot.
    ///
    /// If this fails, the events remain queued for the next flush attempt.
    /// The caller (registry.create) will verify durability via force_flush().
    async fn write_session_start(&mut self) {
        let seq = self.next_seq();
        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::SessionStart {
                agent: self.agent.clone(),
                on_disconnect: self.on_disconnect,
                gateway: self.gateway.clone(),
                gateway_chat_id: self.gateway_chat_id.clone(),
            },
        ));

        // Attempt flush and snapshot immediately (crash safety)
        // If this fails, events stay queued and force_flush() will retry
        if let Err(e) = self.flush_and_snapshot().await {
            warn!(
                session_id = %self.id,
                error = %e,
                "Initial flush failed, events queued for retry"
            );
        }
    }

    /// Main command processing loop.
    async fn command_loop(&mut self) {
        let mut flush_timer = interval_at(Instant::now() + FLUSH_INTERVAL, FLUSH_INTERVAL);

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        debug!(session_id = %self.id, "Session actor received shutdown signal");
                        // Drain and process remaining queued commands before shutdown
                        self.drain_commands().await;
                        let _ = self.flush_and_snapshot().await;
                        break;
                    }
                }

                // Process commands
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            self.handle_command(command).await;

                            // Check if batch is full
                            if self.pending_events.len() >= BATCH_SIZE {
                                let _ = self.flush_events().await;
                            }
                        }
                        None => {
                            // All senders dropped, shutdown
                            debug!(session_id = %self.id, "All handles dropped, shutting down");
                            let _ = self.flush_and_snapshot().await;
                            break;
                        }
                    }
                }

                // Periodic flush
                _ = flush_timer.tick() => {
                    if !self.pending_events.is_empty() {
                        let _ = self.flush_events().await;
                    }
                }
            }
        }

        debug!(session_id = %self.id, "Session actor stopped");
    }

    /// Drain and process all remaining commands in the queue.
    async fn drain_commands(&mut self) {
        while let Ok(cmd) = self.command_rx.try_recv() {
            self.handle_command(cmd).await;
        }
    }

    /// Handle a single command.
    async fn handle_command(&mut self, cmd: SessionCommand) {
        match cmd {
            SessionCommand::AddUserMessage { content, reply } => {
                let result = self.add_user_message(content).await;
                let _ = reply.send(result);
            }
            SessionCommand::AddAssistantMessage {
                content,
                usage,
                reply,
            } => {
                let result = self.add_assistant_message(content, usage).await;
                let _ = reply.send(result);
            }
            SessionCommand::RecordToolCall {
                call_id,
                tool_name,
                arguments,
                reply,
            } => {
                let result = self.record_tool_call(call_id, tool_name, arguments).await;
                let _ = reply.send(result);
            }
            SessionCommand::RecordToolResult {
                call_id,
                success,
                content,
                reply,
            } => {
                let result = self.record_tool_result(call_id, success, content).await;
                let _ = reply.send(result);
            }
            SessionCommand::RecordApprovalRequired {
                call_id,
                command,
                reply,
            } => {
                let result = self.record_approval_required(call_id, command).await;
                let _ = reply.send(result);
            }
            SessionCommand::RecordApprovalDecision {
                call_id,
                decision,
                reply,
            } => {
                let result = self.record_approval_decision(call_id, decision).await;
                let _ = reply.send(result);
            }
            SessionCommand::SetPendingApproval { pending, reply } => {
                let result = self.set_pending_approval(pending).await;
                let _ = reply.send(result);
            }
            SessionCommand::ClearPendingApproval { reply } => {
                let result = self.clear_pending_approval().await;
                let _ = reply.send(result);
            }
            SessionCommand::SetStatus { status, reply } => {
                let result = self.set_status(status).await;
                let _ = reply.send(result);
            }
            SessionCommand::RecordError {
                code,
                message,
                reply,
            } => {
                let result = self.record_error(code, message).await;
                let _ = reply.send(result);
            }
            SessionCommand::GetMessages { reply } => {
                let _ = reply.send(Ok(self.messages.clone()));
            }
            SessionCommand::GetMetadata { reply } => {
                let metadata = SessionMetadata {
                    id: self.id.clone(),
                    agent: self.agent.clone(),
                    status: self.status,
                    created_at: self.created_at,
                    updated_at: self.updated_at,
                    last_event_seq: self.last_event_seq,
                    on_disconnect: self.on_disconnect,
                    gateway: self.gateway.clone(),
                    gateway_chat_id: self.gateway_chat_id.clone(),
                };
                let _ = reply.send(Ok(metadata));
            }
            SessionCommand::GetPendingApproval { reply } => {
                let _ = reply.send(Ok(self.pending_approval.clone()));
            }
            SessionCommand::FinalizeStream {
                content,
                usage,
                reply,
            } => {
                let result = self.finalize_stream(content, usage).await;
                let _ = reply.send(result);
            }
            SessionCommand::ForceFlush { reply } => {
                let result = self.flush_events().await;
                let _ = reply.send(result);
            }
            SessionCommand::ForceSnapshot { reply } => {
                let result = self.flush_and_snapshot().await;
                let _ = reply.send(result);
            }
        }
    }

    // ------------------------------------------------------------------------
    // Write Operations
    // ------------------------------------------------------------------------

    async fn add_user_message(&mut self, content: String) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        // Add to memory
        self.messages.push(Message::text(Role::User, &content));

        // Queue event
        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::UserMessage { content },
        ));

        Ok(seq)
    }

    async fn add_assistant_message(
        &mut self,
        content: String,
        usage: Option<Usage>,
    ) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        // Add to memory
        self.messages.push(Message::text(Role::Assistant, &content));

        // Queue event
        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::AssistantMessage {
                agent: self.agent.clone(),
                content,
                usage,
            },
        ));

        Ok(seq)
    }

    async fn record_tool_call(
        &mut self,
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    ) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::ToolCall {
                call_id,
                tool_name,
                arguments,
            },
        ));

        Ok(seq)
    }

    async fn record_tool_result(
        &mut self,
        call_id: String,
        success: bool,
        content: String,
    ) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::ToolResult {
                call_id,
                result: ToolResultData { success, content },
            },
        ));

        Ok(seq)
    }

    async fn record_approval_required(
        &mut self,
        call_id: String,
        command: String,
    ) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::ApprovalRequired { call_id, command },
        ));

        Ok(seq)
    }

    async fn record_approval_decision(
        &mut self,
        call_id: String,
        decision: super::events::ApprovalDecisionType,
    ) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::ApprovalDecision { call_id, decision },
        ));

        Ok(seq)
    }

    async fn set_pending_approval(&mut self, pending: PendingApproval) -> Result<(), ActorError> {
        self.updated_at = Utc::now();
        self.pending_approval = Some(pending);

        // Force flush and snapshot on approval changes (crash safety)
        self.flush_and_snapshot().await
    }

    async fn clear_pending_approval(&mut self) -> Result<(), ActorError> {
        if self.pending_approval.is_some() {
            self.updated_at = Utc::now();
            self.pending_approval = None;

            // Force flush and snapshot on approval changes (crash safety)
            self.flush_and_snapshot().await?;
        }

        Ok(())
    }

    async fn set_status(&mut self, status: SessionStatus) -> Result<(), ActorError> {
        if self.status != status {
            self.updated_at = Utc::now();
            let old_status = self.status;
            self.status = status;

            // Record status change event
            let seq = self.next_seq();
            self.pending_events.push_back(SessionEvent::new(
                seq,
                SessionEventPayload::StatusChange {
                    from: old_status,
                    to: status,
                },
            ));

            // Force flush and snapshot on status changes
            self.flush_and_snapshot().await?;
        }

        Ok(())
    }

    async fn record_error(&mut self, code: String, message: String) -> Result<u64, ActorError> {
        self.updated_at = Utc::now();
        let seq = self.next_seq();

        self.pending_events.push_back(SessionEvent::new(
            seq,
            SessionEventPayload::Error { code, message },
        ));

        Ok(seq)
    }

    async fn finalize_stream(
        &mut self,
        content: String,
        usage: Option<Usage>,
    ) -> Result<u64, ActorError> {
        let seq = self.add_assistant_message(content, usage).await?;

        // Force snapshot after stream finalization
        self.flush_and_snapshot().await?;

        Ok(seq)
    }

    // ------------------------------------------------------------------------
    // Persistence
    // ------------------------------------------------------------------------

    fn next_seq(&mut self) -> u64 {
        self.last_event_seq += 1;
        self.last_event_seq
    }

    /// Flush pending events to disk.
    ///
    /// Returns an error if events could not be persisted.
    async fn flush_events(&mut self) -> Result<(), ActorError> {
        if self.pending_events.is_empty() {
            return Ok(());
        }

        let events: Vec<SessionEvent> = self.pending_events.drain(..).collect();
        let last_seq = events
            .last()
            .map(|e| e.seq)
            .unwrap_or(self.last_flushed_seq);

        let mut writer = match EventWriter::new(&self.sessions_path, &self.id).await {
            Ok(w) => w,
            Err(e) => {
                warn!(session_id = %self.id, error = %e, "Failed to open event writer");
                // Re-queue events on failure
                for event in events.into_iter().rev() {
                    self.pending_events.push_front(event);
                }
                return Err(e.into());
            }
        };

        if let Err(e) = writer.append_batch(&events).await {
            warn!(session_id = %self.id, error = %e, "Failed to flush events");
            // Re-queue events on failure
            for event in events.into_iter().rev() {
                self.pending_events.push_front(event);
            }
            return Err(e.into());
        }

        self.last_flushed_seq = last_seq;

        // Check if snapshot is needed
        if self.last_flushed_seq - self.last_snapshot_seq >= SNAPSHOT_INTERVAL {
            self.write_snapshot().await?;
        }

        Ok(())
    }

    /// Write a snapshot of current state.
    async fn write_snapshot(&mut self) -> Result<(), ActorError> {
        let snapshot = SessionSnapshot::new(
            self.id.clone(),
            self.agent.clone(),
            self.status,
            self.created_at,
            self.last_flushed_seq,
            self.messages.clone(),
            SessionConfig {
                on_disconnect: self.on_disconnect,
                gateway: self.gateway.clone(),
                gateway_chat_id: self.gateway_chat_id.clone(),
                pending_approval: self.pending_approval.clone(),
            },
        );

        write_snapshot(&self.sessions_path, &self.id, &snapshot)
            .await
            .map_err(|e| {
                warn!(session_id = %self.id, error = %e, "Failed to write snapshot");
                ActorError::Persistence(e.to_string())
            })?;

        self.last_snapshot_seq = self.last_flushed_seq;
        Ok(())
    }

    /// Flush all pending events and write a snapshot.
    async fn flush_and_snapshot(&mut self) -> Result<(), ActorError> {
        self.flush_events().await?;
        self.write_snapshot().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_actor(
        temp_dir: &TempDir,
    ) -> (
        mpsc::Sender<SessionCommand>,
        watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let config = ActorConfig {
            id: "session_test123".to_string(),
            agent: "test-agent".to_string(),
            sessions_path: temp_dir.path().to_path_buf(),
            on_disconnect: OnDisconnect::Pause,
            gateway: None,
            gateway_chat_id: None,
        };
        let (tx, task_handle) = SessionActor::spawn(config, shutdown_rx);
        (tx, shutdown_tx, task_handle)
    }

    #[tokio::test]
    async fn add_user_message_returns_seq() {
        let temp_dir = TempDir::new().unwrap();
        let (tx, shutdown_tx, _task_handle) = setup_test_actor(&temp_dir);

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::AddUserMessage {
            content: "Hello".to_string(),
            reply: reply_tx,
        })
        .await
        .unwrap();

        let result = reply_rx.await.unwrap();
        assert!(result.is_ok());
        // Seq is 2 because SessionStart event (seq=1) is written first
        assert_eq!(result.unwrap(), 2);

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn get_messages_returns_history() {
        let temp_dir = TempDir::new().unwrap();
        let (tx, shutdown_tx, _task_handle) = setup_test_actor(&temp_dir);

        // Add a message
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::AddUserMessage {
            content: "Test message".to_string(),
            reply: reply_tx,
        })
        .await
        .unwrap();
        reply_rx.await.unwrap().unwrap();

        // Get messages
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::GetMessages { reply: reply_tx })
            .await
            .unwrap();

        let messages = reply_rx.await.unwrap().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content_str(), "Test message");

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn get_metadata_returns_session_info() {
        let temp_dir = TempDir::new().unwrap();
        let (tx, shutdown_tx, _task_handle) = setup_test_actor(&temp_dir);

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::GetMetadata { reply: reply_tx })
            .await
            .unwrap();

        let metadata = reply_rx.await.unwrap().unwrap();
        assert_eq!(metadata.id, "session_test123");
        assert_eq!(metadata.agent, "test-agent");
        assert_eq!(metadata.status, SessionStatus::Active);

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn force_flush_persists_events() {
        let temp_dir = TempDir::new().unwrap();
        let (tx, shutdown_tx, _task_handle) = setup_test_actor(&temp_dir);

        // Add a message
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::AddUserMessage {
            content: "Persistent message".to_string(),
            reply: reply_tx,
        })
        .await
        .unwrap();
        reply_rx.await.unwrap().unwrap();

        // Force flush
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::ForceFlush { reply: reply_tx })
            .await
            .unwrap();
        reply_rx.await.unwrap().unwrap();

        // Verify file exists
        let events_file = temp_dir.path().join("session_test123").join("events.jsonl");
        assert!(events_file.exists());

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn shutdown_flushes_and_snapshots() {
        let temp_dir = TempDir::new().unwrap();
        let (tx, shutdown_tx, _task_handle) = setup_test_actor(&temp_dir);

        // Add a message
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(SessionCommand::AddUserMessage {
            content: "Will be saved".to_string(),
            reply: reply_tx,
        })
        .await
        .unwrap();
        reply_rx.await.unwrap().unwrap();

        // Trigger shutdown
        shutdown_tx.send(true).unwrap();

        // Give actor time to flush
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify snapshot exists
        let snapshot_file = temp_dir.path().join("session_test123").join("state.yaml");
        assert!(snapshot_file.exists());
    }
}
