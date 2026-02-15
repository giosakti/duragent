//! Session event types for durable event logging.
//!
//! Events are appended to a JSONL file for crash-safe persistence.
//! Each event has a monotonic sequence number for replay ordering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::OnDisconnect;
use crate::api::SessionStatus;
use crate::llm::{Message, Role, Usage};

/// A session event that can be persisted to the event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Monotonic sequence number for ordering.
    pub seq: u64,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// The event payload.
    #[serde(flatten)]
    pub payload: SessionEventPayload,
}

/// The payload of a session event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEventPayload {
    /// Session was created.
    SessionStart {
        agent: String,
        /// Behavior when client disconnects. Defaults to Pause for backward compatibility.
        #[serde(default)]
        on_disconnect: OnDisconnect,
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_chat_id: Option<String>,
    },
    /// Session ended (completed or terminated).
    SessionEnd { reason: SessionEndReason },
    /// User sent a message.
    UserMessage {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_name: Option<String>,
    },
    /// Assistant (LLM) responded.
    AssistantMessage {
        /// Which agent produced this response.
        agent: String,
        content: String,
        /// Token usage for this response, if available.
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// Agent requested a tool call.
    ToolCall {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Tool execution completed.
    ToolResult {
        call_id: String,
        result: ToolResultData,
    },
    /// Tool requires human approval before execution.
    ApprovalRequired { call_id: String, command: String },
    /// User made an approval decision.
    ApprovalDecision {
        call_id: String,
        decision: ApprovalDecisionType,
    },
    /// Session status changed.
    StatusChange {
        from: SessionStatus,
        to: SessionStatus,
    },
    /// A message stored in session history but excluded from LLM conversation.
    ///
    /// Used for group messages from senders with `silent` disposition.
    /// Visible in `events.jsonl` for audit; `to_message()` returns `None`.
    SilentMessage {
        content: String,
        sender_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_name: Option<String>,
    },
    /// An error occurred (recoverable).
    Error { code: String, message: String },
}

/// Type of approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionType {
    /// Allow this command once.
    AllowOnce,
    /// Allow this command pattern always (saves to policy.local.yaml).
    AllowAlways,
    /// Deny this command.
    Deny,
}

/// Reason for session end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// User or client requested completion.
    Completed,
    /// Session was explicitly terminated.
    Terminated,
    /// Session timed out.
    Timeout,
    /// Unrecoverable error.
    Error,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultData {
    /// Whether the tool succeeded.
    pub success: bool,
    /// Content for LLM consumption.
    pub content: String,
}

impl SessionEvent {
    /// Create a new event with the given sequence number and payload.
    #[must_use]
    pub fn new(seq: u64, payload: SessionEventPayload) -> Self {
        Self {
            seq,
            timestamp: Utc::now(),
            payload,
        }
    }

