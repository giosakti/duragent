//! Structured context for LLM requests.
//!
//! Context is built up from multiple sources (agent spec, hooks, runtime)
//! and rendered to a final `ChatRequest` only when needed. This allows
//! inspection, diffing, and modification of context components before
//! the final render.

mod builder;
mod directives;
mod tokens;
mod truncation;

pub use builder::ContextBuilder;
pub use directives::{ensure_memory_directive, load_all_directives};
pub use tokens::*;
pub use truncation::*;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::llm::{ChatRequest, Message, Role};

// ============================================================================
// Core Types
// ============================================================================

/// Structured context that can be inspected and modified before rendering.
///
/// Unlike a raw string system prompt, this maintains provenance of each
/// component and allows late-binding of tools, directives, and memory.
///
/// Tools are NOT stored here — they live in `ToolExecutor` (single source of truth).
/// Use `tool_refs` to specify which tools should be visible to the LLM.
#[derive(Debug, Clone, Default)]
pub struct StructuredContext {
    /// Ordered system blocks with provenance.
    pub system_blocks: Vec<SystemBlock>,
    /// Active directives (runtime instructions).
    pub directives: Vec<DirectiveEntry>,
    /// Tool references (IDs/names) to include in the request.
    /// If None, all tools from executor are included.
    /// If Some, only tools with matching names are included.
    /// Actual definitions come from ToolExecutor at render time.
    pub tool_refs: Option<HashSet<String>>,
    /// Conversation messages (user/assistant history).
    pub messages: Vec<Message>,
}

/// A labeled block of system content with source tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    /// The content of this block.
    pub content: String,
    /// Human-readable label (e.g., "soul", "system_prompt", "skill:deploy").
    pub label: String,
    /// Where this block came from.
    pub source: BlockSource,
    /// Ordering priority (lower runs first).
    pub priority: i32,
}

/// Source of a system block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlockSource {
    /// From agent YAML spec (soul, system_prompt, instructions).
    AgentSpec,
    /// From overlay file.
    Overlay,
    /// From a hook.
    Hook { name: String },
    /// From an active skill.
    Skill { name: String },
    /// Session-specific (transient).
    Session,
}

/// A runtime directive (instruction) with scope and provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectiveEntry {
    /// The instruction text.
    pub instruction: String,
    /// Persistence scope.
    pub scope: Scope,
    /// What added this directive (hook name, tool, etc.).
    pub source: String,
    /// When this directive was added.
    pub added_at: DateTime<Utc>,
}

/// Persistence scope for directives and preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Scope {
    /// Dies with session (stored in snapshot only).
    #[default]
    Session,
    /// Persists to agent overlay file.
    Agent,
    /// Global across all agents (typically disallowed).
    Global,
}

/// Token budget for render-time truncation.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum input tokens for the model.
    pub max_input_tokens: u32,
    /// Maximum output tokens reserved.
    pub max_output_tokens: u32,
    /// Maximum tokens for conversation history (0 = no cap).
    pub max_history_tokens: u32,
}

// ============================================================================
// Rendering
// ============================================================================

/// Priority constants for system blocks.
pub mod priority {
    /// Soul (personality) comes first.
    pub const SOUL: i32 = 0;
    /// Core system prompt.
    pub const SYSTEM_PROMPT: i32 = 100;
    /// Agent-level instructions.
    pub const INSTRUCTIONS: i32 = 200;
    /// Hook-injected blocks.
    pub const HOOK: i32 = 300;
    /// Skill-injected blocks (active skill context).
    pub const SKILL: i32 = 400;
    /// Session-specific additions.
    pub const SESSION: i32 = 500;
    /// Runtime directives.
    pub const DIRECTIVES: i32 = 600;
}

