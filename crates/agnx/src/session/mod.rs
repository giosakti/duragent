//! Session management for Agnx.
//!
//! v0.1.0: In-memory session store.
//! v0.2.0: Persistent storage with JSONL event log + YAML snapshots.
//! v0.4.0: Agentic loop for tool-using agents.

mod agentic;
mod chat_request;
mod error;
mod event_reader;
mod event_writer;
mod events;
mod persist;
mod recover;
mod resume;
mod snapshot;
mod snapshot_loader;
mod snapshot_writer;
mod stream;

// Types and errors
pub use error::{Result, SessionError};
pub use events::{
    ApprovalDecisionType, SessionEndReason, SessionEvent, SessionEventPayload, ToolResultData,
};
pub use snapshot::{APPROVAL_TIMEOUT_SECONDS, PendingApproval, SessionConfig, SessionSnapshot};

// Event I/O
pub use event_reader::EventReader;
pub use event_writer::EventWriter;
pub use snapshot_loader::load_snapshot;
pub use snapshot_writer::write_snapshot;

// Persistence and recovery
pub use persist::{
    SessionContext, clear_pending_approval, commit_event, get_pending_approval,
    persist_assistant_message, record_event, set_pending_approval, write_session_snapshot,
};
pub use recover::{RecoveryResult, recover_sessions};
pub use resume::{ResumedSession, resume_session};

// Chat and streaming
pub use chat_request::{build_chat_messages, build_chat_request, build_system_message};
pub use stream::{AccumulatingStream, StreamConfig};

// Agentic loop
pub use agentic::{
    AgenticError, AgenticResult, EventContext, resume_agentic_loop, run_agentic_loop,
};

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::api::SessionStatus;
use crate::llm::Message;

/// A conversation session with an agent.
///
/// Note: Messages are stored separately in `SessionStore` to avoid O(n) clones
/// on every mutation. Use `SessionStore::get_messages()` to retrieve them.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_event_seq: u64,
}

/// In-memory session store.
///
/// Uses `Arc<Session>` for cheap clones on read operations.
/// Messages are stored separately to avoid O(n) clones on every mutation.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Arc<Session>>>>,
    messages: Arc<RwLock<HashMap<String, Vec<Message>>>>,
}

impl SessionStore {
    // ----------------------------------------------------------------------------
    // Constructor
    // ----------------------------------------------------------------------------

    /// Create a new empty session store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ----------------------------------------------------------------------------
    // Private Helpers
    // ----------------------------------------------------------------------------

    /// Update a session using clone-modify-insert pattern.
    ///
    /// The closure receives a mutable reference to a cloned session and can
    /// return a value. The modified session replaces the original in the map.
    async fn update<F, T>(&self, id: &str, f: F) -> Result<T>
    where
        F: FnOnce(&mut Session) -> T,
    {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound(id.to_string()))?;

