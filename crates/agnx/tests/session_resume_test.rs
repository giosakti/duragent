//! Integration tests for session resume functionality.
//!
//! Tests the `resume_session` function which combines snapshot loading
//! with event replay to restore sessions after crash/restart.

use std::path::PathBuf;

use chrono::Utc;
use tempfile::TempDir;

use agnx::agent::OnDisconnect;
use agnx::api::SessionStatus;
use agnx::llm::{Message, Role, Usage};
use agnx::session::{
    EventWriter, SessionConfig, SessionEvent, SessionEventPayload, SessionSnapshot, resume_session,
    write_snapshot,
};

fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("sessions")
}

fn create_test_snapshot(
    session_id: &str,
    last_event_seq: u64,
    conversation: Vec<Message>,
) -> SessionSnapshot {
    SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        last_event_seq,
        conversation,
        SessionConfig::default(),
    )
}

// ============================================================================
// Resume With Snapshot Only
// ============================================================================

#[tokio::test]
async fn resume_session_with_snapshot_only_no_events() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "snapshot_only";

    let conversation = vec![
        Message::text(Role::User, "Hello"),
        Message::text(Role::Assistant, "Hi there!"),
    ];

    let snapshot = create_test_snapshot(session_id, 2, conversation.clone());
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    assert!(result.is_some());
    let resumed = result.unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.agent, "test-agent");
    assert_eq!(resumed.status, SessionStatus::Active);
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.conversation[0].content_str(), "Hello");
    assert_eq!(resumed.conversation[1].content_str(), "Hi there!");
    assert_eq!(resumed.last_event_seq, 2);
}

// ============================================================================
// Resume With Snapshot + Events To Replay
// ============================================================================

#[tokio::test]
async fn resume_session_with_snapshot_and_events_to_replay() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "snapshot_with_events";

    // Snapshot at seq 2 with 2 messages
    let initial_conversation = vec![
        Message::text(Role::User, "First question"),
        Message::text(Role::Assistant, "First answer"),
    ];

    let snapshot = create_test_snapshot(session_id, 2, initial_conversation);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Write events including ones before snapshot (should be ignored) and after
    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Events 1-2 are in snapshot, will be skipped during replay
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "First question".to_string(),
            },
        ))
        .await
        .unwrap();
    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "resume-agent".to_string(),
                content: "First answer".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    // Events 3-4 are after snapshot, should be replayed
    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::UserMessage {
                content: "Second question".to_string(),
            },
        ))
        .await
        .unwrap();
    writer
        .append(&SessionEvent::new(
            4,
            SessionEventPayload::AssistantMessage {
                agent: "resume-agent".to_string(),
                content: "Second answer".to_string(),
                usage: Some(Usage {
                    prompt_tokens: 20,
                    completion_tokens: 15,
                    total_tokens: 35,
                }),
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 4);
    assert_eq!(resumed.conversation[0].content_str(), "First question");
    assert_eq!(resumed.conversation[1].content_str(), "First answer");
    assert_eq!(resumed.conversation[2].content_str(), "Second question");
    assert_eq!(resumed.conversation[3].content_str(), "Second answer");
    assert_eq!(resumed.last_event_seq, 4);
}

// ============================================================================
// Resume Rebuilds Conversation Correctly
// ============================================================================

#[tokio::test]
async fn resume_session_rebuilds_conversation_in_order() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "conversation_order";

    // Empty snapshot at start
    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Write conversation as events
    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let messages = [
        ("user", "Message 1"),
        ("assistant", "Reply 1"),
        ("user", "Message 2"),
        ("assistant", "Reply 2"),
        ("user", "Message 3"),
        ("assistant", "Reply 3"),
    ];

    for (i, (role, content)) in messages.iter().enumerate() {
        let payload = if *role == "user" {
            SessionEventPayload::UserMessage {
                content: content.to_string(),
            }
        } else {
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: content.to_string(),
                usage: None,
            }
        };
        writer
            .append(&SessionEvent::new((i + 1) as u64, payload))
            .await
            .unwrap();
    }

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 6);

    // Verify order and roles
    assert_eq!(resumed.conversation[0].role, Role::User);
    assert_eq!(resumed.conversation[0].content_str(), "Message 1");
    assert_eq!(resumed.conversation[1].role, Role::Assistant);
    assert_eq!(resumed.conversation[1].content_str(), "Reply 1");
    assert_eq!(resumed.conversation[2].role, Role::User);
    assert_eq!(resumed.conversation[2].content_str(), "Message 2");
    assert_eq!(resumed.conversation[3].role, Role::Assistant);
    assert_eq!(resumed.conversation[3].content_str(), "Reply 2");
    assert_eq!(resumed.conversation[4].role, Role::User);
    assert_eq!(resumed.conversation[4].content_str(), "Message 3");
    assert_eq!(resumed.conversation[5].role, Role::Assistant);
    assert_eq!(resumed.conversation[5].content_str(), "Reply 3");

    assert_eq!(resumed.last_event_seq, 6);
}

