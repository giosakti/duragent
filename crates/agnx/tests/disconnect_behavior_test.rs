//! Integration tests for disconnect behavior (pause/continue modes).
//!
//! Tests the observable effects of on_disconnect settings:
//! - Pause mode: session status changes to Paused, snapshot written with Paused status
//! - Continue mode: session stays Running, events logged to JSONL, final snapshot with Active status
//!
//! These tests focus on the persistence layer: snapshots, event logging, and session resume.

use std::path::PathBuf;

use chrono::Utc;
use tempfile::TempDir;

use agnx::agent::OnDisconnect;
use agnx::api::SessionStatus;
use agnx::llm::{Message, Role, Usage};
use agnx::session::{
    EventReader, EventWriter, SessionConfig, SessionEvent, SessionEventPayload, SessionSnapshot,
    load_snapshot, resume_session, write_snapshot,
};

fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("sessions")
}

// ============================================================================
// Pause Mode Tests
// ============================================================================

/// Test that snapshot is written with Paused status on disconnect.
#[tokio::test]
async fn pause_mode_snapshot_written_with_paused_status() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "pause_snapshot_test";

    let conversation = vec![
        Message::text(Role::User, "Hello"),
        Message::text(Role::Assistant, "Hi there!"),
    ];

    // Simulate writing a snapshot when session is paused
    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Paused,
        Utc::now(),
        2,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Verify snapshot was written with Paused status
    let loaded = load_snapshot(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.status, SessionStatus::Paused);
    assert_eq!(loaded.config.on_disconnect, OnDisconnect::Pause);
    assert_eq!(loaded.conversation.len(), 2);
}

/// Test snapshot with partial conversation is written correctly.
#[tokio::test]
async fn pause_mode_snapshot_preserves_partial_conversation() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "partial_conversation";

    // Simulate partial conversation saved on disconnect
    let conversation = vec![
        Message::text(Role::User, "Explain async/await"),
        Message::text(Role::Assistant, "Async/await is a"), // Partial response
    ];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Paused,
        Utc::now(),
        2,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let loaded = load_snapshot(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.conversation.len(), 2);
    assert_eq!(loaded.conversation[1].content_str(), "Async/await is a");
    assert_eq!(loaded.status, SessionStatus::Paused);
}

// ============================================================================
// Continue Mode Tests
// ============================================================================

/// Test that session status is set to Running when background processing starts.
#[tokio::test]
async fn continue_mode_snapshot_written_with_running_status() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "continue_running_test";

    // Simulate writing a Running snapshot when continue mode background task starts
    let conversation = vec![Message::text(Role::User, "Hello")];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Running,
        Utc::now(),
        1,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let loaded = load_snapshot(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.status, SessionStatus::Running);
    assert_eq!(loaded.config.on_disconnect, OnDisconnect::Continue);
}

/// Test that events are logged to JSONL during background processing.
#[tokio::test]
async fn continue_mode_events_logged_during_background_processing() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "continue_events_test";

    // Simulate background task logging token events
    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Log streaming token events as they would be in continue mode
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Rust ".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "is ".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "great!".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    // Final event with usage
    writer
        .append(&SessionEvent::new(
            4,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: String::new(),
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                }),
            },
        ))
        .await
        .unwrap();

    // Read events back
    let mut reader = EventReader::open(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    let events = reader.read_all().await.unwrap();

    assert_eq!(events.len(), 4);
    match &events[0].payload {
        SessionEventPayload::AssistantMessage { content, .. } => {
            assert_eq!(content, "Rust ");
        }
        _ => panic!("expected AssistantMessage"),
    }
    match &events[3].payload {
        SessionEventPayload::AssistantMessage { usage, .. } => {
            assert!(usage.is_some());
            assert_eq!(usage.as_ref().unwrap().total_tokens, 15);
        }
        _ => panic!("expected AssistantMessage"),
    }
}

