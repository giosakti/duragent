#![cfg(feature = "server")]
//! Integration tests for session persistence using FileSessionStore.

use chrono::Utc;
use tempfile::TempDir;

use duragent::agent::OnDisconnect;
use duragent::api::SessionStatus;
use duragent::llm::{Message, Role, Usage};
use duragent::session::{SessionConfig, SessionEvent, SessionEventPayload, SessionSnapshot};
use duragent::store::SessionStore;
use duragent::store::file::FileSessionStore;

// ============================================================================
// Helpers
// ============================================================================

fn create_store(temp_dir: &TempDir) -> FileSessionStore {
    FileSessionStore::new(temp_dir.path().join("sessions"))
}

// ============================================================================
// Event Persistence Tests
// ============================================================================

#[tokio::test]
async fn event_write_read_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "test_session";

    let events = vec![
        SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "test-agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        ),
        SessionEvent::new(
            2,
            SessionEventPayload::UserMessage {
                content: "Hello, agent!".to_string(),
                sender_id: None,
                sender_name: None,
            },
        ),
        SessionEvent::new(
            3,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Hello! How can I help?".to_string(),
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 8,
                    total_tokens: 18,
                }),
            },
        ),
    ];

    store.append_events(session_id, &events).await.unwrap();

    // Read events back
    let read_events = store.load_events(session_id, 0).await.unwrap();

    // Verify order and content
    assert_eq!(read_events.len(), 3);

    assert_eq!(read_events[0].seq, 1);
    match &read_events[0].payload {
        SessionEventPayload::SessionStart { agent, .. } => {
            assert_eq!(agent, "test-agent");
        }
        _ => panic!("expected SessionStart"),
    }

    assert_eq!(read_events[1].seq, 2);
    match &read_events[1].payload {
        SessionEventPayload::UserMessage { content, .. } => {
            assert_eq!(content, "Hello, agent!");
        }
        _ => panic!("expected UserMessage"),
    }

    assert_eq!(read_events[2].seq, 3);
    match &read_events[2].payload {
        SessionEventPayload::AssistantMessage {
            agent,
            content,
            usage,
        } => {
            assert_eq!(agent, "test-agent");
            assert_eq!(content, "Hello! How can I help?");
            assert!(usage.is_some());
            let usage = usage.as_ref().unwrap();
            assert_eq!(usage.total_tokens, 18);
        }
        _ => panic!("expected AssistantMessage"),
    }
}

#[tokio::test]
async fn events_preserve_sequence_order() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "seq_test";

    // Write events in sequence order
    let events: Vec<_> = (1..=10)
        .map(|i| {
            SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Message {}", i),
                    sender_id: None,
                    sender_name: None,
                },
            )
        })
        .collect();

    store.append_events(session_id, &events).await.unwrap();

    let read_events = store.load_events(session_id, 0).await.unwrap();

    // Verify order preserved
    for (i, event) in read_events.iter().enumerate() {
        assert_eq!(event.seq, (i + 1) as u64);
    }
}

#[tokio::test]
async fn append_to_existing_file() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "append_test";

    // Write first batch of events
    store
        .append_events(
            session_id,
            &[
                SessionEvent::new(
                    1,
                    SessionEventPayload::UserMessage {
                        content: "First".to_string(),
                        sender_id: None,
                        sender_name: None,
                    },
                ),
                SessionEvent::new(
                    2,
                    SessionEventPayload::UserMessage {
                        content: "Second".to_string(),
                        sender_id: None,
                        sender_name: None,
                    },
                ),
            ],
        )
        .await
        .unwrap();

    // Append more (simulating new request)
    store
        .append_events(
            session_id,
            &[SessionEvent::new(
                3,
                SessionEventPayload::UserMessage {
                    content: "Third".to_string(),
                    sender_id: None,
                    sender_name: None,
                },
            )],
        )
        .await
        .unwrap();

    // Read all events
    let events = store.load_events(session_id, 0).await.unwrap();

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
}

