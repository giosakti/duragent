//! Session management for Agnx.
//!
//! v0.1.0: In-memory session store.
//! v0.2.0: Persistent storage with JSONL event log + YAML snapshots.

mod event_reader;
mod event_writer;
mod events;
mod snapshot;
mod snapshot_loader;
mod snapshot_writer;

pub use event_reader::EventReader;
pub use event_writer::EventWriter;
pub use events::{
    SessionEndReason, SessionEvent, SessionEventPayload, SessionStatusValue, TokenUsage,
    ToolResultData,
};
pub use snapshot::{OnDisconnect, SessionConfig, SessionSnapshot, SnapshotStatus};
pub use snapshot_loader::load_snapshot;
pub use snapshot_writer::write_snapshot;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::llm::Message;

/// A conversation session with an agent.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
        }
    }
}

/// In-memory session store.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    /// Create a new empty session store.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session for the given agent.
    pub async fn create(&self, agent: String) -> Session {
        let now = Utc::now();
        let session = Session {
            id: format!("session_{}", Uuid::new_v4().simple()),
            agent,
            status: SessionStatus::Active,
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());
        session
    }

    /// Get a session by ID.
    pub async fn get(&self, id: &str) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Add a message to a session and update the timestamp.
    pub async fn add_message(&self, id: &str, message: Message) -> Option<()> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id)?;
        session.messages.push(message);
        session.updated_at = Utc::now();
        Some(())
    }

    /// Get all messages for a session.
    pub async fn get_messages(&self, id: &str) -> Option<Vec<Message>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).map(|s| s.messages.clone())
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
        let session = store.create("test-agent".to_string()).await;

        assert!(session.id.starts_with("session_"));
        assert_eq!(session.agent, "test-agent");
        assert_eq!(session.status, SessionStatus::Active);
        assert!(session.messages.is_empty());
    }

    #[tokio::test]
    async fn get_session() {
        let store = SessionStore::new();
        let session = store.create("test-agent".to_string()).await;

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
        let session = store.create("test-agent".to_string()).await;

        let msg = Message {
            role: Role::User,
            content: "Hello".to_string(),
        };

        let result = store.add_message(&session.id, msg).await;
        assert!(result.is_some());

        let messages = store.get_messages(&session.id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello");
        assert_eq!(messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn add_message_to_nonexistent_session() {
        let store = SessionStore::new();
        let msg = Message {
            role: Role::User,
            content: "Hello".to_string(),
        };

        let result = store.add_message("nonexistent", msg).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn session_status_display() {
        assert_eq!(SessionStatus::Active.to_string(), "active");
    }
}