    /// Convert this event to a Message if it represents a chat message.
    pub fn to_message(&self) -> Option<Message> {
        match &self.payload {
            SessionEventPayload::UserMessage { content, .. } => {
                Some(Message::text(Role::User, content))
            }
            SessionEventPayload::AssistantMessage { content, .. } => {
                Some(Message::text(Role::Assistant, content))
            }
            SessionEventPayload::ToolCall {
                call_id,
                tool_name,
                arguments,
            } => {
                let tc = crate::llm::ToolCall {
                    id: call_id.clone(),
                    tool_type: "function".to_string(),
                    function: crate::llm::FunctionCall {
                        name: tool_name.clone(),
                        arguments: serde_json::to_string(arguments).unwrap_or_default(),
                    },
                };
                Some(Message::assistant_tool_calls(vec![tc]))
            }
            SessionEventPayload::ToolResult { call_id, result } => {
                Some(Message::tool_result(call_id, &result.content))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_user_message_event() {
        let event = SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
                sender_id: None,
                sender_name: None,
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"user_message\""));
        assert!(json.contains("\"content\":\"Hello\""));
        assert!(json.contains("\"seq\":1"));
    }

    #[test]
    fn serialize_assistant_message_event() {
        let event = SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Hi there!".to_string(),
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                }),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"assistant_message\""));
        assert!(json.contains("\"agent\":\"test-agent\""));
        assert!(json.contains("\"prompt_tokens\":10"));
    }

    #[test]
    fn serialize_tool_call_event() {
        let event = SessionEvent::new(
            3,
            SessionEventPayload::ToolCall {
                call_id: "call_123".to_string(),
                tool_name: "web".to_string(),
                arguments: serde_json::json!({"action": "search", "query": "rust async"}),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        assert!(json.contains("\"tool_name\":\"web\""));
    }

    #[test]
    fn deserialize_event_roundtrip() {
        let event = SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                agent: "my-agent".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.seq, 1);
        match parsed.payload {
            SessionEventPayload::SessionStart {
                agent,
                on_disconnect,
                ..
            } => {
                assert_eq!(agent, "my-agent");
                assert_eq!(on_disconnect, OnDisconnect::Pause);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn event_to_message() {
        let user_event = SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
                sender_id: None,
                sender_name: None,
            },
        );
        let msg = user_event.to_message().unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content_str(), "Hello");

        let assistant_event = SessionEvent::new(
            2,
            SessionEventPayload::AssistantMessage {
                agent: "test-agent".to_string(),
                content: "Hi".to_string(),
                usage: None,
            },
        );
        let msg = assistant_event.to_message().unwrap();
        assert_eq!(msg.role, Role::Assistant);

        let start_event = SessionEvent::new(
            0,
            SessionEventPayload::SessionStart {
                agent: "a".to_string(),
                on_disconnect: OnDisconnect::Pause,
                gateway: None,
                gateway_chat_id: None,
            },
        );
        assert!(start_event.to_message().is_none());

        // ToolCall -> assistant message with tool_calls
        let tool_call_event = SessionEvent::new(
            3,
            SessionEventPayload::ToolCall {
                call_id: "call_123".to_string(),
                tool_name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            },
        );
        let msg = tool_call_event.to_message().unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert!(msg.content.is_none());
        let calls = msg.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_123");
        assert_eq!(calls[0].function.name, "bash");

        // ToolResult -> tool message
        let tool_result_event = SessionEvent::new(
            4,
            SessionEventPayload::ToolResult {
                call_id: "call_123".to_string(),
                result: ToolResultData {
                    success: true,
                    content: "file1.txt\nfile2.txt".to_string(),
                },
            },
        );
        let msg = tool_result_event.to_message().unwrap();
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
        assert_eq!(msg.content_str(), "file1.txt\nfile2.txt");
    }

    #[test]
    fn silent_message_serialization_roundtrip() {
        let event = SessionEvent::new(
            5,
            SessionEventPayload::SilentMessage {
                content: "alice: hello everyone".to_string(),
                sender_id: "12345".to_string(),
                sender_name: Some("alice".to_string()),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"silent_message\""));
        assert!(json.contains("\"sender_id\":\"12345\""));

        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.seq, 5);
        match parsed.payload {
            SessionEventPayload::SilentMessage {
                content,
                sender_id,
                sender_name,
            } => {
                assert_eq!(content, "alice: hello everyone");
                assert_eq!(sender_id, "12345");
                assert_eq!(sender_name, Some("alice".to_string()));
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn silent_message_to_message_returns_none() {
        let event = SessionEvent::new(
            1,
            SessionEventPayload::SilentMessage {
                content: "ignored content".to_string(),
                sender_id: "123".to_string(),
                sender_name: None,
            },
        );
        assert!(event.to_message().is_none());
    }

    #[test]
    fn deserialize_old_session_start_without_on_disconnect() {
        // Old format without on_disconnect field - should default to Pause
        let old_json = r#"{"seq":1,"timestamp":"2024-01-01T00:00:00Z","type":"session_start","agent":"old-agent"}"#;
        let parsed: SessionEvent = serde_json::from_str(old_json).unwrap();

        assert_eq!(parsed.seq, 1);
        match parsed.payload {
            SessionEventPayload::SessionStart {
                agent,
                on_disconnect,
                gateway,
                gateway_chat_id,
            } => {
                assert_eq!(agent, "old-agent");
                assert_eq!(on_disconnect, OnDisconnect::Pause); // Default value
                assert!(gateway.is_none());
                assert!(gateway_chat_id.is_none());
            }
            _ => panic!("Wrong event type"),
        }
    }
}
