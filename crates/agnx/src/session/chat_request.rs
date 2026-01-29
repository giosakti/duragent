//! Chat request utilities for building LLM requests.
//!
//! Provides helper functions for constructing chat messages and requests.

use crate::agent::AgentSpec;
use crate::llm::{ChatRequest, Message, Role};

/// Build a chat request from system content and conversation history.
pub fn build_chat_request(
    model_name: &str,
    system_message: Option<&str>,
    history: &[Message],
    temperature: Option<f32>,
    max_output_tokens: Option<u32>,
) -> ChatRequest {
    let messages = build_chat_messages(system_message, history);
    ChatRequest::new(model_name, messages, temperature, max_output_tokens)
}

/// Build system message from agent spec.
///
/// Combines `system_prompt` and `instructions` into a single string.
pub fn build_system_message(agent_spec: &AgentSpec) -> Option<String> {
    let mut content = String::new();

    if let Some(ref prompt) = agent_spec.system_prompt {
        content.push_str(prompt);
    }

    if let Some(ref instructions) = agent_spec.instructions {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(instructions);
    }

    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

/// Build messages for a chat request from system content and conversation history.
pub fn build_chat_messages(system_message: Option<&str>, history: &[Message]) -> Vec<Message> {
    let mut messages = Vec::new();
    if let Some(content) = system_message {
        messages.push(Message {
            role: Role::System,
            content: content.to_string(),
        });
    }
    messages.extend(history.iter().cloned());
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentMetadata, AgentSessionConfig, ModelConfig};
    use crate::llm::Provider;
    use std::collections::HashMap;

    fn test_agent_spec(system_prompt: Option<&str>, instructions: Option<&str>) -> AgentSpec {
        AgentSpec {
            api_version: "agnx/v1alpha1".to_string(),
            kind: "Agent".to_string(),
            metadata: AgentMetadata {
                name: "test-agent".to_string(),
                description: None,
                version: None,
                labels: HashMap::new(),
            },
            model: ModelConfig {
                provider: Provider::Other("test".to_string()),
                name: "test-model".to_string(),
                base_url: None,
                temperature: None,
                max_input_tokens: None,
                max_output_tokens: None,
            },
            system_prompt: system_prompt.map(|s| s.to_string()),
            instructions: instructions.map(|s| s.to_string()),
            session: AgentSessionConfig::default(),
        }
    }

    #[test]
    fn build_system_message_with_both() {
        let spec = test_agent_spec(Some("You are helpful."), Some("Be concise."));
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("You are helpful.\n\nBe concise.".to_string()));
    }

    #[test]
    fn build_system_message_prompt_only() {
        let spec = test_agent_spec(Some("You are helpful."), None);
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("You are helpful.".to_string()));
    }

    #[test]
    fn build_system_message_instructions_only() {
        let spec = test_agent_spec(None, Some("Be concise."));
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("Be concise.".to_string()));
    }

    #[test]
    fn build_system_message_empty() {
        let spec = test_agent_spec(None, None);
        let msg = build_system_message(&spec);
        assert!(msg.is_none());
    }
}
