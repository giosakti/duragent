//! Session event types for durable event logging.
//!
//! Events are appended to a JSONL file for crash-safe persistence.
//! Each event has a monotonic sequence number for replay ordering.
//!
//! Evaluation methods (`to_message`, `into_messages`) live in
//! `duragent::session::events_eval`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::{OnDisconnect, ToolType};
use crate::llm::{FunctionCall, Message, ToolCall, Usage};

use super::SessionStatus;

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
    /// Composite assistant response with optional tool calls (preferred for new events).
    ///
    /// Folds content + tool_calls into a single atomic event, eliminating the
    /// merge heuristic needed when reading separate AssistantMessage + ToolCall events.
    AssistantResponse {
        agent: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<EventToolCall>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// Agent requested a tool call (legacy — kept for reading existing JSONL).
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
    /// Multiple tool calls were aborted (e.g. steering or approval interrupted the iteration).
    ///
    /// Registry replay synthesizes N `tool_result` messages with success=false.
    /// Replaces N individual `ToolResult` events with one record.
    ToolsSkipped {
        call_ids: Vec<String>,
        reason: String,
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

/// A tool call within an `AssistantResponse` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

impl EventToolCall {
    /// Convert to the LLM `ToolCall` type for message construction.
    pub fn to_llm_tool_call(&self) -> ToolCall {
        ToolCall {
            id: self.call_id.clone(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: self.tool_name.clone(),
                arguments: serde_json::to_string(&self.arguments).unwrap_or_default(),
            },
        }
    }
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
}

/// A pending approval waiting for user decision.
///
/// Approvals have no timeout — they wait indefinitely until the user
/// approves, denies, or sends a new message.
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
    /// The tool type (for saving "Allow Always" patterns).
    #[serde(default = "default_tool_type")]
    pub tool_type: ToolType,
    /// Accumulated messages to restore when resuming the loop.
    pub messages: Vec<Message>,
    /// Platform sender ID of the user who triggered this approval.
    /// Used in group chats to ensure only the requester can approve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_id: Option<String>,
}

/// Default tool type for backwards compatibility with persisted approvals.
fn default_tool_type() -> ToolType {
    ToolType::Bash
}

impl PendingApproval {
    /// Create a new pending approval.
    pub fn new(
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        command: String,
        tool_type: ToolType,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            call_id,
            tool_name,
            arguments,
            command,
            tool_type,
            messages,
            requester_id: None,
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
    fn serialize_assistant_response_with_tool_calls() {
        let event = SessionEvent::new(
            10,
            SessionEventPayload::AssistantResponse {
                agent: "test-agent".to_string(),
                content: "Let me search for that.".to_string(),
                tool_calls: vec![EventToolCall {
                    call_id: "call_abc".to_string(),
                    tool_name: "web".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                }],
                usage: None,
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"assistant_response\""));
        assert!(json.contains("\"call_id\":\"call_abc\""));

        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        match parsed.payload {
            SessionEventPayload::AssistantResponse {
                agent,
                content,
                tool_calls,
                ..
            } => {
                assert_eq!(agent, "test-agent");
                assert_eq!(content, "Let me search for that.");
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].tool_name, "web");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn serialize_assistant_response_without_tool_calls() {
        let event = SessionEvent::new(
            11,
            SessionEventPayload::AssistantResponse {
                agent: "test-agent".to_string(),
                content: "Hello!".to_string(),
                tool_calls: vec![],
                usage: None,
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        // Empty tool_calls should be omitted
        assert!(!json.contains("tool_calls"));

        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        match parsed.payload {
            SessionEventPayload::AssistantResponse {
                content,
                tool_calls,
                ..
            } => {
                assert_eq!(content, "Hello!");
                assert!(tool_calls.is_empty());
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn event_tool_call_to_llm_tool_call() {
        let etc = EventToolCall {
            call_id: "call_123".to_string(),
            tool_name: "bash".to_string(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        let tc = etc.to_llm_tool_call();
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.function.name, "bash");
        assert_eq!(tc.function.arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn tools_skipped_serialization_roundtrip() {
        let event = SessionEvent::new(
            7,
            SessionEventPayload::ToolsSkipped {
                call_ids: vec!["call_a".to_string(), "call_b".to_string()],
                reason: "new user message received".to_string(),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tools_skipped\""));
        assert!(json.contains("\"call_ids\""));
        assert!(json.contains("\"reason\":\"new user message received\""));

        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.seq, 7);
        match parsed.payload {
            SessionEventPayload::ToolsSkipped { call_ids, reason } => {
                assert_eq!(call_ids, vec!["call_a", "call_b"]);
                assert_eq!(reason, "new user message received");
            }
            _ => panic!("Wrong event type"),
        }
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