impl StructuredContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a system block.
    pub fn add_block(&mut self, block: SystemBlock) {
        self.system_blocks.push(block);
    }

    /// Add a directive.
    pub fn add_directive(&mut self, directive: DirectiveEntry) {
        self.directives.push(directive);
    }

    /// Set tool references (IDs/names) to include in requests.
    /// Pass None to include all tools, or Some with specific names to filter.
    pub fn set_tool_refs(&mut self, refs: Option<HashSet<String>>) {
        self.tool_refs = refs;
    }

    /// Set conversation messages.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Render to a final ChatRequest.
    ///
    /// This combines all system blocks into a single system message,
    /// appends directives, and includes the provided tools.
    ///
    /// Tools should be pre-resolved from `ToolExecutor::tool_definitions()`
    /// with `tool_refs` passed as the filter. This ensures:
    /// - Single source of truth (executor owns tool definitions)
    /// - Visibility guarantees (only see tools executor knows)
    /// - Execution guarantees (only call tools executor can run)
    pub fn render(
        &self,
        model: &str,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tools: Vec<crate::llm::ToolDefinition>,
    ) -> ChatRequest {
        let system_message = self.render_system_message();
        let mut messages = Vec::new();

        if let Some(content) = system_message {
            messages.push(Message::text(Role::System, content));
        }

        messages.extend(self.messages.iter().cloned());

        ChatRequest {
            model: model.to_string(),
            messages,
            temperature,
            max_tokens,
            tools: if tools.is_empty() { None } else { Some(tools) },
        }
    }

    /// Render to a final ChatRequest with token budget enforcement.
    ///
    /// Like `render()`, but applies a token budget to conversation history:
    /// 1. Compute available budget: max_input - system - tools - output_reserve - 10% safety
    /// 2. Apply max_history_tokens cap if > 0
    /// 3. Fill messages from newest to oldest within budget
    /// 4. Inject truncation notice when messages are omitted
    pub fn render_with_budget(
        &self,
        model: &str,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tools: Vec<crate::llm::ToolDefinition>,
        budget: &TokenBudget,
    ) -> ChatRequest {
        let system_message = self.render_system_message();
        let mut messages = Vec::new();

        // Calculate system message tokens
        let system_tokens = system_message
            .as_ref()
            .map(|s| estimate_tokens(s))
            .unwrap_or(0);

        // Calculate tool definition tokens
        let tool_tokens = estimate_tool_definitions_tokens(&tools);

        // Safety margin: 10% of max_input_tokens
        let safety_margin = budget.max_input_tokens / 10;

        // Available budget for history
        let reserved = system_tokens + tool_tokens + budget.max_output_tokens + safety_margin;
        let available = budget.max_input_tokens.saturating_sub(reserved);

        // Apply max_history_tokens cap (0 = no cap)
        let history_budget = if budget.max_history_tokens > 0 {
            available.min(budget.max_history_tokens)
        } else {
            available
        };

        // Add system message
        if let Some(content) = system_message {
            messages.push(Message::text(Role::System, content));
        }

        // Fill messages from newest to oldest within budget
        let mut used_tokens = 0u32;
        let mut included_from = self.messages.len();

        for (i, msg) in self.messages.iter().enumerate().rev() {
            let msg_tokens = estimate_message_tokens(msg);
            if used_tokens + msg_tokens > history_budget {
                break;
            }
            used_tokens += msg_tokens;
            included_from = i;
        }

        // Inject truncation notice if messages were omitted
        let omitted = included_from;
        if omitted > 0 {
            messages.push(Message::text(
                Role::System,
                format!(
                    "[conversation truncated — {} older messages omitted]",
                    omitted
                ),
            ));
        }

        messages.extend(self.messages[included_from..].iter().cloned());

        ChatRequest {
            model: model.to_string(),
            messages,
            temperature,
            max_tokens,
            tools: if tools.is_empty() { None } else { Some(tools) },
        }
    }

    /// Get a summary of the context (for minimal payloads).
    pub fn summary(&self) -> ContextSummary {
        ContextSummary {
            block_count: self.system_blocks.len(),
            directive_count: self.directives.len(),
            tool_ref_count: self.tool_refs.as_ref().map(|r| r.len()),
            message_count: self.messages.len(),
        }
    }

    /// Render just the system message (for inspection/debugging).
    pub fn render_system_message(&self) -> Option<String> {
        if self.system_blocks.is_empty() && self.directives.is_empty() {
            return None;
        }

        let mut output = String::new();

        // Sort blocks by priority, then by insertion order (stable sort)
        let mut blocks: Vec<_> = self.system_blocks.iter().collect();
        blocks.sort_by_key(|b| b.priority);

        for block in blocks {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(&block.content);
        }

        // Append directives if any
        if !self.directives.is_empty() {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str("## Directives\n");
            for directive in &self.directives {
                output.push_str(&format!("- {}\n", directive.instruction));
            }
        }

        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }
}