// ============================================================================
// Resume Handles Status Changes
// ============================================================================

#[tokio::test]
async fn resume_session_handles_status_change_to_paused() {
    use agnx::api::SessionStatus;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "status_change";

    // Start with active session
    let snapshot = create_test_snapshot(session_id, 1, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Write initial event (in snapshot)
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
            },
        ))
        .await
        .unwrap();

    // Status change after snapshot
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

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Paused);
    assert_eq!(resumed.last_event_seq, 2);
}

#[tokio::test]
async fn resume_session_handles_multiple_status_changes() {
    use agnx::api::SessionStatus;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "multi_status";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Active -> Paused
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Active,
                to: SessionStatus::Paused,
            },
        ))
        .await
        .unwrap();

    // Paused -> Running
    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Paused,
                to: SessionStatus::Running,
            },
        ))
        .await
        .unwrap();

    // Running -> Active
    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Running,
                to: SessionStatus::Active,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    // Final status should be Active (last status change)
    assert_eq!(resumed.status, SessionStatus::Active);
    assert_eq!(resumed.last_event_seq, 3);
}

// ============================================================================
// Resume Handles Session End Event
// ============================================================================

#[tokio::test]
async fn resume_session_handles_completed_session_end() {
    use agnx::session::SessionEndReason;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "session_completed";

    let snapshot = create_test_snapshot(session_id, 1, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Goodbye".to_string(),
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

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Completed);
}

#[tokio::test]
async fn resume_session_handles_terminated_session_end() {
    use agnx::session::SessionEndReason;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "session_terminated";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::SessionEnd {
                reason: SessionEndReason::Terminated,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Completed);
}

#[tokio::test]
async fn resume_session_handles_timeout_session_end() {
    use agnx::session::SessionEndReason;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "session_timeout";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::SessionEnd {
                reason: SessionEndReason::Timeout,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Completed);
}

#[tokio::test]
async fn resume_session_handles_error_session_end() {
    use agnx::session::SessionEndReason;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "session_error";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::SessionEnd {
                reason: SessionEndReason::Error,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Completed);
}

// ============================================================================
// Resume Non-Existent Session
// ============================================================================

#[tokio::test]
async fn resume_nonexistent_session_returns_none() {
    let temp_dir = TempDir::new().unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), "no_such_session")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn resume_from_nonexistent_directory_returns_none() {
    let temp_dir = TempDir::new().unwrap();
    let nonexistent = temp_dir.path().join("does_not_exist");

    let result = resume_session(&nonexistent, "any_session").await.unwrap();

    assert!(result.is_none());
}

// ============================================================================
// Resume With Corrupted/Missing Event File
// ============================================================================

#[tokio::test]
async fn resume_with_missing_event_file_uses_snapshot_only() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "no_events";

    // Write snapshot with conversation
    let conversation = vec![
        Message::text(Role::User, "Question"),
        Message::text(Role::Assistant, "Answer"),
    ];

    let snapshot = create_test_snapshot(session_id, 2, conversation);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // No events file created

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.conversation[0].content_str(), "Question");
    assert_eq!(resumed.conversation[1].content_str(), "Answer");
    assert_eq!(resumed.last_event_seq, 2);
}

#[tokio::test]
async fn resume_with_empty_event_file_uses_snapshot_only() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "empty_events";

    let conversation = vec![Message::text(Role::User, "Hello")];

    let snapshot = create_test_snapshot(session_id, 1, conversation);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Create empty events file
    let session_dir = sessions_dir(&temp_dir).join(session_id);
    let events_path = session_dir.join("events.jsonl");
    tokio::fs::write(&events_path, "").await.unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 1);
    assert_eq!(resumed.last_event_seq, 1);
}

#[tokio::test]
async fn resume_with_malformed_events_skips_bad_lines() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "malformed_events";

    // Snapshot at seq 0
    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Create events file with mix of valid and invalid lines
    let session_dir = sessions_dir(&temp_dir).join(session_id);

    let valid_event_1 = SessionEvent::new(
        1,
        SessionEventPayload::UserMessage {
            content: "Valid 1".to_string(),
        },
    );
    let valid_event_2 = SessionEvent::new(
        2,
        SessionEventPayload::AssistantMessage {
            agent: "test-agent".to_string(),
            content: "Valid 2".to_string(),
            usage: None,
        },
    );

    let content = format!(
        "{}\n\
         {{\"broken\n\
         not json\n\
         {}\n",
        serde_json::to_string(&valid_event_1).unwrap(),
        serde_json::to_string(&valid_event_2).unwrap(),
    );

    let events_path = session_dir.join("events.jsonl");
    tokio::fs::write(&events_path, content).await.unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    // Should have replayed the valid events
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.conversation[0].content_str(), "Valid 1");
    assert_eq!(resumed.conversation[1].content_str(), "Valid 2");
    assert_eq!(resumed.last_event_seq, 2);
}