#[tokio::test]
async fn all_event_types_roundtrip() {
    use duragent::api::SessionStatus;
    use duragent::session::{SessionEndReason, ToolResultData};

    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "all_types_test";

    let events = vec![
        SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "test-agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        ),
        SessionEvent::new(
            2,
            SessionEventPayload::UserMessage {
                content: "Search for rust".to_string(),
                sender_id: None,
                sender_name: None,
            },
        ),
        SessionEvent::new(
            3,
            SessionEventPayload::ToolCall {
                call_id: "call_abc123".to_string(),
                tool_name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust programming"}),
            },
        ),
        SessionEvent::new(
            4,
            SessionEventPayload::ToolResult {
                call_id: "call_abc123".to_string(),
                result: ToolResultData {
                    success: true,
                    content: "Rust is a systems programming language.".to_string(),
                },
            },
        ),
        SessionEvent::new(
            5,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Here's what I found about Rust.".to_string(),
                usage: Some(Usage {
                    prompt_tokens: 50,
                    completion_tokens: 20,
                    total_tokens: 70,
                }),
            },
        ),
        SessionEvent::new(
            6,
            SessionEventPayload::StatusChange {
                from: SessionStatus::Active,
                to: SessionStatus::Paused,
            },
        ),
        SessionEvent::new(
            7,
            SessionEventPayload::Error {
                code: "rate_limit".to_string(),
                message: "Too many requests".to_string(),
            },
        ),
        SessionEvent::new(
            8,
            SessionEventPayload::SessionEnd {
                reason: SessionEndReason::Completed,
            },
        ),
    ];

    store.append_events(session_id, &events).await.unwrap();

    let read_events = store.load_events(session_id, 0).await.unwrap();

    assert_eq!(read_events.len(), 8);

    // Verify each event type deserialized correctly
    match &read_events[2].payload {
        SessionEventPayload::ToolCall {
            call_id,
            tool_name,
            arguments,
        } => {
            assert_eq!(call_id, "call_abc123");
            assert_eq!(tool_name, "web_search");
            assert_eq!(arguments["query"], "rust programming");
        }
        _ => panic!("expected ToolCall"),
    }

    match &read_events[3].payload {
        SessionEventPayload::ToolResult { call_id, result } => {
            assert_eq!(call_id, "call_abc123");
            assert!(result.success);
        }
        _ => panic!("expected ToolResult"),
    }

    match &read_events[5].payload {
        SessionEventPayload::StatusChange { from, to } => {
            assert_eq!(*from, SessionStatus::Active);
            assert_eq!(*to, SessionStatus::Paused);
        }
        _ => panic!("expected StatusChange"),
    }

    match &read_events[6].payload {
        SessionEventPayload::Error { code, message } => {
            assert_eq!(code, "rate_limit");
            assert_eq!(message, "Too many requests");
        }
        _ => panic!("expected Error"),
    }

    match &read_events[7].payload {
        SessionEventPayload::SessionEnd { reason } => {
            assert_eq!(*reason, SessionEndReason::Completed);
        }
        _ => panic!("expected SessionEnd"),
    }
}

// ============================================================================
// Snapshot Persistence Tests
// ============================================================================

#[tokio::test]
async fn snapshot_write_load_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "snapshot_test";
    let created_at = Utc::now();

    let conversation = vec![
        Message::text(Role::User, "What is 2 + 2?"),
        Message::text(Role::Assistant, "2 + 2 equals 4."),
    ];

    let config = SessionConfig {
        on_disconnect: OnDisconnect::Continue,
        ..Default::default()
    };

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "math-tutor".to_string(),
        SessionStatus::Active,
        created_at,
        42,
        42, // checkpoint_seq
        conversation.clone(),
        config,
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // Load snapshot back
    let loaded = store
        .load_snapshot(session_id)
        .await
        .unwrap()
        .expect("snapshot should exist");

    // Verify all fields
    assert_eq!(loaded.session_id, session_id);
    assert_eq!(loaded.agent, "math-tutor");
    assert_eq!(loaded.status, SessionStatus::Active);
    assert_eq!(loaded.last_event_seq, 42);
    assert_eq!(loaded.config.on_disconnect, OnDisconnect::Continue);

    assert_eq!(loaded.conversation.len(), 2);
    assert_eq!(loaded.conversation[0].role, Role::User);
    assert_eq!(loaded.conversation[0].content_str(), "What is 2 + 2?");
    assert_eq!(loaded.conversation[1].role, Role::Assistant);
    assert_eq!(loaded.conversation[1].content_str(), "2 + 2 equals 4.");
}

