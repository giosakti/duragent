//! Common types for LLM chat completions.

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use super::error::LLMError;

/// A chat completion request (OpenAI-compatible format).
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
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
        }
    }
}

/// A message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// The role of a message sender.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
        }
    }
}

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
                Message {
                    role: Role::System,
                    content: "You are a helpful assistant.".to_string(),
                },
                Message {
                    role: Role::User,
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            max_tokens: None,
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
            messages: vec![Message {
                role: Role::User,
                content: "Hi".to_string(),
            }],
            temperature: None,
            max_tokens: None,
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
            "Hello! How can I help you today?"
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

        assert_eq!(serde_json::to_string(&system).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&user).unwrap(), "\"user\"");
        assert_eq!(serde_json::to_string(&assistant).unwrap(), "\"assistant\"");

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
    }
}
