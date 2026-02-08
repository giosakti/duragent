//! Session handle for communicating with a session actor.
//!
//! `SessionHandle` is a thin wrapper around an `mpsc::Sender<SessionCommand>`.
//! It provides async methods for all session operations and is cheap to clone.

use tokio::sync::{mpsc, oneshot};

use crate::api::SessionStatus;
use crate::llm::{Message, Usage};

use super::actor_types::{ActorError, SessionCommand, SessionMetadata, SilentMessageEntry};
use super::events::ApprovalDecisionType;
use super::snapshot::PendingApproval;

/// Handle for interacting with a session actor.
///
/// This is cheap to clone (just an `Arc` inside the `mpsc::Sender`).
/// All methods are async and communicate with the actor via message passing.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<SessionCommand>,
    id: String,
    agent: String,
}

impl SessionHandle {
    /// Create a new handle from a command sender.
    pub(crate) fn new(tx: mpsc::Sender<SessionCommand>, id: String, agent: String) -> Self {
        Self { tx, id, agent }
    }

    /// Get the session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the agent name.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    // ------------------------------------------------------------------------
    // Write Operations
    // ------------------------------------------------------------------------

    /// Add a user message to the session.
    ///
    /// Returns the event sequence number on success.
    pub async fn add_user_message(&self, content: String) -> Result<u64, ActorError> {
        self.add_user_message_with_sender(content, None, None).await
    }