#[tokio::test]
async fn snapshot_preserves_all_statuses() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);

    let statuses = [
        SessionStatus::Active,
        SessionStatus::Paused,
        SessionStatus::Running,
        SessionStatus::Completed,
    ];

    for (i, status) in statuses.iter().enumerate() {
        let session_id = format!("status_test_{}", i);

        let snapshot = SessionSnapshot::new(
            session_id.clone(),
            "agent".to_string(),
            *status,
            Utc::now(),
            0,
            0, // checkpoint_seq
            vec![],
            SessionConfig::default(),
        );

        store.save_snapshot(&session_id, &snapshot).await.unwrap();

        let loaded = store.load_snapshot(&session_id).await.unwrap().unwrap();

        assert_eq!(loaded.status, *status);
    }
}

// ============================================================================
// Event-Snapshot Integration Tests
// ============================================================================

#[tokio::test]
async fn read_events_from_snapshot_seq() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "resume_test";

    // Write 10 events
    let events: Vec<_> = (1..=10)
        .map(|i| {
            SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Message {}", i),
                    sender_id: None,
                    sender_name: None,
                },
            )
        })
        .collect();

    store.append_events(session_id, &events).await.unwrap();

    // Write snapshot at seq 5
    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        5,
        5, // checkpoint_seq
        vec![],
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // Load snapshot and get last_event_seq
    let loaded_snapshot = store.load_snapshot(session_id).await.unwrap().unwrap();

    assert_eq!(loaded_snapshot.last_event_seq, 5);

    // Read events starting from seq after snapshot
    let events = store
        .load_events(session_id, loaded_snapshot.last_event_seq)
        .await
        .unwrap();

    // Should get events 6-10
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].seq, 6);
    assert_eq!(events[4].seq, 10);
}

#[tokio::test]
async fn events_and_snapshot_coordinate_recovery() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "recovery_test";

    // Simulate a session: write events
    let initial_events = vec![
        SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "recovery-agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        ),
        SessionEvent::new(
            2,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
                sender_id: None,
                sender_name: None,
            },
        ),
        SessionEvent::new(
            3,
            SessionEventPayload::AssistantMessage {
                agent: "recovery-agent".to_string(),
                content: "Hi there!".to_string(),
                usage: None,
            },
        ),
    ];

    store
        .append_events(session_id, &initial_events)
        .await
        .unwrap();

    // Take snapshot at seq 3
    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "recovery-agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        3,
        3, // checkpoint_seq
        vec![
            Message::text(Role::User, "Hello"),
            Message::text(Role::Assistant, "Hi there!"),
        ],
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // More events after snapshot
    let later_events = vec![
        SessionEvent::new(
            4,
            SessionEventPayload::UserMessage {
                content: "How are you?".to_string(),
                sender_id: None,
                sender_name: None,
            },
        ),
        SessionEvent::new(
            5,
            SessionEventPayload::AssistantMessage {
                agent: "recovery-agent".to_string(),
                content: "I'm doing well!".to_string(),
                usage: None,
            },
        ),
    ];

    store
        .append_events(session_id, &later_events)
        .await
        .unwrap();

    // Now simulate recovery: load snapshot and replay events
    let loaded_snapshot = store.load_snapshot(session_id).await.unwrap().unwrap();

    // Start with snapshot conversation
    let mut recovered_messages = loaded_snapshot.conversation.clone();
    assert_eq!(recovered_messages.len(), 2);

    // Replay events after snapshot
    let replay_events = store
        .load_events(session_id, loaded_snapshot.last_event_seq)
        .await
        .unwrap();

    for event in replay_events {
        if let Some(msg) = event.to_message() {
            recovered_messages.push(msg);
        }
    }

    // Verify full conversation recovered
    assert_eq!(recovered_messages.len(), 4);
    assert_eq!(recovered_messages[0].content_str(), "Hello");
    assert_eq!(recovered_messages[1].content_str(), "Hi there!");
    assert_eq!(recovered_messages[2].content_str(), "How are you?");
    assert_eq!(recovered_messages[3].content_str(), "I'm doing well!");
}

