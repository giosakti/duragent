//! Integration tests for disconnect behavior (pause/continue modes).
//!
//! Tests the observable effects of on_disconnect settings:
//! - Pause mode: session status changes to Paused, snapshot written with Paused status
//! - Continue mode: session stays Running, events logged to JSONL, final snapshot with Active status
//!
//! Note: These tests simulate the outcomes of disconnect scenarios by directly
//! testing the session store, snapshot, and event persistence layer, since
//! actual SSE disconnect testing would require end-to-end server tests.

use std::path::PathBuf;

use chrono::Utc;
use tempfile::TempDir;

use agnx::agent::OnDisconnect;
use agnx::api::SessionStatus;
use agnx::llm::{Message, Role, Usage};
use agnx::session::{
    EventReader, EventWriter, SessionConfig, SessionEvent, SessionEventPayload, SessionSnapshot,
    SessionStore, load_snapshot, resume_session, write_snapshot,
};

fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("sessions")
}

// ============================================================================
// Pause Mode Tests
// ============================================================================

/// Test that session status can be set to Paused (simulates disconnect with pause mode).
#[tokio::test]
async fn pause_mode_session_status_changes_to_paused() {
    let store = SessionStore::new();
    let session = store.create("test-agent").await;

    // Initially active
    assert_eq!(session.status, SessionStatus::Active);

    // Simulate disconnect with pause mode
    store
        .set_status(&session.id, SessionStatus::Paused)
        .await
        .unwrap();

    let updated = store.get(&session.id).await.unwrap();
    assert_eq!(updated.status, SessionStatus::Paused);
}

/// Test that snapshot is written with Paused status on disconnect.
#[tokio::test]
async fn pause_mode_snapshot_written_with_paused_status() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "pause_snapshot_test";

    let conversation = vec![
        Message {
            role: Role::User,
            content: "Hello".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Hi there!".to_string(),
        },
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

/// Test that partial content is saved when disconnected mid-stream (pause mode).
#[tokio::test]
async fn pause_mode_partial_content_saved_on_disconnect() {
    let store = SessionStore::new();
    let session = store.create("test-agent").await;

    // Simulate user message
    let user_msg = Message {
        role: Role::User,
        content: "What is Rust?".to_string(),
    };
    store.add_message(&session.id, user_msg).await.unwrap();

    // Simulate partial assistant response (disconnect mid-stream)
    let partial_response = Message {
        role: Role::Assistant,
        content: "Rust is a systems programming".to_string(), // Incomplete
    };
    store
        .add_message(&session.id, partial_response)
        .await
        .unwrap();

    // Set to paused
    store
        .set_status(&session.id, SessionStatus::Paused)
        .await
        .unwrap();

    // Verify partial content was saved
    let messages = store.get_messages(&session.id).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "Rust is a systems programming");

    let updated = store.get(&session.id).await.unwrap();
    assert_eq!(updated.status, SessionStatus::Paused);
}

/// Test snapshot with partial conversation is written correctly.
#[tokio::test]
async fn pause_mode_snapshot_preserves_partial_conversation() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "partial_conversation";

    // Simulate partial conversation saved on disconnect
    let conversation = vec![
        Message {
            role: Role::User,
            content: "Explain async/await".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Async/await is a".to_string(), // Partial response
        },
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
    assert_eq!(loaded.conversation[1].content, "Async/await is a");
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
    let conversation = vec![Message {
        role: Role::User,
        content: "Hello".to_string(),
    }];

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

/// Test that final message is saved when background task completes.
#[tokio::test]
async fn continue_mode_final_message_saved_on_completion() {
    let store = SessionStore::new();
    let session = store.create("background-agent").await;

    // Simulate user message before disconnect
    let user_msg = Message {
        role: Role::User,
        content: "What is Rust?".to_string(),
    };
    store.add_message(&session.id, user_msg).await.unwrap();

    // Simulate background task completing and saving full response
    let complete_response = Message {
        role: Role::Assistant,
        content: "Rust is a systems programming language focused on safety and performance."
            .to_string(),
    };
    store
        .add_message(&session.id, complete_response)
        .await
        .unwrap();

    // Verify complete message was saved
    let messages = store.get_messages(&session.id).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1].content,
        "Rust is a systems programming language focused on safety and performance."
    );
}

/// Test that final snapshot has Active status after background processing completes.
#[tokio::test]
async fn continue_mode_final_snapshot_has_active_status() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "continue_complete_test";

    // Simulate final snapshot after background task completes
    let conversation = vec![
        Message {
            role: Role::User,
            content: "Hello".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Hi there! How can I help you today?".to_string(),
        },
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
        Message {
            role: Role::User,
            content: "What is async?".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Async is a".to_string(), // Partial response when paused
        },
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
    assert_eq!(resumed.conversation[0].content, "What is async?");
    assert_eq!(resumed.conversation[1].content, "Async is a");
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Pause);
}