/// Test that final snapshot has Active status after background processing completes.
#[tokio::test]
async fn continue_mode_final_snapshot_has_active_status() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "continue_complete_test";

    // Simulate final snapshot after background task completes
    let conversation = vec![
        Message::text(Role::User, "Hello"),
        Message::text(Role::Assistant, "Hi there! How can I help you today?"),
    ];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        5,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    let loaded = load_snapshot(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(loaded.status, SessionStatus::Active);
    assert_eq!(loaded.config.on_disconnect, OnDisconnect::Continue);
    assert_eq!(loaded.conversation.len(), 2);
}

/// Test that error events are logged when background processing fails.
#[tokio::test]
async fn continue_mode_error_event_logged_on_failure() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "continue_error_test";

    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    // Simulate partial tokens before error
    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Let me ".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    // Simulate error during background processing
    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::Error {
                code: "timeout".to_string(),
                message: "Stream idle timeout in background".to_string(),
            },
        ))
        .await
        .unwrap();

    let mut reader = EventReader::open(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    let events = reader.read_all().await.unwrap();
    assert_eq!(events.len(), 2);

    match &events[1].payload {
        SessionEventPayload::Error { code, message } => {
            assert_eq!(code, "timeout");
            assert!(message.contains("timeout"));
        }
        _ => panic!("expected Error event"),
    }
}

// ============================================================================
// Attach After Disconnect Tests
// ============================================================================

/// Test that we can attach to a paused session.
#[tokio::test]
async fn attach_to_paused_session() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "attach_paused";

    // Simulate a session that was paused on disconnect
    let conversation = vec![
        Message::text(Role::User, "What is async?"),
        Message::text(Role::Assistant, "Async is a"), // Partial response when paused
    ];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Paused,
        Utc::now(),
        2,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Resume (attach) to the paused session
    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.status, SessionStatus::Paused);
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(resumed.conversation[0].content_str(), "What is async?");
    assert_eq!(resumed.conversation[1].content_str(), "Async is a");
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Pause);
}

/// Test that we can attach to a session that continued in background.
#[tokio::test]
async fn attach_to_session_continued_in_background() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "attach_continued";

    // Simulate a session that completed in background after disconnect
    let conversation = vec![
        Message::text(Role::User, "Write a poem"),
        Message::text(
            Role::Assistant,
            "Roses are red,\nViolets are blue,\nRust is great,\nAnd so are you!",
        ),
    ];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        5,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Resume (attach) to the session that was completed in background
    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.status, SessionStatus::Active);
    assert_eq!(resumed.conversation.len(), 2);
    assert!(
        resumed.conversation[1]
            .content_str()
            .contains("Roses are red")
    );
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Continue);
}

/// Test attaching to a session that was Running (background still processing).
#[tokio::test]
async fn attach_to_session_still_running_in_background() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "attach_running";

    // Simulate a session still running in background
    let conversation = vec![Message::text(Role::User, "Generate a long response")];

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Running,
        Utc::now(),
        1,
        conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // Add events that happened in background
    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Generate a long response".to_string(),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Here is a partial response so far...".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    // Resume - should see Running status and partial conversation
    let result = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    let resumed = result.unwrap();
    assert_eq!(resumed.status, SessionStatus::Running);
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(
        resumed.conversation[1].content_str(),
        "Here is a partial response so far..."
    );
}

// ============================================================================
// OnDisconnect Configuration Tests
// ============================================================================

/// Test that OnDisconnect::Pause is the default.
#[test]
fn on_disconnect_default_is_pause() {
    let config = SessionConfig::default();
    assert_eq!(config.on_disconnect, OnDisconnect::Pause);
}