// ============================================================================
// Recovery Integration Tests
// ============================================================================

/// Test that recovery correctly replays events after a snapshot.
///
/// This simulates crash recovery: a session with 60 messages (triggering a
/// snapshot at 50), then recovery that must replay events 51-60 on top of
/// the snapshot to restore the full conversation.
#[tokio::test]
async fn recovery_replays_events_after_snapshot() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "sess_recovery_60";
    let agent = "recovery-agent";

    // 1. Write events 1-60 (simulating what the actor would write)
    let events: Vec<_> = (1..=60)
        .map(|i| {
            if i == 1 {
                SessionEvent::new(
                    i,
                    SessionEventPayload::SessionStart {
                        agent: agent.to_string(),
                        on_disconnect: OnDisconnect::Pause,
                        gateway: None,
                        gateway_chat_id: None,
                    },
                )
            } else if i % 2 == 0 {
                SessionEvent::new(
                    i,
                    SessionEventPayload::UserMessage {
                        content: format!("User message {}", i),
                        sender_id: None,
                        sender_name: None,
                    },
                )
            } else {
                SessionEvent::new(
                    i,
                    SessionEventPayload::AssistantMessage {
                        agent: agent.to_string(),
                        content: format!("Assistant message {}", i),
                        usage: None,
                    },
                )
            }
        })
        .collect();

    store.append_events(session_id, &events).await.unwrap();

    // 2. Create snapshot at seq 50 (as the actor would after 50 events)
    //    The conversation in the snapshot contains messages from events 2-50
    let snapshot_conversation: Vec<_> = (2..=50)
        .map(|i| {
            if i % 2 == 0 {
                Message::text(Role::User, format!("User message {}", i))
            } else {
                Message::text(Role::Assistant, format!("Assistant message {}", i))
            }
        })
        .collect();

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        agent.to_string(),
        SessionStatus::Active,
        Utc::now(),
        50, // last_event_seq at snapshot time
        50, // checkpoint_seq
        snapshot_conversation,
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // 3. Simulate recovery: load snapshot and replay events
    let loaded_snapshot = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(loaded_snapshot.last_event_seq, 50);
    assert_eq!(loaded_snapshot.checkpoint_seq, 50);
    assert_eq!(loaded_snapshot.conversation.len(), 49); // messages 2-50

    // Load events after snapshot
    let replay_events = store
        .load_events(session_id, loaded_snapshot.last_event_seq)
        .await
        .unwrap();

    // Should get events 51-60
    assert_eq!(replay_events.len(), 10);
    assert_eq!(replay_events[0].seq, 51);
    assert_eq!(replay_events[9].seq, 60);

    // 4. Replay events to rebuild full conversation
    let mut recovered_messages = loaded_snapshot.conversation.clone();

    for event in replay_events {
        if let Some(msg) = event.to_message() {
            recovered_messages.push(msg);
        }
    }

    // 5. Verify full conversation recovered (messages 2-60 = 59 messages)
    //    Event 1 is SessionStart, doesn't produce a message
    assert_eq!(recovered_messages.len(), 59);

    // Verify first message from snapshot
    assert_eq!(recovered_messages[0].content_str(), "User message 2");

    // Verify last message from snapshot
    assert_eq!(recovered_messages[48].content_str(), "User message 50");

    // Verify first replayed message (event 51)
    assert_eq!(recovered_messages[49].content_str(), "Assistant message 51");

    // Verify last replayed message (event 60)
    assert_eq!(recovered_messages[58].content_str(), "User message 60");
}