/// Test that we can attach to a session that continued in background.
#[tokio::test]
async fn attach_to_session_continued_in_background() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "attach_continued";

    // Simulate a session that completed in background after disconnect
    let conversation = vec![
        Message {
            role: Role::User,
            content: "Write a poem".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Roses are red,\nViolets are blue,\nRust is great,\nAnd so are you!"
                .to_string(),
        },
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
    assert!(resumed.conversation[1].content.contains("Roses are red"));
    assert_eq!(resumed.config.on_disconnect, OnDisconnect::Continue);
}

/// Test attaching to a session that was Running (background still processing).
#[tokio::test]
async fn attach_to_session_still_running_in_background() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "attach_running";

    // Simulate a session still running in background
    let conversation = vec![Message {
        role: Role::User,
        content: "Generate a long response".to_string(),
    }];

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
        resumed.conversation[1].content,
        "Here is a partial response so far..."
    );
}

// ============================================================================
// Session Store Registration Tests
// ============================================================================

/// Test registering a paused session in the session store.
#[tokio::test]
async fn register_paused_session_in_store() {
    use agnx::session::Session;

    let store = SessionStore::new();

    let session = Session {
        id: "session_paused_123".to_string(),
        agent: "test-agent".to_string(),
        status: SessionStatus::Paused,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_event_seq: 0,
    };
    let messages = vec![
        Message {
            role: Role::User,
            content: "Hello".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Hi!".to_string(),
        },
    ];

    store.register(session, messages).await;

    let fetched = store.get("session_paused_123").await.unwrap();
    assert_eq!(fetched.status, SessionStatus::Paused);

    let fetched_messages = store.get_messages("session_paused_123").await.unwrap();
    assert_eq!(fetched_messages.len(), 2);
}

/// Test that a registered session can have messages added.
#[tokio::test]
async fn registered_session_can_receive_messages() {
    use agnx::session::Session;

    let store = SessionStore::new();

    let session = Session {
        id: "session_continue_123".to_string(),
        agent: "background-agent".to_string(),
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_event_seq: 0,
    };
    let messages = vec![Message {
        role: Role::User,
        content: "Hello".to_string(),
    }];

    store.register(session, messages).await;

    // Add new message (simulating user continuing the conversation)
    let new_msg = Message {
        role: Role::User,
        content: "Follow up question".to_string(),
    };
    store
        .add_message("session_continue_123", new_msg)
        .await
        .unwrap();

    let messages = store.get_messages("session_continue_123").await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "Follow up question");
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

/// Test full pause mode scenario: create, message, disconnect, snapshot, resume.
#[tokio::test]
async fn full_pause_mode_scenario() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "full_pause_scenario";
    let store = SessionStore::new();

    // 1. Create session
    let session = store.create("test-agent").await;
    let session_id_owned = session.id.clone();

    // 2. Add user message
    store
        .add_message(
            &session_id_owned,
            Message {
                role: Role::User,
                content: "What is Rust?".to_string(),
            },
        )
        .await
        .unwrap();

    // 3. Simulate partial streaming response before disconnect
    store
        .add_message(
            &session_id_owned,
            Message {
                role: Role::Assistant,
                content: "Rust is a systems programming".to_string(),
            },
        )
        .await
        .unwrap();

    // 4. Disconnect happens - set status to paused
    store
        .set_status(&session_id_owned, SessionStatus::Paused)
        .await
        .unwrap();

    // 5. Write snapshot with paused state
    let messages = store.get_messages(&session_id_owned).await.unwrap();
    let session_data = store.get(&session_id_owned).await.unwrap();

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        session_data.agent.clone(),
        SessionStatus::Paused,
        session_data.created_at,
        2,
        messages,
        SessionConfig {
            on_disconnect: OnDisconnect::Pause,
            ..Default::default()
        },
    );

    write_snapshot(&sessions_dir(&temp_dir), session_id, &snapshot)
        .await
        .unwrap();

    // 6. Verify we can resume the paused session
    let resumed = resume_session(&sessions_dir(&temp_dir), session_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resumed.status, SessionStatus::Paused);
    assert_eq!(resumed.conversation.len(), 2);
    assert_eq!(
        resumed.conversation[1].content,
        "Rust is a systems programming"
    );
}

/// Test full continue mode scenario: create, message, disconnect, background, complete.
#[tokio::test]
async fn full_continue_mode_scenario() {
    let temp_dir = TempDir::new().unwrap();
    let session_id = "full_continue_scenario";

    // 1. Simulate session start with user message
    let initial_conversation = vec![Message {
        role: Role::User,
        content: "Write a haiku".to_string(),
    }];

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
        Message {
            role: Role::User,
            content: "Write a haiku".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Code compiles without error\nJoy fills my heart today\nRust is beautiful"
                .to_string(),
        },
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
            .content
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
        Message {
            role: Role::User,
            content: "Hello".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Hi!".to_string(),
        },
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
        Message {
            role: Role::User,
            content: "Hello".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "Hi!".to_string(),
        },
        Message {
            role: Role::User,
            content: "How are you?".to_string(),
        },
        Message {
            role: Role::Assistant,
            content: "I'm doing well!".to_string(),
        },
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
    assert_eq!(resumed_v2.conversation[3].content, "I'm doing well!");
    assert_eq!(resumed_v2.status, SessionStatus::Active);
}