/// Summary of context for minimal hook payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    pub block_count: usize,
    pub directive_count: usize,
    /// Number of tool refs, or None if all tools are included.
    pub tool_ref_count: Option<usize>,
    pub message_count: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_renders_none() {
        let ctx = StructuredContext::new();
        assert!(ctx.render_system_message().is_none());
    }

    #[test]
    fn single_block_renders() {
        let mut ctx = StructuredContext::new();
        ctx.add_block(SystemBlock {
            content: "You are helpful.".to_string(),
            label: "system_prompt".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SYSTEM_PROMPT,
        });

        let msg = ctx.render_system_message().unwrap();
        assert_eq!(msg, "You are helpful.");
    }

    #[test]
    fn blocks_sorted_by_priority() {
        let mut ctx = StructuredContext::new();

        // Add in reverse order
        ctx.add_block(SystemBlock {
            content: "Be concise.".to_string(),
            label: "instructions".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::INSTRUCTIONS,
        });
        ctx.add_block(SystemBlock {
            content: "I am cheerful.".to_string(),
            label: "soul".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SOUL,
        });
        ctx.add_block(SystemBlock {
            content: "You are helpful.".to_string(),
            label: "system_prompt".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SYSTEM_PROMPT,
        });

        let msg = ctx.render_system_message().unwrap();
        assert_eq!(msg, "I am cheerful.\n\nYou are helpful.\n\nBe concise.");
    }

    #[test]
    fn directives_appended() {
        let mut ctx = StructuredContext::new();
        ctx.add_block(SystemBlock {
            content: "You are helpful.".to_string(),
            label: "system_prompt".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SYSTEM_PROMPT,
        });
        ctx.add_directive(DirectiveEntry {
            instruction: "Use memory-tool-b for recall".to_string(),
            scope: Scope::Session,
            source: "agent".to_string(),
            added_at: Utc::now(),
        });

        let msg = ctx.render_system_message().unwrap();
        assert!(msg.contains("You are helpful."));
        assert!(msg.contains("## Directives"));
        assert!(msg.contains("- Use memory-tool-b for recall"));
    }

    #[test]
    fn render_produces_chat_request() {
        let mut ctx = StructuredContext::new();
        ctx.add_block(SystemBlock {
            content: "You are helpful.".to_string(),
            label: "system_prompt".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SYSTEM_PROMPT,
        });
        ctx.set_messages(vec![Message::text(Role::User, "Hello")]);

        let request = ctx.render("gpt-4", Some(0.7), Some(1024), vec![]);

        assert_eq!(request.model, "gpt-4");
        assert_eq!(request.messages.len(), 2); // system + user
        assert_eq!(request.messages[0].role, Role::System);
        assert_eq!(request.messages[1].role, Role::User);
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.max_tokens, Some(1024));
    }

    #[test]
    fn summary_counts_correctly() {
        let mut ctx = StructuredContext::new();
        ctx.add_block(SystemBlock {
            content: "block1".to_string(),
            label: "b1".to_string(),
            source: BlockSource::AgentSpec,
            priority: 0,
        });
        ctx.add_block(SystemBlock {
            content: "block2".to_string(),
            label: "b2".to_string(),
            source: BlockSource::AgentSpec,
            priority: 0,
        });
        ctx.add_directive(DirectiveEntry {
            instruction: "directive".to_string(),
            scope: Scope::Session,
            source: "test".to_string(),
            added_at: Utc::now(),
        });
        ctx.set_messages(vec![
            Message::text(Role::User, "hi"),
            Message::text(Role::Assistant, "hello"),
        ]);

        let summary = ctx.summary();
        assert_eq!(summary.block_count, 2);
        assert_eq!(summary.directive_count, 1);
        assert!(summary.tool_ref_count.is_none()); // No refs set = all tools
        assert_eq!(summary.message_count, 2);
    }

    // ------------------------------------------------------------------------
    // Tool refs and rendering
    // ------------------------------------------------------------------------

    fn make_tool(name: &str) -> crate::llm::ToolDefinition {
        crate::llm::ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::llm::FunctionDefinition {
                name: name.to_string(),
                description: format!("Tool: {}", name),
                parameters: None,
            },
        }
    }

    #[test]
    fn render_with_tools_includes_them() {
        let ctx = StructuredContext::new();
        let tools = vec![make_tool("bash"), make_tool("deploy")];

        let request = ctx.render("gpt-4", None, None, tools);

        let result_tools = request.tools.unwrap();
        assert_eq!(result_tools.len(), 2);
    }

    #[test]
    fn render_with_empty_tools_sets_none() {
        let ctx = StructuredContext::new();

        let request = ctx.render("gpt-4", None, None, vec![]);

        assert!(request.tools.is_none());
    }

    #[test]
    fn tool_refs_tracked_in_summary() {
        let mut ctx = StructuredContext::new();
        ctx.set_tool_refs(Some(HashSet::from_iter([
            "bash".to_string(),
            "git".to_string(),
        ])));

        let summary = ctx.summary();
        assert_eq!(summary.tool_ref_count, Some(2));
    }

    #[test]
    fn block_source_skill_variant() {
        let source = BlockSource::Skill {
            name: "deploy-skill".to_string(),
        };
        assert_eq!(
            source,
            BlockSource::Skill {
                name: "deploy-skill".to_string()
            }
        );
    }

    // ------------------------------------------------------------------------
    // render_with_budget tests
    // ------------------------------------------------------------------------

    #[test]
    fn render_with_budget_no_truncation() {
        let mut ctx = StructuredContext::new();
        ctx.set_messages(vec![
            Message::text(Role::User, "Hello"),
            Message::text(Role::Assistant, "Hi"),
        ]);

        let budget = TokenBudget {
            max_input_tokens: 200_000,
            max_output_tokens: 4096,
            max_history_tokens: 0, // no cap
        };

        let request = ctx.render_with_budget("gpt-4", None, None, vec![], &budget);
        // No system message, no truncation notice, just 2 messages
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, Role::User);
        assert_eq!(request.messages[1].role, Role::Assistant);
    }

    #[test]
    fn render_with_budget_truncates_old_messages() {
        let mut ctx = StructuredContext::new();
        // Create many messages that will exceed a small budget
        for i in 0..20 {
            ctx.messages
                .push(Message::text(Role::User, format!("Message {}", i)));
        }

        let budget = TokenBudget {
            max_input_tokens: 100, // Very small
            max_output_tokens: 10,
            max_history_tokens: 0, // no cap, use available
        };

        let request = ctx.render_with_budget("gpt-4", None, None, vec![], &budget);
        // Should have fewer than 20 messages + truncation notice
        assert!(request.messages.len() < 21);
        // First message should be the truncation notice
        assert!(
            request.messages[0]
                .content_str()
                .contains("conversation truncated")
        );
    }

    #[test]
    fn render_with_budget_max_history_tokens_caps() {
        let mut ctx = StructuredContext::new();
        for i in 0..50 {
            ctx.messages
                .push(Message::text(Role::User, format!("Message {}", i)));
        }

        let budget = TokenBudget {
            max_input_tokens: 200_000,
            max_output_tokens: 4096,
            max_history_tokens: 20, // Very small cap
        };

        let request = ctx.render_with_budget("gpt-4", None, None, vec![], &budget);
        // Should be truncated due to max_history_tokens cap
        assert!(request.messages.len() < 51);
        assert!(
            request.messages[0]
                .content_str()
                .contains("conversation truncated")
        );
    }

    #[test]
    fn render_with_budget_includes_system_message() {
        let mut ctx = StructuredContext::new();
        ctx.add_block(SystemBlock {
            content: "You are helpful.".to_string(),
            label: "system_prompt".to_string(),
            source: BlockSource::AgentSpec,
            priority: priority::SYSTEM_PROMPT,
        });
        ctx.set_messages(vec![Message::text(Role::User, "Hello")]);

        let budget = TokenBudget {
            max_input_tokens: 200_000,
            max_output_tokens: 4096,
            max_history_tokens: 0,
        };

        let request = ctx.render_with_budget("gpt-4", None, None, vec![], &budget);
        assert_eq!(request.messages[0].role, Role::System);
        assert_eq!(request.messages[0].content_str(), "You are helpful.");
        assert_eq!(request.messages[1].role, Role::User);
    }
}