/// Test that recovery handles multiple snapshots, using only the latest.
#[tokio::test]
async fn recovery_uses_latest_snapshot() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "sess_multi_snapshot";
    let agent = "test-agent";

    // Write events 1-100
    let events: Vec<_> = (1..=100)
        .map(|i| {
            SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Message {}", i),
                    sender_id: None,
                    sender_name: None,
                },
            )
        })
        .collect();

    store.append_events(session_id, &events).await.unwrap();

    // Snapshot overwrites previous - this simulates taking a newer snapshot
    let snapshot_conversation: Vec<_> = (1..=75)
        .map(|i| Message::text(Role::User, format!("Message {}", i)))
        .collect();

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        agent.to_string(),
        SessionStatus::Active,
        Utc::now(),
        75, // last_event_seq at snapshot time
        75, // checkpoint_seq
        snapshot_conversation,
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // Simulate recovery
    let loaded = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(loaded.last_event_seq, 75);
    assert_eq!(loaded.checkpoint_seq, 75);

    let replay_events = store
        .load_events(session_id, loaded.last_event_seq)
        .await
        .unwrap();

    // Should only replay events 76-100
    assert_eq!(replay_events.len(), 25);
    assert_eq!(replay_events[0].seq, 76);

    let mut recovered = loaded.conversation.clone();
    for event in replay_events {
        if let Some(msg) = event.to_message() {
            recovered.push(msg);
        }
    }

    // All 100 messages recovered
    assert_eq!(recovered.len(), 100);
    assert_eq!(recovered[0].content_str(), "Message 1");
    assert_eq!(recovered[99].content_str(), "Message 100");
}

/// Test recovery when snapshot exists but no events after it.
#[tokio::test]
async fn recovery_with_no_events_after_snapshot() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "sess_snapshot_only";
    let agent = "test-agent";

    // Write events 1-10
    let events: Vec<_> = (1..=10)
        .map(|i| {
            SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Message {}", i),
                    sender_id: None,
                    sender_name: None,
                },
            )
        })
        .collect();

    store.append_events(session_id, &events).await.unwrap();

    // Snapshot at seq 10 (covers all events)
    let snapshot_conversation: Vec<_> = (1..=10)
        .map(|i| Message::text(Role::User, format!("Message {}", i)))
        .collect();

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        agent.to_string(),
        SessionStatus::Active,
        Utc::now(),
        10,
        10, // checkpoint_seq
        snapshot_conversation,
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // Simulate recovery
    let loaded = store.load_snapshot(session_id).await.unwrap().unwrap();
    let replay_events = store
        .load_events(session_id, loaded.replay_from_seq())
        .await
        .unwrap();

    // No events to replay (snapshot is current)
    assert!(replay_events.is_empty());

    // Conversation from snapshot is complete
    assert_eq!(loaded.conversation.len(), 10);
}

// ============================================================================
// Edge Cases: Missing Files
// ============================================================================

#[tokio::test]
async fn load_events_returns_empty_for_missing_session() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);

    let events = store.load_events("nonexistent_session", 0).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn load_snapshot_returns_none_for_missing_session() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);

    let result = store.load_snapshot("nonexistent_session").await.unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Edge Cases: Malformed Data
// ============================================================================

#[tokio::test]
async fn load_events_skips_malformed_lines() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "malformed_test";

    // Create session directory manually
    let session_dir = temp_dir.path().join("sessions").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    // Write a file with mixed valid and invalid lines
    let valid_event_1 = SessionEvent::new(
        1,
        SessionEventPayload::UserMessage {
            content: "First".to_string(),
            sender_id: None,
            sender_name: None,
        },
    );
    let valid_event_2 = SessionEvent::new(
        2,
        SessionEventPayload::UserMessage {
            content: "Second".to_string(),
            sender_id: None,
            sender_name: None,
        },
    );
    let valid_event_3 = SessionEvent::new(
        3,
        SessionEventPayload::UserMessage {
            content: "Third".to_string(),
            sender_id: None,
            sender_name: None,
        },
    );

    let content = format!(
        "{}\n\
         {{\"incomplete\n\
         not json at all\n\
         {}\n\
         \n\
         {{\"seq\": 999}}\n\
         {}\n",
        serde_json::to_string(&valid_event_1).unwrap(),
        serde_json::to_string(&valid_event_2).unwrap(),
        serde_json::to_string(&valid_event_3).unwrap(),
    );

    let events_path = session_dir.join("events.jsonl");
    tokio::fs::write(&events_path, content).await.unwrap();

    // Read should skip malformed lines
    let events = store.load_events(session_id, 0).await.unwrap();

    // Only valid events should be returned
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
}