    /// Add a user message with sender attribution to the session.
    ///
    /// Used for gateway messages where sender identity should be stored.
    /// Returns the event sequence number on success.
    pub async fn add_user_message_with_sender(
        &self,
        content: String,
        sender_id: Option<String>,
        sender_name: Option<String>,
    ) -> Result<u64, ActorError> {
        let seq = self
            .enqueue_user_message(content, sender_id, sender_name)
            .await?;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Enqueue a user message without forcing a flush.
    pub async fn enqueue_user_message(
        &self,
        content: String,
        sender_id: Option<String>,
        sender_name: Option<String>,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::AddUserMessage {
                content,
                sender_id,
                sender_name,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Add an assistant message to the session.
    ///
    /// Returns the event sequence number on success.
    pub async fn add_assistant_message(
        &self,
        content: String,
        usage: Option<Usage>,
    ) -> Result<u64, ActorError> {
        let seq = self.enqueue_assistant_message(content, usage).await?;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Enqueue an assistant message without forcing a flush.
    ///
    /// Useful for high-frequency internal events where batching is preferred.
    pub async fn enqueue_assistant_message(
        &self,
        content: String,
        usage: Option<Usage>,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::AddAssistantMessage {
                content,
                usage,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Add a silent message to session history, excluded from LLM conversation.
    ///
    /// Used for group messages from senders with `silent` disposition.
    /// Persisted in `events.jsonl` for audit but not included in `get_messages()`.
    /// Returns the event sequence number on success.
    pub async fn add_silent_message(
        &self,
        content: String,
        sender_id: String,
        sender_name: Option<String>,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::AddSilentMessage {
                content,
                sender_id,
                sender_name,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        let seq = reply_rx.await.map_err(|_| ActorError::ActorShutdown)??;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Record a tool call event.
    ///
    /// Returns the event sequence number on success.
    pub async fn record_tool_call(
        &self,
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    ) -> Result<u64, ActorError> {
        let seq = self
            .enqueue_tool_call(call_id, tool_name, arguments)
            .await?;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Enqueue a tool call event without forcing a flush.
    pub async fn enqueue_tool_call(
        &self,
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::RecordToolCall {
                call_id,
                tool_name,
                arguments,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Record a tool result event.
    ///
    /// Returns the event sequence number on success.
    pub async fn record_tool_result(
        &self,
        call_id: String,
        success: bool,
        content: String,
    ) -> Result<u64, ActorError> {
        let seq = self.enqueue_tool_result(call_id, success, content).await?;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Enqueue a tool result event without forcing a flush.
    pub async fn enqueue_tool_result(
        &self,
        call_id: String,
        success: bool,
        content: String,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::RecordToolResult {
                call_id,
                success,
                content,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Record an approval required event.
    ///
    /// Returns the event sequence number on success.
    pub async fn record_approval_required(
        &self,
        call_id: String,
        command: String,
    ) -> Result<u64, ActorError> {
        let seq = self.enqueue_approval_required(call_id, command).await?;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Enqueue an approval required event without forcing a flush.
    pub async fn enqueue_approval_required(
        &self,
        call_id: String,
        command: String,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::RecordApprovalRequired {
                call_id,
                command,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Record an approval decision event.
    ///
    /// Returns the event sequence number on success.
    pub async fn record_approval_decision(
        &self,
        call_id: String,
        decision: ApprovalDecisionType,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::RecordApprovalDecision {
                call_id,
                decision,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        let seq = reply_rx.await.map_err(|_| ActorError::ActorShutdown)??;
        self.force_flush().await?;
        Ok(seq)
    }

    /// Set a pending approval on the session.
    ///
    /// This persists immediately (flush + snapshot) for crash safety.
    pub async fn set_pending_approval(&self, pending: PendingApproval) -> Result<(), ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::SetPendingApproval {
                pending,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Clear the pending approval from the session.
    ///
    /// This persists immediately (flush + snapshot) for crash safety.
    pub async fn clear_pending_approval(&self) -> Result<(), ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::ClearPendingApproval { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Set the session status.
    ///
    /// This persists immediately (flush + snapshot) for consistency.
    pub async fn set_status(&self, status: SessionStatus) -> Result<(), ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::SetStatus {
                status,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Record an error event.
    ///
    /// Returns the event sequence number on success.
    pub async fn record_error(&self, code: String, message: String) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::RecordError {
                code,
                message,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        let seq = reply_rx.await.map_err(|_| ActorError::ActorShutdown)??;
        self.force_flush().await?;
        Ok(seq)
    }

    // ------------------------------------------------------------------------
    // Read Operations
    // ------------------------------------------------------------------------

    /// Get all messages in the session.
    pub async fn get_messages(&self) -> Result<Vec<Message>, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::GetMessages { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Get recent silent messages from the ephemeral buffer for context injection.
    ///
    /// Returns entries within `max_age` of now, limited to `max_messages`,
    /// in chronological order (oldest first).
    pub async fn get_recent_silent_messages(
        &self,
        max_messages: usize,
        max_age: chrono::Duration,
    ) -> Result<Vec<SilentMessageEntry>, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::GetRecentSilentMessages {
                max_messages,
                max_age,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Get session metadata.
    pub async fn get_metadata(&self) -> Result<SessionMetadata, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::GetMetadata { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Get the pending approval, if any.
    pub async fn get_pending_approval(&self) -> Result<Option<PendingApproval>, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::GetPendingApproval { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    // ------------------------------------------------------------------------
    // Stream/Flush Operations
    // ------------------------------------------------------------------------

    /// Finalize a stream by saving the accumulated content.
    ///
    /// This is called when a streaming response completes. It saves the
    /// assistant message and forces a snapshot.
    ///
    /// Returns the event sequence number on success.
    pub async fn finalize_stream(
        &self,
        content: String,
        usage: Option<Usage>,
    ) -> Result<u64, ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::FinalizeStream {
                content,
                usage,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Force an immediate flush of pending events to disk.
    pub async fn force_flush(&self) -> Result<(), ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::ForceFlush { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }

    /// Force an immediate flush and snapshot.
    pub async fn force_snapshot(&self) -> Result<(), ActorError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SessionCommand::ForceSnapshot { reply: reply_tx })
            .await
            .map_err(|_| ActorError::ActorShutdown)?;

        reply_rx.await.map_err(|_| ActorError::ActorShutdown)?
    }
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::OnDisconnect;
    use crate::session::actor::SessionActor;
    use crate::session::actor_types::{
        ActorConfig, DEFAULT_ACTOR_MESSAGE_LIMIT, DEFAULT_SILENT_BUFFER_CAP,
    };
    use crate::store::file::FileSessionStore;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::watch;

    fn create_test_handle(
        temp_dir: &TempDir,
    ) -> (
        SessionHandle,
        watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let store = Arc::new(FileSessionStore::new(temp_dir.path()));
        let config = ActorConfig {
            id: "session_test".to_string(),
            agent: "test-agent".to_string(),
            store,
            on_disconnect: OnDisconnect::Pause,
            gateway: None,
            gateway_chat_id: None,
            silent_buffer_cap: DEFAULT_SILENT_BUFFER_CAP,
            actor_message_limit: DEFAULT_ACTOR_MESSAGE_LIMIT,
        };
        let (tx, task_handle) = SessionActor::spawn(config, shutdown_rx);
        let handle = SessionHandle::new(tx, "session_test".to_string(), "test-agent".to_string());
        (handle, shutdown_tx, task_handle)
    }

    #[tokio::test]
    async fn handle_id_and_agent() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        assert_eq!(handle.id(), "session_test");
        assert_eq!(handle.agent(), "test-agent");

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn handle_add_and_get_messages() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        // Add messages
        handle.add_user_message("Hello".to_string()).await.unwrap();
        handle
            .add_assistant_message("Hi there!".to_string(), None)
            .await
            .unwrap();

        // Get messages
        let messages = handle.get_messages().await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content_str(), "Hello");
        assert_eq!(messages[1].content_str(), "Hi there!");

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn handle_get_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        let metadata = handle.get_metadata().await.unwrap();
        assert_eq!(metadata.id, "session_test");
        assert_eq!(metadata.agent, "test-agent");
        assert_eq!(metadata.status, SessionStatus::Active);

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn handle_set_status() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        handle.set_status(SessionStatus::Paused).await.unwrap();

        let metadata = handle.get_metadata().await.unwrap();
        assert_eq!(metadata.status, SessionStatus::Paused);

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn handle_pending_approval_flow() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        // Initially no pending approval
        let pending = handle.get_pending_approval().await.unwrap();
        assert!(pending.is_none());

        // Set pending approval
        let approval = PendingApproval::new(
            "call_123".to_string(),
            "bash".to_string(),
            serde_json::json!({"command": "ls"}),
            "ls".to_string(),
            vec![],
        );
        handle.set_pending_approval(approval).await.unwrap();

        // Verify it's set
        let pending = handle.get_pending_approval().await.unwrap();
        assert!(pending.is_some());
        assert_eq!(pending.unwrap().call_id, "call_123");

        // Clear it
        handle.clear_pending_approval().await.unwrap();

        // Verify it's gone
        let pending = handle.get_pending_approval().await.unwrap();
        assert!(pending.is_none());

        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    async fn handle_is_cloneable() {
        let temp_dir = TempDir::new().unwrap();
        let (handle, shutdown_tx, _task_handle) = create_test_handle(&temp_dir);

        let handle2 = handle.clone();

        // Both handles can add messages
        handle
            .add_user_message("From handle 1".to_string())
            .await
            .unwrap();
        handle2
            .add_user_message("From handle 2".to_string())
            .await
            .unwrap();

        let messages = handle.get_messages().await.unwrap();
        assert_eq!(messages.len(), 2);

        shutdown_tx.send(true).unwrap();
    }
}
