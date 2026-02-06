//! Builder for constructing StructuredContext from various sources.

use std::collections::HashSet;

use crate::agent::AgentSpec;
use crate::llm::Message;

use super::{BlockSource, DirectiveEntry, StructuredContext, SystemBlock, priority};

/// Builder for constructing a StructuredContext.
///
/// Typical usage:
/// ```ignore
/// let context = ContextBuilder::new()
///     .from_agent_spec(&agent_spec)
///     .with_messages(history)
///     .with_tool_refs(tool_names)  // optional: filter which tools LLM sees
///     .build();
///
/// // Get tools from executor (single source of truth)
/// let tools = executor.tool_definitions(context.tool_refs.as_ref());
/// let request = context.render(model, temp, max_tokens, tools);
/// ```
#[derive(Debug, Default)]
pub struct ContextBuilder {
    context: StructuredContext,
}

impl ContextBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Populate context from an AgentSpec.
    ///
    /// Extracts soul, system_prompt, and instructions as separate blocks
    /// with appropriate priorities.
    pub fn from_agent_spec(mut self, spec: &AgentSpec) -> Self {
        if let Some(ref soul) = spec.soul {
            self.context.add_block(SystemBlock {
                content: soul.clone(),
                label: "soul".to_string(),
                source: BlockSource::AgentSpec,
                priority: priority::SOUL,
            });
        }

        if let Some(ref system_prompt) = spec.system_prompt {
            self.context.add_block(SystemBlock {
                content: system_prompt.clone(),
                label: "system_prompt".to_string(),
                source: BlockSource::AgentSpec,
                priority: priority::SYSTEM_PROMPT,
            });
        }

        if let Some(ref instructions) = spec.instructions {
            self.context.add_block(SystemBlock {
                content: instructions.clone(),
                label: "instructions".to_string(),
                source: BlockSource::AgentSpec,
                priority: priority::INSTRUCTIONS,
            });
        }

        self
    }

    /// Add a custom system block.
    pub fn add_block(mut self, block: SystemBlock) -> Self {
        self.context.add_block(block);
        self
    }

    /// Add pre-loaded directives to the context.
    pub fn with_directives(mut self, directives: Vec<DirectiveEntry>) -> Self {
        for directive in directives {
            self.context.add_directive(directive);
        }
        self
    }

    /// Set tool references to restrict which tools the LLM sees.
    /// Actual tool definitions come from ToolExecutor at render time.
    pub fn with_tool_refs(mut self, refs: HashSet<String>) -> Self {
        self.context.tool_refs = Some(refs);
        self
    }

    /// Add conversation history messages.
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.context.set_messages(messages);
        self
    }

    /// Build the final StructuredContext.
    pub fn build(self) -> StructuredContext {
        self.context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentMetadata, AgentSessionConfig, ModelConfig, ToolPolicy};
    use crate::llm::{Provider, Role};
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
            memory: None,
            tools: Vec::new(),
            policy: ToolPolicy::default(),
            agent_dir: PathBuf::from("/tmp/test-agent"),
        }
    }

    #[test]
    fn builder_from_agent_spec_all_fields() {
        let spec = test_agent_spec(
            Some("I am cheerful."),
            Some("You are helpful."),
            Some("Be concise."),
        );

        let ctx = ContextBuilder::new().from_agent_spec(&spec).build();

        assert_eq!(ctx.system_blocks.len(), 3);

        let msg = ctx.render_system_message().unwrap();
        assert_eq!(msg, "I am cheerful.\n\nYou are helpful.\n\nBe concise.");
    }

    #[test]
    fn builder_from_agent_spec_partial() {
        let spec = test_agent_spec(None, Some("You are helpful."), None);

        let ctx = ContextBuilder::new().from_agent_spec(&spec).build();

        assert_eq!(ctx.system_blocks.len(), 1);
        assert_eq!(ctx.system_blocks[0].label, "system_prompt");
    }

    #[test]
    fn builder_with_messages() {
        let ctx = ContextBuilder::new()
            .with_messages(vec![
                Message::text(Role::User, "Hello"),
                Message::text(Role::Assistant, "Hi there!"),
            ])
            .build();

        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn builder_chained() {
        let spec = test_agent_spec(Some("I am cheerful."), None, None);

        let ctx = ContextBuilder::new()
            .from_agent_spec(&spec)
            .with_messages(vec![Message::text(Role::User, "Hello")])
            .add_block(SystemBlock {
                content: "Extra instructions".to_string(),
                label: "extra".to_string(),
                source: BlockSource::Session,
                priority: priority::SESSION,
            })
            .build();

        assert_eq!(ctx.system_blocks.len(), 2);
        assert_eq!(ctx.messages.len(), 1);

        let msg = ctx.render_system_message().unwrap();
        assert!(msg.contains("I am cheerful."));
        assert!(msg.contains("Extra instructions"));
    }

    #[test]
    fn builder_renders_to_chat_request() {
        let spec = test_agent_spec(None, Some("You are helpful."), None);

        let ctx = ContextBuilder::new()
            .from_agent_spec(&spec)
            .with_messages(vec![Message::text(Role::User, "Hello")])
            .build();

        let request = ctx.render("gpt-4", Some(0.7), Some(1024), vec![]);

        assert_eq!(request.model, "gpt-4");
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.temperature, Some(0.7));
    }

    #[test]
    fn builder_with_tool_refs() {
        use std::collections::HashSet;

        let refs = HashSet::from_iter(["bash".to_string()]);

        let ctx = ContextBuilder::new().with_tool_refs(refs).build();

        assert!(ctx.tool_refs.is_some());
        assert_eq!(ctx.tool_refs.unwrap().len(), 1);
    }
}
