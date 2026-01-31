//! Session event types for durable event logging.
//!
//! Events are appended to a JSONL file for crash-safe persistence.
//! Each event has a monotonic sequence number for replay ordering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    SessionStart { session_id: String, agent: String },
    /// Session ended (completed or terminated).
    SessionEnd { reason: SessionEndReason },
    /// User sent a message.
    UserMessage { content: String },
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
    /// Approval request timed out without user response.
    ApprovalTimeout { call_id: String, command: String },
    /// Session status changed.
    StatusChange {
        from: SessionStatus,
        to: SessionStatus,
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
            SessionEventPayload::UserMessage { content } => {
                Some(Message::text(Role::User, content))
            }
            SessionEventPayload::AssistantMessage { content, .. } => {
                Some(Message::text(Role::Assistant, content))
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
                tool_name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust async"}),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        assert!(json.contains("\"tool_name\":\"web_search\""));
    }

    #[test]
    fn deserialize_event_roundtrip() {
        let event = SessionEvent::new(
            1,
            SessionEventPayload::SessionStart {
                session_id: "session_abc".to_string(),
                agent: "my-agent".to_string(),
            },
        );

        let json = serde_json::to_string(&event).unwrap();
        let parsed: SessionEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.seq, 1);
        match parsed.payload {
            SessionEventPayload::SessionStart { session_id, agent } => {
                assert_eq!(session_id, "session_abc");
                assert_eq!(agent, "my-agent");
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
                session_id: "s".to_string(),
                agent: "a".to_string(),
            },
        );
        assert!(start_event.to_message().is_none());
    }
}