        let mut updated = (**session).clone();
        let result = f(&mut updated);
        sessions.insert(id.to_string(), Arc::new(updated));
        Ok(result)
    }

    // ----------------------------------------------------------------------------
    // Session CRUD
    // ----------------------------------------------------------------------------

    /// Create a new session for the given agent.
    ///
    /// Returns an `Arc<Session>` for cheap sharing.
    pub async fn create(&self, agent: &str) -> Arc<Session> {
        let now = Utc::now();
        let session = Arc::new(Session {
            id: format!(
                "{}{}",
                crate::api::SESSION_ID_PREFIX,
                Uuid::new_v4().simple()
            ),
            agent: agent.to_string(),
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_event_seq: 0,
        });

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.id.clone(), Arc::clone(&session));
        }

        let mut messages = self.messages.write().await;
        messages.insert(session.id.clone(), Vec::new());

        session
    }

    /// Get a session by ID.
    ///
    /// Returns an `Arc<Session>` for cheap cloning (O(1) reference count bump).
    pub async fn get(&self, id: &str) -> Option<Arc<Session>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// List all sessions.
    ///
    /// Returns cloned Session values (not Arc) for simpler API.
    pub async fn list(&self) -> Vec<Session> {
        let sessions = self.sessions.read().await;
        sessions.values().map(|s| (**s).clone()).collect()
    }

    /// Update a session's status.
    pub async fn set_status(&self, id: &str, status: SessionStatus) -> Result<()> {
        self.update(id, |s| {
            s.status = status;
            s.updated_at = Utc::now();
        })
        .await
    }

    /// Register an existing session (e.g., recovered from disk).
    ///
    /// This is used during server startup to restore sessions from snapshots.
    /// Messages are passed separately to maintain the split storage model.
    pub async fn register(&self, session: Session, session_messages: Vec<Message>) {
        let session_id = session.id.clone();

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), Arc::new(session));
        }

        let mut messages = self.messages.write().await;
        messages.insert(session_id, session_messages);
    }

    // ----------------------------------------------------------------------------
    // Messages
    // ----------------------------------------------------------------------------

    /// Add a message to a session and update the timestamp.
    ///
    /// This is O(1) amortized - messages are stored separately to avoid cloning.
    pub async fn add_message(&self, id: &str, message: Message) -> Result<()> {
        // Verify session exists and update timestamp
        self.update(id, |s| s.updated_at = Utc::now()).await?;

        // Append message to separate storage (O(1) amortized)
        let mut messages = self.messages.write().await;
        messages.entry(id.to_string()).or_default().push(message);

        Ok(())
    }

    /// Get all messages for a session.
    pub async fn get_messages(&self, id: &str) -> Option<Vec<Message>> {
        let messages = self.messages.read().await;
        messages.get(id).cloned()
    }

    // ----------------------------------------------------------------------------
    // Event Sequence
    // ----------------------------------------------------------------------------

    /// Get the last event sequence number for a session.
    pub async fn last_event_seq(&self, id: &str) -> Result<u64> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .map(|s| s.last_event_seq)
            .ok_or_else(|| SessionError::NotFound(id.to_string()))
    }

    /// Increment and return the next event sequence number for a session.
    ///
    /// **Warning:** This increments in-memory state immediately. For safer persistence,
    /// use `peek_next_event_seq` + `commit_event_seq` pattern to avoid drift on write failure.
    pub async fn next_event_seq(&self, id: &str) -> Result<u64> {
        self.update(id, |s| {
            s.last_event_seq += 1;
            s.last_event_seq
        })
        .await
    }

    /// Get the next event sequence number without incrementing.
    ///
    /// Use with `commit_event_seq` for safe persistence: peek the value, write to disk,
    /// then commit only on success. This avoids drift if the disk write fails.
    pub async fn peek_next_event_seq(&self, id: &str) -> Result<u64> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .map(|s| s.last_event_seq + 1)
            .ok_or_else(|| SessionError::NotFound(id.to_string()))
    }

    /// Commit an event sequence number after successful persistence.
    ///
    /// Only updates in-memory state if `new_seq` is greater than the current value.
    /// This ensures we don't accidentally go backwards.
    pub async fn commit_event_seq(&self, id: &str, new_seq: u64) -> Result<()> {
        self.update(id, |s| {
            if new_seq > s.last_event_seq {
                s.last_event_seq = new_seq;
            }
        })
        .await
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[tokio::test]
    async fn create_session() {
        let store = SessionStore::new();
        let session = store.create("test-agent").await;

        assert!(session.id.starts_with("session_"));
        assert_eq!(session.agent, "test-agent");
        assert_eq!(session.status, SessionStatus::Active);
        assert_eq!(session.last_event_seq, 0);

        let messages = store.get_messages(&session.id).await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn get_session() {
        let store = SessionStore::new();
        let session = store.create("test-agent").await;

        let fetched = store.get(&session.id).await;
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, session.id);
        assert_eq!(fetched.agent, "test-agent");
    }

    #[tokio::test]
    async fn get_nonexistent_session() {
        let store = SessionStore::new();
        let fetched = store.get("nonexistent").await;
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn add_message() {
        let store = SessionStore::new();
        let session = store.create("test-agent").await;

        let msg = Message::text(Role::User, "Hello");

        let result = store.add_message(&session.id, msg).await;
        assert!(result.is_ok());

        let messages = store.get_messages(&session.id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content_str(), "Hello");
        assert_eq!(messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn add_message_to_nonexistent_session() {
        let store = SessionStore::new();
        let msg = Message::text(Role::User, "Hello");

        let result = store.add_message("nonexistent", msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn session_status_display() {
        assert_eq!(SessionStatus::Active.to_string(), "active");
        assert_eq!(SessionStatus::Paused.to_string(), "paused");
    }

    #[tokio::test]
    async fn set_session_status() {
        let store = SessionStore::new();
        let session = store.create("test-agent").await;

        assert_eq!(session.status, SessionStatus::Active);

        let result = store.set_status(&session.id, SessionStatus::Paused).await;
        assert!(result.is_ok());

        let updated = store.get(&session.id).await.unwrap();
        assert_eq!(updated.status, SessionStatus::Paused);
    }

    #[tokio::test]
    async fn set_status_nonexistent_session() {
        let store = SessionStore::new();
        let result = store.set_status("nonexistent", SessionStatus::Paused).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn register_session() {
        let store = SessionStore::new();
        let now = Utc::now();

        let session = Session {
            id: "session_recovered123".to_string(),
            agent: "recovered-agent".to_string(),
            status: SessionStatus::Paused,
            created_at: now,
            updated_at: now,
            last_event_seq: 7,
        };
        let messages = vec![Message::text(Role::User, "Previous message")];

        store.register(session, messages).await;

        let fetched = store.get("session_recovered123").await;
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, "session_recovered123");
        assert_eq!(fetched.agent, "recovered-agent");
        assert_eq!(fetched.status, SessionStatus::Paused);
        assert_eq!(fetched.last_event_seq, 7);

        let fetched_messages = store.get_messages("session_recovered123").await.unwrap();
        assert_eq!(fetched_messages.len(), 1);
        assert_eq!(fetched_messages[0].content_str(), "Previous message");
    }
}
