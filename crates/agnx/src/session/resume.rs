//! Session resume logic for restoring state after crash/restart.
//!
//! Combines snapshot loading with event replay to restore a session
//! to its exact state at the time of failure.

use std::path::Path;

use chrono::{DateTime, Utc};

use super::error::Result;
use super::{EventReader, SessionConfig, SessionEventPayload, SessionSnapshot, load_snapshot};
use crate::api::SessionStatus;
use crate::llm::Message;

/// A resumed session with its rebuilt state.
#[derive(Debug, Clone)]
pub struct ResumedSession {
    /// The session ID.
    pub session_id: String,
    /// The agent this session is using.
    pub agent: String,
    /// Current session status.
    pub status: SessionStatus,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// The rebuilt conversation history.
    pub conversation: Vec<Message>,
    /// Session configuration.
    pub config: SessionConfig,
    /// The sequence number of the last event processed.
    pub last_event_seq: u64,
}

/// Resume a session from disk by loading its snapshot and replaying events.
///
/// Returns `Ok(None)` if the session does not exist (no snapshot file).
/// Returns an error if the snapshot exists but cannot be loaded, or if
/// event replay fails.
///
/// # Arguments
/// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
/// * `session_id` - The session ID to resume
pub async fn resume_session(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<Option<ResumedSession>> {
    // Load the snapshot
    let snapshot = match load_snapshot(sessions_dir, session_id).await? {
        Some(s) => s,
        None => return Ok(None),
    };

    // Replay events from after the snapshot
    let (conversation, last_seq, status) =
        replay_events(sessions_dir, session_id, &snapshot).await?;

    Ok(Some(ResumedSession {
        session_id: snapshot.session_id,
        agent: snapshot.agent,
        status,
        created_at: snapshot.created_at,
        conversation,
        config: snapshot.config,
        last_event_seq: last_seq,
    }))
}

/// Replay events after the snapshot to rebuild conversation state.
///
/// Returns the rebuilt conversation, the last event sequence number processed,
/// and the final status (which may have changed due to status change events).
async fn replay_events(
    sessions_dir: &Path,
    session_id: &str,
    snapshot: &SessionSnapshot,
) -> Result<(Vec<Message>, u64, SessionStatus)> {
    let mut conversation = snapshot.conversation.clone();
    let mut last_seq = snapshot.last_event_seq;
    let mut status = snapshot.status;

    // Open the event reader; if no events file, just return snapshot state
    let reader = match EventReader::open(sessions_dir, session_id).await? {
        Some(r) => r,
        None => return Ok((conversation, last_seq, status)),
    };

    // Read events after the snapshot's last_event_seq
    let events = reader_read_from_seq(reader, snapshot.last_event_seq + 1).await?;

    for event in events {
        last_seq = event.seq;

        match &event.payload {
            SessionEventPayload::UserMessage { content } => {
                conversation.push(Message::text(crate::llm::Role::User, content));
            }
            SessionEventPayload::AssistantMessage { content, .. } => {
                conversation.push(Message::text(crate::llm::Role::Assistant, content));
            }
            SessionEventPayload::StatusChange { to, .. } => {
                status = *to;
            }
            SessionEventPayload::SessionEnd { reason } => {
                status = match reason {
                    super::SessionEndReason::Completed
                    | super::SessionEndReason::Terminated
                    | super::SessionEndReason::Timeout
                    | super::SessionEndReason::Error => SessionStatus::Completed,
                };
            }
            // Other events don't affect conversation or status
            SessionEventPayload::SessionStart { .. }
            | SessionEventPayload::ToolCall { .. }
            | SessionEventPayload::ToolResult { .. }
            | SessionEventPayload::ApprovalRequired { .. }
            | SessionEventPayload::ApprovalDecision { .. }
            | SessionEventPayload::Error { .. } => {}
        }
    }

    Ok((conversation, last_seq, status))
}

/// Helper to read events from a sequence number.
/// Consumes the reader since EventReader doesn't implement Clone.
async fn reader_read_from_seq(
    mut reader: EventReader,
    start_seq: u64,
) -> Result<Vec<super::SessionEvent>> {
    reader.read_from_seq(start_seq).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::OnDisconnect;
    use crate::llm::{Role, Usage};
    use crate::session::{
        EventWriter, SessionEndReason, SessionEvent, SessionEventPayload, ToolResultData,
    };
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::fs;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    fn create_test_snapshot(session_id: &str, last_event_seq: u64) -> SessionSnapshot {
        SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            last_event_seq,
            vec![
                Message::text(Role::User, "Hello"),
                Message::text(Role::Assistant, "Hi there!"),
            ],
            SessionConfig::default(),
        )
    }

    async fn write_test_snapshot(temp_dir: &TempDir, session_id: &str, snapshot: &SessionSnapshot) {
        let session_dir = sessions_dir(temp_dir).join(session_id);
        fs::create_dir_all(&session_dir).await.unwrap();

        let yaml = serde_saphyr::to_string(snapshot).unwrap();
        let path = session_dir.join("state.yaml");
        fs::write(&path, yaml).await.unwrap();
    }

    #[tokio::test]
    async fn resume_nonexistent_session_returns_none() {
        let temp_dir = TempDir::new().unwrap();

        let result = resume_session(&sessions_dir(&temp_dir), "no_such_session").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn resume_session_with_snapshot_only() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = create_test_snapshot("session_abc", 2);
        write_test_snapshot(&temp_dir, "session_abc", &snapshot).await;

        let result = resume_session(&sessions_dir(&temp_dir), "session_abc")
            .await
            .unwrap();

        assert!(result.is_some());
        let resumed = result.unwrap();
        assert_eq!(resumed.session_id, "session_abc");
        assert_eq!(resumed.agent, "test-agent");
        assert_eq!(resumed.status, SessionStatus::Active);
        assert_eq!(resumed.conversation.len(), 2);
        assert_eq!(resumed.last_event_seq, 2);
    }

    #[tokio::test]
    async fn resume_session_replays_events_after_snapshot() {
        let temp_dir = TempDir::new().unwrap();

        // Create snapshot at seq 2
        let snapshot = create_test_snapshot("session_replay", 2);
        write_test_snapshot(&temp_dir, "session_replay", &snapshot).await;

        // Write events including ones before and after snapshot
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "session_replay")
            .await
            .unwrap();

        // Events 1-2 are already in snapshot, write them anyway
        writer
            .append(&SessionEvent::new(
                1,
                SessionEventPayload::UserMessage {
                    content: "Hello".to_string(),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                2,
                SessionEventPayload::AssistantMessage {
                    agent: "test-agent".to_string(),
                    content: "Hi there!".to_string(),
                    usage: None,
                },
            ))
            .await
            .unwrap();

        // Events 3-4 should be replayed
        writer
            .append(&SessionEvent::new(
                3,
                SessionEventPayload::UserMessage {
                    content: "How are you?".to_string(),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                4,
                SessionEventPayload::AssistantMessage {
                    agent: "test-agent".to_string(),
                    content: "I'm doing great!".to_string(),
                    usage: Some(Usage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                        total_tokens: 30,
                    }),
                },
            ))
            .await
            .unwrap();

        let result = resume_session(&sessions_dir(&temp_dir), "session_replay")
            .await
            .unwrap();

        let resumed = result.unwrap();
        assert_eq!(resumed.conversation.len(), 4);
        assert_eq!(resumed.conversation[0].content_str(), "Hello");
        assert_eq!(resumed.conversation[1].content_str(), "Hi there!");
        assert_eq!(resumed.conversation[2].content_str(), "How are you?");
        assert_eq!(resumed.conversation[3].content_str(), "I'm doing great!");
        assert_eq!(resumed.last_event_seq, 4);
    }

    #[tokio::test]
    async fn resume_session_handles_status_change() {
        let temp_dir = TempDir::new().unwrap();

        // Create active snapshot
        let snapshot = create_test_snapshot("session_status", 1);
        write_test_snapshot(&temp_dir, "session_status", &snapshot).await;

        // Write status change event
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "session_status")
            .await
            .unwrap();

        writer
            .append(&SessionEvent::new(
                1,
                SessionEventPayload::UserMessage {
                    content: "Hello".to_string(),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                2,
                SessionEventPayload::StatusChange {
                    from: SessionStatus::Active,
                    to: SessionStatus::Paused,
                },
            ))
            .await
            .unwrap();

        let result = resume_session(&sessions_dir(&temp_dir), "session_status")
            .await
            .unwrap();

        let resumed = result.unwrap();
        assert_eq!(resumed.status, SessionStatus::Paused);
        assert_eq!(resumed.last_event_seq, 2);
    }

    #[tokio::test]
    async fn resume_session_handles_session_end() {
        let temp_dir = TempDir::new().unwrap();

        let snapshot = create_test_snapshot("session_end", 1);
        write_test_snapshot(&temp_dir, "session_end", &snapshot).await;

        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "session_end")
            .await
            .unwrap();

        writer
            .append(&SessionEvent::new(
                1,
                SessionEventPayload::UserMessage {
                    content: "Hello".to_string(),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                2,
                SessionEventPayload::SessionEnd {
                    reason: SessionEndReason::Completed,
                },
            ))
            .await
            .unwrap();

        let result = resume_session(&sessions_dir(&temp_dir), "session_end")
            .await
            .unwrap();

        let resumed = result.unwrap();
        assert_eq!(resumed.status, SessionStatus::Completed);
    }

    #[tokio::test]
    async fn resume_session_ignores_non_message_events() {
        let temp_dir = TempDir::new().unwrap();

        let snapshot = create_test_snapshot("session_tools", 2);
        write_test_snapshot(&temp_dir, "session_tools", &snapshot).await;

        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "session_tools")
            .await
            .unwrap();

        // Write events including tool calls (which don't affect conversation)
        writer
            .append(&SessionEvent::new(
                1,
                SessionEventPayload::UserMessage {
                    content: "Hello".to_string(),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                2,
                SessionEventPayload::AssistantMessage {
                    agent: "test-agent".to_string(),
                    content: "Hi!".to_string(),
                    usage: None,
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                3,
                SessionEventPayload::ToolCall {
                    call_id: "call_123".to_string(),
                    tool_name: "search".to_string(),
                    arguments: serde_json::json!({"q": "test"}),
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                4,
                SessionEventPayload::ToolResult {
                    call_id: "call_123".to_string(),
                    result: ToolResultData {
                        success: true,
                        content: "found it".to_string(),
                    },
                },
            ))
            .await
            .unwrap();
        writer
            .append(&SessionEvent::new(
                5,
                SessionEventPayload::UserMessage {
                    content: "Thanks!".to_string(),
                },
            ))
            .await
            .unwrap();

        let result = resume_session(&sessions_dir(&temp_dir), "session_tools")
            .await
            .unwrap();

        let resumed = result.unwrap();
        // Snapshot had 2 messages, replay adds 1 more (seq 5)
        // Tool events at seq 3 and 4 don't add messages
        assert_eq!(resumed.conversation.len(), 3);
        assert_eq!(resumed.conversation[2].content_str(), "Thanks!");
        assert_eq!(resumed.last_event_seq, 5);
    }

    #[tokio::test]
    async fn resume_preserves_config() {
        let temp_dir = TempDir::new().unwrap();

        let mut snapshot = create_test_snapshot("session_config", 1);
        snapshot.config = SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        };
        write_test_snapshot(&temp_dir, "session_config", &snapshot).await;

        let result = resume_session(&sessions_dir(&temp_dir), "session_config")
            .await
            .unwrap();

        let resumed = result.unwrap();
        assert_eq!(resumed.config.on_disconnect, OnDisconnect::Continue);
    }
}