/// Test OnDisconnect serialization roundtrip.
#[test]
fn on_disconnect_serialization_roundtrip() {
    let pause_config = SessionConfig {
        on_disconnect: OnDisconnect::Pause,
        ..Default::default()
    };
    let continue_config = SessionConfig {
        on_disconnect: OnDisconnect::Continue,
        ..Default::default()
    };

    // Serialize and deserialize via JSON
    let pause_json = serde_json::to_string(&pause_config).unwrap();
    let continue_json = serde_json::to_string(&continue_config).unwrap();

    assert!(pause_json.contains("pause"));
    assert!(continue_json.contains("continue"));

    let pause_loaded: SessionConfig = serde_json::from_str(&pause_json).unwrap();
    let continue_loaded: SessionConfig = serde_json::from_str(&continue_json).unwrap();

    assert_eq!(pause_loaded.on_disconnect, OnDisconnect::Pause);
    assert_eq!(continue_loaded.on_disconnect, OnDisconnect::Continue);
}

// ============================================================================
// Complex Scenario Tests
// ============================================================================

/// Test full continue mode scenario: create, message, disconnect, background, complete.
#[tokio::test]
async fn full_continue_mode_scenario() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "full_continue_scenario";

    // 1. Simulate session start with user message
    let initial_conversation = vec![Message::text(Role::User, "Write a haiku")];

    // 2. Write Running snapshot (disconnect happened, background task started)
    let running_snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Running,
        Utc::now(),
        1,
        initial_conversation.clone(),
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &running_snapshot)
        .await
        .unwrap();

    // 3. Simulate background task logging events
    let mut writer = EventWriter::new(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Write a haiku".to_string(),
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Code ".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            3,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "compiles ".to_string(),
                usage: None,
            },
        ))
        .await
        .unwrap();

    writer
        .append(&SessionEvent::new(
            4,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "without error\nJoy fills my heart today\nRust is beautiful".to_string(),
                usage: Some(Usage {
                    prompt_tokens: 15,
                    completion_tokens: 20,
                    total_tokens: 35,
                }),
            },
        ))
        .await
        .unwrap();

    // 4. Background task completes - write final Active snapshot
    let final_conversation = vec![
        Message::text(Role::User, "Write a haiku"),
        Message::text(
            Role::Assistant,
            "Code compiles without error\nJoy fills my heart today\nRust is beautiful",
        ),
    ];

    let final_snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "background-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        4,
        final_conversation,
        SessionConfig {
            on_disconnect: OnDisconnect::Continue,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &final_snapshot)
        .await
        .unwrap();

    // 5. Verify we can resume and see complete response
    let resumed = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resumed.status, SessionStatus::Active);
    assert_eq!(resumed.conversation.len(), 2);
    assert!(
        resumed.conversation[1]
            .content_str()
            .contains("Rust is beautiful")
    );
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Continue);
}

/// Test that multiple disconnects/reconnects work correctly.
#[tokio::test]
async fn multiple_disconnect_reconnect_cycles() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "multi_disconnect";

    // First cycle: create session, disconnect, pause
    let conversation_v1 = vec![
        Message::text(Role::User, "Hello"),
        Message::text(Role::Assistant, "Hi!"),
    ];

    let snapshot_v1 = SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Paused,
        Utc::now(),
        2,
        conversation_v1,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot_v1)
        .await
        .unwrap();

    // Resume and verify
    let resumed_v1 = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed_v1.conversation.len(), 2);
    assert_eq!(resumed_v1.status, SessionStatus::Paused);

    // Second cycle: continue conversation, disconnect again
    let conversation_v2 = vec![
        Message::text(Role::User, "Hello"),
        Message::text(Role::Assistant, "Hi!"),
        Message::text(Role::User, "How are you?"),
        Message::text(Role::Assistant, "I'm doing well!"),
    ];

    let snapshot_v2 = SessionSnapshot::new(
        session_id.to_string(),
        "test-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        4,
        conversation_v2,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot_v2)
        .await
        .unwrap();

    // Resume and verify full conversation preserved
    let resumed_v2 = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed_v2.conversation.len(), 4);
    assert_eq!(resumed_v2.conversation[3].content_str(), "I'm doing well!");
    assert_eq!(resumed_v2.status, SessionStatus::Active);
}
