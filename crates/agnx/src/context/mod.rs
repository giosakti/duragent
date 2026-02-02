//! Structured context for LLM requests.
//!
//! Context is built up from multiple sources (agent spec, hooks, runtime)
//! and rendered to a final `ChatRequest` only when needed. This allows
//! inspection, diffing, and modification of context components before
//! the final render.

mod builder;

pub use builder::ContextBuilder;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::llm::{ChatRequest, Message, Role, ToolDefinition};

// ============================================================================
// Core Types
// ============================================================================

/// Structured context that can be inspected and modified before rendering.
///
/// Unlike a raw string system prompt, this maintains provenance of each
/// component and allows late-binding of tools, directives, and memory.
#[derive(Debug, Clone, Default)]
pub struct StructuredContext {
    /// Ordered system blocks with provenance.
    pub system_blocks: Vec<SystemBlock>,
    /// Active directives (runtime instructions).
    pub directives: Vec<DirectiveEntry>,
    /// Tool definitions available to the model.
    pub tools: Vec<ToolDefinition>,
    /// If Some, only these tools are rendered to the LLM.
    /// If None, all tools are available. This is a context concern
    /// (what the LLM sees), not a policy concern (what's allowed to execute).
    pub tool_filter: Option<HashSet<String>>,
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

    /// Add a tool definition.
    pub fn add_tool(&mut self, tool: ToolDefinition) {
        self.tools.push(tool);
    }

    /// Set tool filter to restrict which tools the LLM sees.
    pub fn set_tool_filter(&mut self, filter: Option<HashSet<String>>) {
        self.tool_filter = filter;
    }

    /// Set conversation messages.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Render to a final ChatRequest.
    ///
    /// This combines all system blocks into a single system message,
    /// appends directives, and includes tools. Tools are filtered based
    /// on `tool_filter` if set.
    pub fn render(
        &self,
        model: &str,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> ChatRequest {
        let system_message = self.render_system_message();
        let mut messages = Vec::new();

        if let Some(content) = system_message {
            messages.push(Message::text(Role::System, content));
        }

        messages.extend(self.messages.iter().cloned());

        // Filter tools if a filter is set
        let filtered_tools = if let Some(ref filter) = self.tool_filter {
            self.tools
                .iter()
                .filter(|t| filter.contains(&t.function.name))
                .cloned()
                .collect()
        } else {
            self.tools.clone()
        };

        ChatRequest {
            model: model.to_string(),
            messages,
            temperature,
            max_tokens,
            tools: if filtered_tools.is_empty() {
                None
            } else {
                Some(filtered_tools)
            },
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

    /// Get a summary of the context (for minimal payloads).
    pub fn summary(&self) -> ContextSummary {
        ContextSummary {
            block_count: self.system_blocks.len(),
            directive_count: self.directives.len(),
            tool_count: self.tools.len(),
            message_count: self.messages.len(),
        }
    }
}

/// Summary of context for minimal hook payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    pub block_count: usize,
    pub directive_count: usize,
    pub tool_count: usize,
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

        let request = ctx.render("gpt-4", Some(0.7), Some(1024));

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
        assert_eq!(summary.tool_count, 0);
        assert_eq!(summary.message_count, 2);
    }

    // ------------------------------------------------------------------------
    // Tool filtering
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
    fn render_without_filter_includes_all_tools() {
        let mut ctx = StructuredContext::new();
        ctx.tools = vec![make_tool("bash"), make_tool("deploy"), make_tool("git")];
        // No filter set (default)

        let request = ctx.render("gpt-4", None, None);

        let tools = request.tools.unwrap();
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn render_with_empty_filter_includes_no_tools() {
        let mut ctx = StructuredContext::new();
        ctx.tools = vec![make_tool("bash"), make_tool("deploy")];
        ctx.tool_filter = Some(HashSet::new());

        let request = ctx.render("gpt-4", None, None);

        assert!(request.tools.is_none());
    }

    #[test]
    fn render_with_filter_restricts_tools() {
        let mut ctx = StructuredContext::new();
        ctx.tools = vec![make_tool("bash"), make_tool("deploy"), make_tool("git")];
        ctx.tool_filter = Some(HashSet::from_iter(["bash".to_string(), "git".to_string()]));

        let request = ctx.render("gpt-4", None, None);

        let tools = request.tools.unwrap();
        assert_eq!(tools.len(), 2);
        let names: Vec<_> = tools.iter().map(|t| &t.function.name).collect();
        assert!(names.contains(&&"bash".to_string()));
        assert!(names.contains(&&"git".to_string()));
        assert!(!names.contains(&&"deploy".to_string()));
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
}
