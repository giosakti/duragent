//! Common types for LLM chat completions.

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use super::error::LLMError;

// ============================================================================
// Chat Types
// ============================================================================

/// A chat completion request (OpenAI-compatible format).
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Tool definitions available to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
}

impl ChatRequest {
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature,
            max_tokens,
            tools: None,
        }
    }

    /// Create a chat request with tools.
    #[must_use]
    pub fn with_tools(
        model: impl Into<String>,
        messages: Vec<Message>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature,
            max_tokens,
            tools: if tools.is_empty() { None } else { Some(tools) },
        }
    }
}

/// A message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    /// Message content (optional when role is assistant with tool_calls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls made by the assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Tool call ID (when role is tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Create a simple text message.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Create a steering message (internal).
    pub fn steering(content: impl Into<String>) -> Self {
        Self {
            role: Role::Steering,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message with tool calls.
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    /// Get content as string (for backward compatibility).
    pub fn content_str(&self) -> &str {
        self.content.as_deref().unwrap_or("")
    }
}

/// The role of a message sender.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    /// Internal steering message (not user-visible).
    Steering,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
            Role::Steering => write!(f, "steering"),
        }
    }
}

// ============================================================================
// Tool Types
// ============================================================================

/// Tool definition sent to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (always "function" for now).
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition.
    pub function: FunctionDefinition,
}

/// Function definition within a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// Tool call from LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Type of tool (always "function" for now).
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    /// Function call details.
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// Function call details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name to call.
    pub name: String,
    /// JSON-encoded arguments string.
    pub arguments: String,
}

// ============================================================================
// Response Types
// ============================================================================

/// A chat completion response.
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

/// A single completion choice.
#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

/// Token usage statistics.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Events emitted during streaming chat completion.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A content token from the assistant.
    Token(String),
    /// Tool calls from the assistant.
    ToolCalls(Vec<ToolCall>),
    /// The stream is complete with optional usage stats.
    Done { usage: Option<Usage> },
    /// The stream was cancelled (e.g., client disconnected).
    Cancelled,
}

/// A boxed stream of streaming events.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LLMError>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_serialization() {
        let request = ChatRequest {
            model: "openai/gpt-4".to_string(),
            messages: vec![
                Message::text(Role::System, "You are a helpful assistant."),
                Message::text(Role::User, "Hello!"),
            ],
            temperature: Some(0.7),
            max_tokens: None,
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"openai/gpt-4\""));
        assert!(json.contains("\"role\":\"system\""));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"temperature\":0.7"));
        assert!(!json.contains("max_tokens"));
    }

    #[test]
    fn test_chat_request_without_optional_fields() {
        let request = ChatRequest {
            model: "openai/gpt-4".to_string(),
            messages: vec![Message::text(Role::User, "Hi")],
            temperature: None,
            max_tokens: None,
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("temperature"));
        assert!(!json.contains("max_tokens"));
    }

    #[test]
    fn test_chat_response_deserialization() {
        let json = r#"{
            "id": "chatcmpl-123",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! How can I help you today?"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8,
                "total_tokens": 18
            }
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].index, 0);
        assert_eq!(response.choices[0].message.role, Role::Assistant);
        assert_eq!(
            response.choices[0].message.content,
            Some("Hello! How can I help you today?".to_string())
        );
        assert_eq!(response.choices[0].finish_reason, Some("stop".to_string()));

        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 8);
        assert_eq!(usage.total_tokens, 18);
    }

    #[test]
    fn test_chat_response_without_usage() {
        let json = r#"{
            "id": "chatcmpl-456",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Response"
                    },
                    "finish_reason": null
                }
            ]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-456");
        assert!(response.usage.is_none());
        assert!(response.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_message_roles() {
        let system = Role::System;
        let user = Role::User;
        let assistant = Role::Assistant;
        let tool = Role::Tool;
        let steering = Role::Steering;

        assert_eq!(serde_json::to_string(&system).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&user).unwrap(), "\"user\"");
        assert_eq!(serde_json::to_string(&assistant).unwrap(), "\"assistant\"");
        assert_eq!(serde_json::to_string(&tool).unwrap(), "\"tool\"");
        assert_eq!(serde_json::to_string(&steering).unwrap(), "\"steering\"");

        assert_eq!(
            serde_json::from_str::<Role>("\"system\"").unwrap(),
            Role::System
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"user\"").unwrap(),
            Role::User
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"assistant\"").unwrap(),
            Role::Assistant
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"tool\"").unwrap(),
            Role::Tool
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"steering\"").unwrap(),
            Role::Steering
        );
    }

    #[test]
    fn test_tool_call_serialization() {
        let tool_call = ToolCall {
            id: "call_123".to_string(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: "bash".to_string(),
                arguments: r#"{"command":"ls -la"}"#.to_string(),
            },
        };

        let json = serde_json::to_string(&tool_call).unwrap();
        assert!(json.contains("\"id\":\"call_123\""));
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"name\":\"bash\""));
    }

    #[test]
    fn test_message_with_tool_calls() {
        let msg = Message::assistant_tool_calls(vec![ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
        }]);

        assert_eq!(msg.role, Role::Assistant);
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_some());
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_tool_result_message() {
        let msg = Message::tool_result("call_123", "Command output");

        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.content, Some("Command output".to_string()));
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    }
}
