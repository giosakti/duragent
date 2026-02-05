//! Memory tools for agents.
//!
//! Four tools for memory operations:
//! - `recall` — Read memory context
//! - `remember` — Append to daily log
//! - `reflect` — Rewrite MEMORY.md
//! - `update_world` — Update shared world knowledge

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::memory::Memory;
use crate::tools::{Tool, ToolError, ToolResult};

// ============================================================================
// RecallTool
// ============================================================================

/// Tool to load memory context (world knowledge, agent memory, recent experiences).
pub struct RecallTool {
    memory: Arc<Memory>,
}

impl RecallTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

#[derive(Debug, Deserialize)]
struct RecallArgs {
    #[serde(default = "default_days")]
    days: usize,
}

fn default_days() -> usize {
    3
}

#[async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "recall".to_string(),
                description: "Load your memory context (world knowledge, your long-term memory, recent experiences)".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "days": {
                            "type": "integer",
                            "description": "How many days of daily logs to load (default: 3)"
                        }
                    }
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: RecallArgs = serde_json::from_str(arguments).unwrap_or(RecallArgs {
            days: default_days(),
        });

        let content = self
            .memory
            .recall(args.days)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if content.is_empty() {
            Ok(ToolResult {
                success: true,
                content: "No memories found".to_string(),
            })
        } else {
            Ok(ToolResult {
                success: true,
                content,
            })
        }
    }
}

// ============================================================================
// RememberTool
// ============================================================================

/// Tool to record an experience to today's daily log.
pub struct RememberTool {
    memory: Arc<Memory>,
}

impl RememberTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

#[derive(Debug, Deserialize)]
struct RememberArgs {
    content: String,
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "remember".to_string(),
                description: "Record an experience or observation to today's daily log".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "What to remember (will be appended to today's daily file)"
                        }
                    },
                    "required": ["content"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: RememberArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self
            .memory
            .append_daily(&args.content)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("Remembered. Written to {}", path.display()),
        })
    }
}

// ============================================================================
// ReflectTool
// ============================================================================

/// Tool to update long-term memory with curated learnings.
pub struct ReflectTool {
    memory: Arc<Memory>,
}

impl ReflectTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

#[derive(Debug, Deserialize)]
struct ReflectArgs {
    content: String,
}

#[async_trait]
impl Tool for ReflectTool {
    fn name(&self) -> &str {
        "reflect"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "reflect".to_string(),
                description: "Update your long-term memory with curated learnings. This replaces your MEMORY.md file.".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "New content for MEMORY.md (replaces existing)"
                        }
                    },
                    "required": ["content"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: ReflectArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self
            .memory
            .write_memory(&args.content)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("Reflected. Updated {}", path.display()),
        })
    }
}

// ============================================================================
// UpdateWorldTool
// ============================================================================

/// Tool to write shared world knowledge.
pub struct UpdateWorldTool {
    memory: Arc<Memory>,
}

impl UpdateWorldTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateWorldArgs {
    topic: String,
    content: String,
}

#[async_trait]
impl Tool for UpdateWorldTool {
    fn name(&self) -> &str {
        "update_world"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "update_world".to_string(),
                description: "Write shared world knowledge for a topic (visible to all agents). Replaces existing content for the topic.".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Topic file to update (e.g., 'people', 'systems', 'preferences')"
                        },
                        "content": {
                            "type": "string",
                            "description": "Full content for the topic file (replaces existing)"
                        }
                    },
                    "required": ["topic", "content"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: UpdateWorldArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self
            .memory
            .write_world(&args.topic, &args.content)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("Updated world knowledge at {}", path.display()),
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Arc<Memory>) {
        let temp = TempDir::new().unwrap();
        let world_dir = temp.path().join("world");
        let agent_memory_dir = temp.path().join("agent-memory");
        let memory = Arc::new(Memory::new(world_dir, agent_memory_dir));
        (temp, memory)
    }

    #[tokio::test]
    async fn recall_tool_empty_memory() {
        let (_temp, memory) = setup();
        let tool = RecallTool::new(memory);

        let result = tool.execute("{}").await.unwrap();

        assert!(result.success);
        assert_eq!(result.content, "No memories found");
    }

    #[tokio::test]
    async fn recall_tool_with_days_param() {
        let (_temp, memory) = setup();
        let tool = RecallTool::new(memory.clone());

        // Add some memory first
        memory.append_daily("Test content").unwrap();

        let result = tool.execute(r#"{"days": 1}"#).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Test content"));
    }

    #[tokio::test]
    async fn remember_tool_requires_content() {
        let (_temp, memory) = setup();
        let tool = RememberTool::new(memory);

        let result = tool.execute("{}").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn remember_tool_appends() {
        let (_temp, memory) = setup();
        let tool = RememberTool::new(memory);

        let result = tool
            .execute(r#"{"content": "Learned something"}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Remembered"));
    }

    #[tokio::test]
    async fn reflect_tool_requires_content() {
        let (_temp, memory) = setup();
        let tool = ReflectTool::new(memory);

        let result = tool.execute("{}").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reflect_tool_writes_memory() {
        let (temp, memory) = setup();
        let tool = ReflectTool::new(memory);

        let result = tool
            .execute(r#"{"content": "My curated memory notes"}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Reflected"));

        let path = temp.path().join("agent-memory/MEMORY.md");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn update_world_tool_requires_topic_and_content() {
        let (_temp, memory) = setup();
        let tool = UpdateWorldTool::new(memory);

        assert!(tool.execute("{}").await.is_err());
        assert!(tool.execute(r#"{"topic": "test"}"#).await.is_err());
        assert!(tool.execute(r#"{"content": "test"}"#).await.is_err());
    }

    #[tokio::test]
    async fn update_world_tool_writes() {
        let (temp, memory) = setup();
        let tool = UpdateWorldTool::new(memory);

        let result = tool
            .execute(r#"{"topic": "people", "content": "Alice - Role: Engineer"}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Updated world"));

        let path = temp.path().join("world/people.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "Alice - Role: Engineer");
    }
}
