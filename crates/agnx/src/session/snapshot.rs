//! Session snapshot schema for fast resume.
//!
//! Snapshots are written as YAML files and contain the complete session state
//! at a point in time. Combined with the event log, they enable fast resume
//! without replaying the entire event history.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::Message;

/// Default approval timeout in seconds.
pub const APPROVAL_TIMEOUT_SECONDS: i64 = 300;

/// A pending approval waiting for user decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    /// The tool call ID that needs approval.
    pub call_id: String,
    /// The tool name (e.g., "bash").
    pub tool_name: String,
    /// The tool call arguments.
    pub arguments: serde_json::Value,
    /// The command being approved (for display).
    pub command: String,
    /// When the approval was requested.
    pub created_at: DateTime<Utc>,
    /// When the approval expires.
    pub expires_at: DateTime<Utc>,
    /// Accumulated messages to restore when resuming the loop.
    pub messages: Vec<Message>,
}

impl PendingApproval {
    /// Create a new pending approval with default timeout.
    pub fn new(
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        command: String,
        messages: Vec<Message>,
    ) -> Self {
        let now = Utc::now();
        Self {
            call_id,
            tool_name,
            arguments,
            command,
            created_at: now,
            expires_at: now + Duration::seconds(APPROVAL_TIMEOUT_SECONDS),
            messages,
        }
    }

    /// Check if this approval has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
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
    /// The conversation history.
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
}

impl SessionSnapshot {
    /// Current schema version.
    pub const SCHEMA_VERSION: &'static str = "1";

    /// Create a new snapshot from session state.
    #[must_use]
    pub fn new(
        session_id: String,
        agent: String,
        status: SessionStatus,
        created_at: DateTime<Utc>,
        last_event_seq: u64,
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
            conversation,
            config,
        }
    }

    /// Check if this snapshot is compatible with the current schema.
    pub fn is_compatible(&self) -> bool {
        self.schema_version == "1"
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

        let parsed: SessionSnapshot = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.session_id, "session_abc123");
        assert_eq!(parsed.agent, "my-agent");
        assert_eq!(parsed.status, SessionStatus::Active);
        assert_eq!(parsed.last_event_seq, 42);
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
            vec![],
            SessionConfig::default(),
        );
        assert!(snapshot.is_compatible());
        assert_eq!(snapshot.schema_version, "1");

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
            vec![Message::text(Role::User, "run ls")],
        );

        let yaml = serde_saphyr::to_string(&pending).unwrap();
        assert!(yaml.contains("call_id: call_123"));
        assert!(yaml.contains("tool_name: bash"));

        let parsed: PendingApproval = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(parsed.call_id, "call_123");
        assert_eq!(parsed.tool_name, "bash");
        assert_eq!(parsed.command, "ls -la");
        assert!(!parsed.is_expired());
    }

    #[test]
    fn pending_approval_expiry() {
        let mut pending = PendingApproval::new(
            "call_123".to_string(),
            "bash".to_string(),
            serde_json::json!({}),
            "ls".to_string(),
            vec![],
        );

        assert!(!pending.is_expired());

        // Set expires_at to the past
        pending.expires_at = Utc::now() - Duration::minutes(1);
        assert!(pending.is_expired());
    }
}
