//! Evaluation methods for agent specification types.
//!
//! Extends `ModelConfig` and `HooksConfig` with runtime logic.
//! The data definitions live in `duragent-types`; evaluation lives here.

use crate::agent::{HooksConfig, ModelConfig};

/// Extension trait for `ModelConfig` evaluation logic.
pub trait ModelConfigEval {
    /// Returns `max_input_tokens` if set, otherwise the default for the model.
    fn effective_max_input_tokens(&self) -> u32;
}

impl ModelConfigEval for ModelConfig {
    fn effective_max_input_tokens(&self) -> u32 {
        self.max_input_tokens
            .unwrap_or_else(|| default_context_window(&self.name))
    }
}

/// Extension trait for `HooksConfig` evaluation logic.
pub trait HooksConfigEval {
    /// Merge with default hooks. Agent-configured hooks override defaults
    /// when they share the same `tool_match` pattern.
    fn with_defaults(self, defaults: HooksConfig) -> Self;
}

impl HooksConfigEval for HooksConfig {
    fn with_defaults(mut self, defaults: HooksConfig) -> Self {
        let agent_before_patterns: std::collections::HashSet<String> = self
            .before_tool
            .iter()
            .map(|h| h.tool_match.clone())
            .collect();
        let agent_after_patterns: std::collections::HashSet<String> = self
            .after_tool
            .iter()
            .map(|h| h.tool_match.clone())
            .collect();

        for hook in defaults.before_tool {
            if !agent_before_patterns.contains(&hook.tool_match) {
                self.before_tool.push(hook);
            }
        }
        for hook in defaults.after_tool {
            if !agent_after_patterns.contains(&hook.tool_match) {
                self.after_tool.push(hook);
            }
        }
        self
    }
}

/// Return the default context window size for a model.
///
/// Matches by model name substring. More specific patterns are checked before
/// catch-all family patterns. Returns a conservative default for unknown models.
///
/// Context windows sourced from OpenRouter model pages (Feb 2026).
fn default_context_window(model_name: &str) -> u32 {
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
    use duragent_types::provider::Provider;

    #[test]
    fn effective_max_input_tokens_with_explicit_value() {
        let config = ModelConfig {
            provider: Provider::OpenRouter,
            name: "anthropic/claude-sonnet-4".to_string(),
            temperature: None,
            max_input_tokens: Some(100_000),
            max_output_tokens: None,
            base_url: None,
        };
        assert_eq!(config.effective_max_input_tokens(), 100_000);
    }

    #[test]
    fn effective_max_input_tokens_defaults_from_model_name() {
        let config = ModelConfig {
            provider: Provider::OpenRouter,
            name: "anthropic/claude-sonnet-4".to_string(),
            temperature: None,
            max_input_tokens: None,
            max_output_tokens: None,
            base_url: None,
        };
        assert_eq!(config.effective_max_input_tokens(), 200_000);
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