#[tokio::test]
async fn load_events_handles_empty_file() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "empty_test";

    let session_dir = temp_dir.path().join("sessions").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    let events_path = session_dir.join("events.jsonl");
    tokio::fs::write(&events_path, "").await.unwrap();

    let events = store.load_events(session_id, 0).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn load_events_handles_only_empty_lines() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "whitespace_test";

    let session_dir = temp_dir.path().join("sessions").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    let events_path = session_dir.join("events.jsonl");
    tokio::fs::write(&events_path, "\n\n   \n\t\n")
        .await
        .unwrap();

    let events = store.load_events(session_id, 0).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn load_snapshot_returns_error_for_invalid_yaml() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "invalid_yaml_test";

    let session_dir = temp_dir.path().join("sessions").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    let state_path = session_dir.join("state.yaml");
    tokio::fs::write(&state_path, "not: [valid: yaml: [[[")
        .await
        .unwrap();

    let result = store.load_snapshot(session_id).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn load_snapshot_returns_error_for_incompatible_schema() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "old_schema_test";

    let mut snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        0,
        0, // checkpoint_seq
        vec![],
        SessionConfig::default(),
    );

    // Set an old schema version
    snapshot.schema_version = "0".to_string();

    let session_dir = temp_dir.path().join("sessions").join(session_id);
    tokio::fs::create_dir_all(&session_dir).await.unwrap();

    let yaml = serde_saphyr::to_string(&snapshot).unwrap();
    let state_path = session_dir.join("state.yaml");
    tokio::fs::write(&state_path, yaml).await.unwrap();

    let result = store.load_snapshot(session_id).await;

    assert!(result.is_err());
}

// ============================================================================
// Atomicity and Durability Tests
// ============================================================================

#[tokio::test]
async fn snapshot_write_is_atomic() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "atomic_test";

    let snapshot = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        42,
        42, // checkpoint_seq
        vec![],
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot).await.unwrap();

    // Verify final file exists
    let final_path = temp_dir
        .path()
        .join("sessions")
        .join(session_id)
        .join("state.yaml");
    assert!(final_path.exists());

    // Verify temp file does not exist
    let temp_path = temp_dir
        .path()
        .join("sessions")
        .join(session_id)
        .join("state.yaml.tmp");
    assert!(!temp_path.exists());
}

#[tokio::test]
async fn snapshot_overwrite_is_atomic() {
    let temp_dir = TempDir::new().unwrap();
    let store = create_store(&temp_dir);
    let session_id = "overwrite_test";

    // Write initial snapshot
    let snapshot1 = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Active,
        Utc::now(),
        10,
        10, // checkpoint_seq
        vec![],
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot1).await.unwrap();

    // Overwrite with new snapshot
    let snapshot2 = SessionSnapshot::new(
        session_id.to_string(),
        "agent".to_string(),
        SessionStatus::Completed,
        Utc::now(),
        50,
        50, // checkpoint_seq
        vec![],
        SessionConfig::default(),
    );

    store.save_snapshot(session_id, &snapshot2).await.unwrap();

    // Load and verify new snapshot
    let loaded = store.load_snapshot(session_id).await.unwrap().unwrap();

    assert_eq!(loaded.status, SessionStatus::Completed);
    assert_eq!(loaded.last_event_seq, 50);

    // Verify no temp file left behind
    let temp_path = temp_dir
        .path()
        .join("sessions")
        .join(session_id)
        .join("state.yaml.tmp");
    assert!(!temp_path.exists());
}
