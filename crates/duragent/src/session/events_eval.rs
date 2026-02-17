//! Evaluation methods for session events.
//!
//! Extends `SessionEvent` and `PendingApproval` with message conversion logic.
//! The data definitions live in `duragent-types`; evaluation lives here.

use crate::llm::{FunctionCall, Message, Role, ToolCall};
use crate::session::{PendingApproval, SessionEvent, SessionEventPayload};

/// Extension trait for `SessionEvent` evaluation logic.
pub trait SessionEventEval {
    /// Convert this event to a Message if it represents a chat message.
    fn to_message(&self) -> Option<Message>;
}

impl SessionEventEval for SessionEvent {
    fn to_message(&self) -> Option<Message> {
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
                let tc = ToolCall {
                    id: call_id.clone(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
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

/// Extension trait for `PendingApproval` evaluation logic.
pub trait PendingApprovalEval {
    /// Convert into initial messages for resuming the loop, appending the tool result.
    fn into_messages(self, tool_result_content: String) -> Vec<Message>;
}

impl PendingApprovalEval for PendingApproval {
    fn into_messages(self, tool_result_content: String) -> Vec<Message> {
        let mut messages = self.messages;
        messages.push(Message::tool_result(&self.call_id, tool_result_content));
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;
    use crate::session::{SessionEvent, SessionEventPayload, ToolResultData};
    use duragent_types::agent::OnDisconnect;

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
}
