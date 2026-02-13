//! Session snapshot schema for fast resume.
//!
//! Snapshots are written as YAML files and contain the complete session state
//! at a point in time. Combined with the event log, they enable fast resume
//! without replaying the entire event history.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::Message;

use super::agentic_loop::PendingApproval;

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
    ///
    /// Messages in `conversation` are stable up to this sequence.
    /// Messages after this sequence are reconstructed from events on recovery.
    /// Defaults to `last_event_seq` for v1 snapshots (full conversation stored).
    #[serde(default)]
    pub checkpoint_seq: u64,
    /// The conversation history (up to checkpoint_seq).
    ///
    /// In schema v2+, this contains only checkpointed messages.
    /// Messages after checkpoint_seq are replayed from events.
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

impl SessionSnapshot {
    /// Current schema version.
    pub const SCHEMA_VERSION: &'static str = "2";

    /// Create a new snapshot from session state.
    ///
    /// This creates a v2 snapshot with checkpoint-based storage.
    /// `checkpoint_seq` indicates the sequence up to which messages are stored.
    /// `conversation` should contain only messages up to that checkpoint.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: String,
        agent: String,
        status: SessionStatus,
        created_at: DateTime<Utc>,
        last_event_seq: u64,
        checkpoint_seq: u64,
        conversation: Vec<Message>,
        config: SessionConfig,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            session_id,
            agent,
            status,
            created_at,
            snapshot_at: Utc::now(),
            last_event_seq,
            checkpoint_seq,
            conversation,
            config,
        }
    }

    /// Check if this snapshot is compatible with the current schema.
    pub fn is_compatible(&self) -> bool {
        matches!(self.schema_version.as_str(), "1" | "2")
    }

    /// Get the sequence from which to replay events.
    ///
    /// For v1 snapshots, this is `last_event_seq` (no replay needed).
    /// For v2 snapshots, this is `checkpoint_seq` (replay events after checkpoint).
    pub fn replay_from_seq(&self) -> u64 {
        if self.checkpoint_seq == 0 {
            // v1 snapshot: checkpoint_seq defaults to 0, replay from last_event_seq
            self.last_event_seq
        } else {
            self.checkpoint_seq
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[test]
    fn snapshot_serialization_roundtrip() {
        let snapshot = SessionSnapshot::new(
            "session_abc123".to_string(),
            "my-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            42,
            42, // checkpoint_seq matches last_event_seq
            vec![
                Message::text(Role::User, "Hello"),
                Message::text(Role::Assistant, "Hi there!"),
            ],
            SessionConfig::default(),
        );

        let yaml = serde_saphyr::to_string(&snapshot).unwrap();
        assert!(yaml.contains("session_id: session_abc123"));
        assert!(yaml.contains("status: active"));
        assert!(yaml.contains("last_event_seq: 42"));
        assert!(yaml.contains("checkpoint_seq: 42"));

        let parsed: SessionSnapshot = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.session_id, "session_abc123");
        assert_eq!(parsed.agent, "my-agent");
        assert_eq!(parsed.status, SessionStatus::Active);
        assert_eq!(parsed.last_event_seq, 42);
        assert_eq!(parsed.checkpoint_seq, 42);
        assert_eq!(parsed.conversation.len(), 2);
    }

    #[test]
    fn snapshot_with_continue_mode() {
        let snapshot = SessionSnapshot::new(
            "session_xyz".to_string(),
            "background-agent".to_string(),
            SessionStatus::Running,
            Utc::now(),
            100,
            100,
            vec![],
            SessionConfig {
                on_disconnect: OnDisconnect::Continue,
                ..Default::default()
            },
        );

        let yaml = serde_saphyr::to_string(&snapshot).unwrap();
        assert!(yaml.contains("on_disconnect: continue"));

        let parsed: SessionSnapshot = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.config.on_disconnect, OnDisconnect::Continue);
    }

    #[test]
    fn snapshot_status_values() {
        assert_eq!(
            serde_json::to_string(&SessionStatus::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&SessionStatus::Paused).unwrap(),
            "\"paused\""
        );
        assert_eq!(
            serde_json::to_string(&SessionStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&SessionStatus::Completed).unwrap(),
            "\"completed\""
        );
    }

    #[test]
    fn on_disconnect_default() {
        let config = SessionConfig::default();
        assert_eq!(config.on_disconnect, OnDisconnect::Pause);
    }

    #[test]
    fn schema_version_check() {
        let snapshot = SessionSnapshot::new(
            "s".to_string(),
            "a".to_string(),
            SessionStatus::Active,
            Utc::now(),
            0,
            0,
            vec![],
            SessionConfig::default(),
        );
        assert!(snapshot.is_compatible());
        assert_eq!(snapshot.schema_version, "2");

        // v1 snapshots are also compatible
        let mut v1_snapshot = snapshot.clone();
        v1_snapshot.schema_version = "1".to_string();
        assert!(v1_snapshot.is_compatible());

        // Other versions are not compatible
        let mut old_snapshot = snapshot.clone();
        old_snapshot.schema_version = "0".to_string();
        assert!(!old_snapshot.is_compatible());
    }

    #[test]
    fn pending_approval_serialization() {
        let pending = PendingApproval::new(
            "call_123".to_string(),
            "bash".to_string(),
            serde_json::json!({"command": "ls -la"}),
            "ls -la".to_string(),
            crate::agent::ToolType::Bash,
            vec![Message::text(Role::User, "run ls")],
        );

        let yaml = serde_saphyr::to_string(&pending).unwrap();
        assert!(yaml.contains("call_id: call_123"));
        assert!(yaml.contains("tool_name: bash"));

        let parsed: PendingApproval = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.call_id, "call_123");
        assert_eq!(parsed.tool_name, "bash");
        assert_eq!(parsed.command, "ls -la");
    }

    #[test]
    fn replay_from_seq_v1_snapshot() {
        // v1 snapshots have checkpoint_seq defaulted to 0
        // replay_from_seq should return last_event_seq
        let yaml = r#"
schema_version: "1"
session_id: old_session
agent: agent
status: active
created_at: 2024-01-01T00:00:00Z
snapshot_at: 2024-01-01T00:00:00Z
last_event_seq: 100
conversation: []
config:
  on_disconnect: pause
"#;
        let snapshot: SessionSnapshot = serde_saphyr::from_str(yaml).unwrap();

        // v1 snapshots should not replay (checkpoint_seq defaults to 0)
        assert_eq!(snapshot.checkpoint_seq, 0);
        assert_eq!(snapshot.replay_from_seq(), 100); // Returns last_event_seq
    }

    #[test]
    fn replay_from_seq_v2_snapshot() {
        // v2 snapshots have explicit checkpoint_seq
        let snapshot = SessionSnapshot::new(
            "session".to_string(),
            "agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            100, // last_event_seq
            50,  // checkpoint_seq (checkpoint is behind)
            vec![],
            SessionConfig::default(),
        );

        // v2 snapshots should replay from checkpoint_seq
        assert_eq!(snapshot.checkpoint_seq, 50);
        assert_eq!(snapshot.replay_from_seq(), 50);
    }

    #[test]
    fn checkpoint_seq_serialization() {
        let snapshot = SessionSnapshot::new(
            "s".to_string(),
            "a".to_string(),
            SessionStatus::Active,
            Utc::now(),
            100,
            75,
            vec![],
            SessionConfig::default(),
        );

        let yaml = serde_saphyr::to_string(&snapshot).unwrap();
        assert!(yaml.contains("checkpoint_seq: 75"));
        assert!(yaml.contains("last_event_seq: 100"));

        let parsed: SessionSnapshot = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.checkpoint_seq, 75);
        assert_eq!(parsed.last_event_seq, 100);
    }
}
