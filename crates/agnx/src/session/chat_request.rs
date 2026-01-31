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
/// Combines `soul`, `system_prompt`, and `instructions` into a single string.
/// Order: soul (personality) → system_prompt (capabilities) → instructions (runtime).
pub fn build_system_message(agent_spec: &AgentSpec) -> Option<String> {
    let mut content = String::new();

    if let Some(ref soul) = agent_spec.soul {
        content.push_str(soul);
    }

    if let Some(ref prompt) = agent_spec.system_prompt {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
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
        messages.push(Message::text(Role::System, content));
    }
    messages.extend(history.iter().cloned());
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentMetadata, AgentSessionConfig, ModelConfig, ToolPolicy};
    use crate::llm::Provider;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_agent_spec(
        soul: Option<&str>,
        system_prompt: Option<&str>,
        instructions: Option<&str>,
    ) -> AgentSpec {
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
            soul: soul.map(|s| s.to_string()),
            system_prompt: system_prompt.map(|s| s.to_string()),
            instructions: instructions.map(|s| s.to_string()),
            session: AgentSessionConfig::default(),
            tools: Vec::new(),
            policy: ToolPolicy::default(),
            agent_dir: PathBuf::from("/tmp/test-agent"),
        }
    }

    #[test]
    fn build_system_message_with_all() {
        let spec = test_agent_spec(
            Some("I am a cheerful assistant."),
            Some("You are helpful."),
            Some("Be concise."),
        );
        let msg = build_system_message(&spec);
        assert_eq!(
            msg,
            Some("I am a cheerful assistant.\n\nYou are helpful.\n\nBe concise.".to_string())
        );
    }

    #[test]
    fn build_system_message_soul_and_prompt() {
        let spec = test_agent_spec(Some("I am cheerful."), Some("You are helpful."), None);
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("I am cheerful.\n\nYou are helpful.".to_string()));
    }

    #[test]
    fn build_system_message_prompt_only() {
        let spec = test_agent_spec(None, Some("You are helpful."), None);
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("You are helpful.".to_string()));
    }

    #[test]
    fn build_system_message_instructions_only() {
        let spec = test_agent_spec(None, None, Some("Be concise."));
        let msg = build_system_message(&spec);
        assert_eq!(msg, Some("Be concise.".to_string()));
    }

    #[test]
    fn build_system_message_empty() {
        let spec = test_agent_spec(None, None, None);
        let msg = build_system_message(&spec);
        assert!(msg.is_none());
    }
}
