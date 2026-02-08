//! Token estimation utilities for context window management.
//!
//! Uses byte-length heuristics (bytes / 4) rather than actual tokenization.
//! This is intentionally approximate — the goal is budget management, not precision.

use crate::llm::{Message, ToolDefinition};

/// Estimate the number of tokens in a text string.
///
/// Uses the `bytes / 4` heuristic, which is a reasonable approximation
/// for most modern tokenizers across English and code.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32) / 4
}

/// Estimate the number of tokens in a message.
///
/// Includes content, serialized tool_calls, and per-message overhead.
pub fn estimate_message_tokens(msg: &Message) -> u32 {
    let mut total = 0u32;

    // Content tokens
    if let Some(content) = &msg.content {
        total += estimate_tokens(content);
    }

    // Tool calls tokens (serialized JSON)
    if let Some(tool_calls) = &msg.tool_calls
        && let Ok(json) = serde_json::to_string(tool_calls)
    {
        total += estimate_tokens(&json);
    }

    // Tool call ID overhead
    if let Some(id) = &msg.tool_call_id {
        total += estimate_tokens(id);
    }

    // Per-message overhead (role, formatting)
    total += 4;

    total
}

/// Estimate the number of tokens for tool definitions.
pub fn estimate_tool_definitions_tokens(tools: &[ToolDefinition]) -> u32 {
    if tools.is_empty() {
        return 0;
    }

    if let Ok(json) = serde_json::to_string(tools) {
        estimate_tokens(&json)
    } else {
        0
    }
}

/// Return the default context window size for a model.
///
/// Looks up known models by prefix. Returns a conservative default for unknown models.
pub fn default_context_window(model_name: &str) -> u32 {
    let name = model_name.to_lowercase();

    // Claude models
    if name.contains("claude") {
        return 200_000;
    }

    // GPT-4o / GPT-4o-mini
    if name.contains("gpt-4o") {
        return 128_000;
    }

    // GPT-4 Turbo
    if name.contains("gpt-4-turbo") || name.contains("gpt-4-1") {
        return 128_000;
    }

    // GPT-4 (original)
    if name.contains("gpt-4") {
        return 128_000;
    }

    // Gemini
    if name.contains("gemini") {
        return 1_000_000;
    }

    // DeepSeek
    if name.contains("deepseek") {
        return 128_000;
    }

    // Llama
    if name.contains("llama") {
        return 128_000;
    }

    // Mistral
    if name.contains("mistral") || name.contains("mixtral") {
        return 128_000;
    }

    // Conservative default
    128_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, FunctionDefinition, Role, ToolCall};

    #[test]
    fn estimate_tokens_basic() {
        // 4 bytes -> 1 token
        assert_eq!(estimate_tokens("abcd"), 1);
        // 8 bytes -> 2 tokens
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        // Empty string -> 0
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_unicode() {
        // Multi-byte chars: "日本語" = 9 bytes -> 2 tokens
        let text = "日本語";
        assert_eq!(estimate_tokens(text), text.len() as u32 / 4);
    }

    #[test]
    fn estimate_message_tokens_text_message() {
        let msg = Message::text(Role::User, "Hello world");
        let tokens = estimate_message_tokens(&msg);
        // "Hello world" = 11 bytes / 4 = 2, plus 4 overhead = 6
        assert_eq!(tokens, 6);
    }

    #[test]
    fn estimate_message_tokens_with_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            content: Some("Thinking...".to_string()),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "bash".to_string(),
                    arguments: r#"{"command":"ls"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        };
        let tokens = estimate_message_tokens(&msg);
        // Should include content + tool_calls JSON + overhead
        assert!(tokens > 10);
    }

    #[test]
    fn estimate_message_tokens_tool_result() {
        let msg = Message::tool_result("call_1", "file1.txt\nfile2.txt");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 4); // content + tool_call_id + overhead
    }

    #[test]
    fn estimate_tool_definitions_tokens_empty() {
        assert_eq!(estimate_tool_definitions_tokens(&[]), 0);
    }

    #[test]
    fn estimate_tool_definitions_tokens_with_tools() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "bash".to_string(),
                description: "Execute a bash command".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"}
                    }
                })),
            },
        }];
        let tokens = estimate_tool_definitions_tokens(&tools);
        assert!(tokens > 0);
    }

    #[test]
    fn default_context_window_claude() {
        assert_eq!(default_context_window("anthropic/claude-sonnet-4"), 200_000);
        assert_eq!(default_context_window("claude-3-opus"), 200_000);
    }

    #[test]
    fn default_context_window_gpt4o() {
        assert_eq!(default_context_window("openai/gpt-4o"), 128_000);
        assert_eq!(default_context_window("gpt-4o-mini"), 128_000);
    }

    #[test]
    fn default_context_window_gemini() {
        assert_eq!(default_context_window("google/gemini-pro"), 1_000_000);
    }

    #[test]
    fn default_context_window_unknown() {
        assert_eq!(default_context_window("some-unknown-model"), 128_000);
    }
}