// ============================================================================
// Resume Preserves on_disconnect Config
// ============================================================================

#[tokio::test]
async fn resume_preserves_on_disconnect_pause() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "config_pause";

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Paused,
        Utc::now(),
        0,
        vec![],
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Pause);
}

#[tokio::test]
async fn resume_preserves_on_disconnect_continue() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "config_continue";

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Running,
        Utc::now(),
        0,
        vec![],
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Continue);
}

// ============================================================================
// Resume Ignores Non-Message Events
// ============================================================================

#[tokio::test]
async fn resume_ignores_tool_call_and_result_events() {
    use agnx::session::ToolResultData;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "tool_events";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Search for Rust".to_string(),
            },
        ))
        .await
        .unwrap();

    // Tool events should not add to conversation
    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::ToolCall {
                call_id: "call_123".to_string(),
                tool_name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::ToolResult {
                call_id: "call_123".to_string(),
                result: ToolResultData {
                    success: true,
                    content: "Found results".to_string(),
                },
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            4,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Here are the results".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    // Only user and assistant messages in conversation
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.conversation[0].content_str(), "Search for Rust");
    assert_eq!(
        resumed.conversation[1].content_str(),
        "Here are the results"
    );
    assert_eq!(resumed.last_event_seq, 4);
}

#[tokio::test]
async fn resume_ignores_error_events() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "error_events";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
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

    // Error event should not add to conversation
    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::Error {
                code: "rate_limit".to_string(),
                message: "Too many requests".to_string(),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Hi there".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.last_event_seq, 3);
}

#[tokio::test]
async fn resume_ignores_session_start_events() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "session_start";

    let snapshot = create_test_snapshot(session_id, 0, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Session start doesn't add to conversation
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "test-agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 1);
    assert_eq!(resumed.conversation[0].content_str(), "Hello");
    assert_eq!(resumed.last_event_seq, 2);
}

// ============================================================================
// Resume Preserves Other Snapshot Fields
// ============================================================================

#[tokio::test]
async fn resume_preserves_agent_name() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "agent_name";

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "custom-agent-v2".to_string(),
        SessionStatus::Active,
        Utc::now(),
        0,
        vec![],
        SessionConfig::default(),
    );
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.agent, "custom-agent-v2");
}

#[tokio::test]
async fn resume_preserves_created_at_timestamp() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "timestamp";

    let created_at = Utc::now() - chrono::Duration::hours(24);
    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Active,
        created_at,
        0,
        vec![],
        SessionConfig::default(),
    );
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    // Check that the timestamp is preserved (within a second tolerance)
    let diff = (resumed.created_at - created_at).num_seconds().abs();
    assert!(diff < 1, "created_at should be preserved");
}

// ============================================================================
// Complex Scenario Tests
// ============================================================================

#[tokio::test]
async fn resume_after_crash_mid_conversation() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "crash_recovery";

    // Simulate: snapshot taken after seq 3, then more events written, then crash
    let snapshot_conversation = vec![
        Message::text(Role::User, "What is Rust?"),
        Message::text(Role::Assistant, "Rust is a systems programming language."),
    ];

    let snapshot = create_test_snapshot(session_id, 3, snapshot_conversation);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Events 1-3 are in snapshot
    for i in 1..=3 {
        writer
            .append(&SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Old {}", i),
                },
            ))
            .await
            .unwrap();
    }

    // Events 4-6 happened after snapshot
    writer
        .append(&SessionEvent::new(
            4,
            SessionEventPayload::UserMessage {
                content: "Can you give an example?".to_string(),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            5,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Sure! Here is a Hello World:".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            6,
            SessionEventPayload::UserMessage {
                content: "Thanks!".to_string(),
            },
        ))
        .await
        .unwrap();

    // Crash happens here - no more events or updated snapshot

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    // Should have: 2 from snapshot + 3 from replay (events 4, 5, 6)
    assert_eq!(resumed.conversation.len(), 5);
    assert_eq!(resumed.conversation[0].content_str(), "What is Rust?");
    assert_eq!(
        resumed.conversation[1].content_str(),
        "Rust is a systems programming language."
    );
    assert_eq!(
        resumed.conversation[2].content_str(),
        "Can you give an example?"
    );
    assert_eq!(
        resumed.conversation[3].content_str(),
        "Sure! Here is a Hello World:"
    );
    assert_eq!(resumed.conversation[4].content_str(), "Thanks!");
    assert_eq!(resumed.last_event_seq, 6);
}

#[tokio::test]
async fn resume_with_status_change_and_messages() {
    use agnx::api::SessionStatus;

    let temp_dir = TempDir::new().unwrap();
    let session_id = "mixed_events";

    let snapshot = create_test_snapshot(session_id, 1, vec![]);
    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Active,
                to: SessionStatus::Running,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            4,
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
            5,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Running,
                to: SessionStatus::Active,
            },
        ))
        .await
        .unwrap();

    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.status, SessionStatus::Active);
    assert_eq!(resumed.last_event_seq, 5);
}
