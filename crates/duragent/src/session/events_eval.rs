//! Evaluation methods for session events.
//!
//! Extends `SessionEvent` and `PendingApproval` with message conversion logic.
//! The data definitions live in `duragent-types`; evaluation lives here.

use crate::llm::{FunctionCall, Message, Role, ToolCall};
use crate::session::{EventToolCall, PendingApproval, SessionEvent, SessionEventPayload};

// ============================================================================
// Public Traits
// ============================================================================

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
            SessionEventPayload::AssistantResponse {
                content,
                tool_calls,
                ..
            } => Some(assistant_response_to_message(content, tool_calls)),
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
            // ToolsAborted maps to N messages — handle directly in registry replay.
            // to_message() is a 1:1 mapping, so we return None here.
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

// ============================================================================
// Shared Helper
// ============================================================================

/// Build a `Message` from assistant content and tool calls.
///
/// Shared by actor (write-time), registry (replay-time), and events_eval.
/// Treats empty content as `None` when tool_calls are present.
pub(crate) fn assistant_response_to_message(
    content: &str,
    tool_calls: &[EventToolCall],
) -> Message {
    let has_tool_calls = !tool_calls.is_empty();
    let content_opt = if content.is_empty() && has_tool_calls {
        None
    } else {
        Some(content.to_string())
    };
    let tool_calls_opt = if has_tool_calls {
        Some(
            tool_calls
                .iter()
                .map(EventToolCall::to_llm_tool_call)
                .collect(),
        )
    } else {
        None
    };
    Message {
        role: Role::Assistant,
        content: content_opt,
        tool_calls: tool_calls_opt,
        tool_call_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::OnDisconnect;
    use crate::llm::Role;
    use crate::session::{SessionEvent, SessionEventPayload, ToolResultData};

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
    fn assistant_response_content_and_tool_calls() {
        use crate::session::EventToolCall;

        let event = SessionEvent::new(
            10,
            SessionEventPayload::AssistantResponse {
                agent: "a".to_string(),
                content: "Let me search.".to_string(),
                tool_calls: vec![EventToolCall {
                    call_id: "call_1".to_string(),
                    tool_name: "web".to_string(),
                    arguments: serde_json::json!({"q": "rust"}),
                }],
                usage: None,
            },
        );
        let msg = event.to_message().unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, Some("Let me search.".to_string()));
        let calls = msg.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "web");
    }

    #[test]
    fn assistant_response_tool_calls_only() {
        use crate::session::EventToolCall;

        let event = SessionEvent::new(
            11,
            SessionEventPayload::AssistantResponse {
                agent: "a".to_string(),
                content: String::new(),
                tool_calls: vec![EventToolCall {
                    call_id: "call_2".to_string(),
                    tool_name: "bash".to_string(),
                    arguments: serde_json::json!({"command": "ls"}),
                }],
                usage: None,
            },
        );
        let msg = event.to_message().unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_some());
    }

    #[test]
    fn assistant_response_content_only() {
        let event = SessionEvent::new(
            12,
            SessionEventPayload::AssistantResponse {
                agent: "a".to_string(),
                content: "Hello!".to_string(),
                tool_calls: vec![],
                usage: None,
            },
        );
        let msg = event.to_message().unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, Some("Hello!".to_string()));
        assert!(msg.tool_calls.is_none());
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
