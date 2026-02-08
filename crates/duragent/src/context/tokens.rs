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
/// Matches by model name substring. More specific patterns are checked before
/// catch-all family patterns. Returns a conservative default for unknown models.
///
/// Context windows sourced from OpenRouter model pages (Feb 2026).
pub fn default_context_window(model_name: &str) -> u32 {
    let name = model_name.to_lowercase();

    // Claude models (200K)
    if name.contains("claude") {
        return 200_000;
    }

    // GPT-5 / GPT-5 mini / GPT-5 nano (400K)
    if name.contains("gpt-5") {
        return 400_000;
    }

    // GPT-4.1 / GPT-4.1 mini / GPT-4.1 nano (1M) — before gpt-4o/gpt-4
    if name.contains("gpt-4.1") {
        return 1_000_000;
    }

    // GPT-4o / GPT-4o-mini (128K)
    if name.contains("gpt-4o") {
        return 128_000;
    }

    // GPT-4 Turbo (128K)
    if name.contains("gpt-4-turbo") {
        return 128_000;
    }

    // GPT-4 (128K)
    if name.contains("gpt-4") {
        return 128_000;
    }

    // Gemini (1M)
    if name.contains("gemini") {
        return 1_000_000;
    }

    // Grok 4.x (2M) — before grok catch-all
    if name.contains("grok-4") {
        return 2_000_000;
    }

    // Grok 3.x and older (131K)
    if name.contains("grok") {
        return 131_072;
    }

    // DeepSeek V3.x (163K) — before deepseek catch-all
    if name.contains("deepseek-v3") || name.contains("deepseek-chat-v3") {
        return 163_840;
    }

    // DeepSeek older (128K)
    if name.contains("deepseek") {
        return 128_000;
    }

    // Qwen 3 (131K)
    if name.contains("qwen3") {
        return 131_072;
    }

    // Qwen older (128K)
    if name.contains("qwen") {
        return 128_000;
    }

    // Llama 4 (327K — conservative; maverick is 1M, scout is 327K)
    if name.contains("llama-4") {
        return 327_680;
    }

    // Llama 3.x and older (128K)
    if name.contains("llama") {
        return 128_000;
    }

    // Mistral Large (262K) — before mistral catch-all
    if name.contains("mistral-large") {
        return 262_144;
    }

    // Mistral / Mixtral (128K)
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
    fn default_context_window_gpt5() {
        assert_eq!(default_context_window("openai/gpt-5"), 400_000);
        assert_eq!(default_context_window("openai/gpt-5-mini"), 400_000);
        assert_eq!(default_context_window("openai/gpt-5-nano"), 400_000);
    }

    #[test]
    fn default_context_window_gpt4_1() {
        assert_eq!(default_context_window("openai/gpt-4.1"), 1_000_000);
        assert_eq!(default_context_window("gpt-4.1-mini-2025-04-14"), 1_000_000);
        assert_eq!(default_context_window("gpt-4.1-nano"), 1_000_000);
    }

    #[test]
    fn default_context_window_gpt4o() {
        assert_eq!(default_context_window("openai/gpt-4o"), 128_000);
        assert_eq!(default_context_window("gpt-4o-mini"), 128_000);
    }

    #[test]
    fn default_context_window_gemini() {
        assert_eq!(default_context_window("google/gemini-pro"), 1_000_000);
        assert_eq!(default_context_window("google/gemini-2.5-flash"), 1_000_000);
    }

    #[test]
    fn default_context_window_grok4() {
        assert_eq!(default_context_window("x-ai/grok-4.1-fast"), 2_000_000);
        assert_eq!(default_context_window("x-ai/grok-4-fast"), 2_000_000);
    }

    #[test]
    fn default_context_window_grok3() {
        assert_eq!(default_context_window("x-ai/grok-3"), 131_072);
        assert_eq!(default_context_window("x-ai/grok-3-mini"), 131_072);
        // Old alias without version — falls to grok catch-all
        assert_eq!(default_context_window("grok-code-fast-1"), 131_072);
    }

    #[test]
    fn default_context_window_deepseek_v3() {
        assert_eq!(
            default_context_window("deepseek/deepseek-chat-v3-0324"),
            163_840
        );
        assert_eq!(
            default_context_window("deepseek/deepseek-chat-v3.1"),
            163_840
        );
        assert_eq!(default_context_window("deepseek/deepseek-v3.2"), 163_840);
    }

    #[test]
    fn default_context_window_deepseek_other() {
        assert_eq!(default_context_window("deepseek/deepseek-r1"), 128_000);
    }

    #[test]
    fn default_context_window_qwen3() {
        assert_eq!(default_context_window("qwen/qwen3-30b-a3b"), 131_072);
        assert_eq!(default_context_window("qwen/qwen3-coder"), 131_072);
    }

    #[test]
    fn default_context_window_qwen_older() {
        assert_eq!(default_context_window("qwen/qwen2-72b"), 128_000);
    }

    #[test]
    fn default_context_window_llama4() {
        assert_eq!(
            default_context_window("meta-llama/llama-4-maverick"),
            327_680
        );
        assert_eq!(default_context_window("meta-llama/llama-4-scout"), 327_680);
    }

    #[test]
    fn default_context_window_llama3() {
        assert_eq!(
            default_context_window("meta-llama/llama-3.3-70b-instruct"),
            128_000
        );
    }

    #[test]
    fn default_context_window_mistral_large() {
        assert_eq!(
            default_context_window("mistralai/mistral-large-2512"),
            262_144
        );
    }

    #[test]
    fn default_context_window_mistral_other() {
        assert_eq!(default_context_window("mistralai/mistral-small"), 128_000);
        assert_eq!(default_context_window("mixtral-8x7b"), 128_000);
    }

    #[test]
    fn default_context_window_unknown() {
        assert_eq!(default_context_window("some-unknown-model"), 128_000);
    }
}
