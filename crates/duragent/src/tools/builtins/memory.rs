//! Memory tool for agents.
//!
//! Consolidated tool with actions: recall, remember, reflect, update_world.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::memory::Memory;
use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

// ============================================================================
// Tool struct
// ============================================================================

/// Consolidated memory tool with actions: recall, remember, reflect, update_world.
pub struct MemoryTool {
    memory: Arc<Memory>,
}

impl MemoryTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

// ============================================================================
// Tool trait implementation
// ============================================================================

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "memory".to_string(),
                description: "Manage your persistent memory. Actions: 'recall' (load world knowledge, long-term memory, recent experiences), 'remember' (record to daily log), 'reflect' (read/update MEMORY.md), 'update_world' (write shared world knowledge). Call 'recall' when starting a new session or when you need context about the user, their projects, or prior conversations. Do not call it on every message.".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["recall", "remember", "reflect", "update_world"],
                            "description": "Action to perform"
                        },
                        "days": {
                            "type": "integer",
                            "description": "(recall) How many days of daily logs to load (default: 3)"
                        },
                        "content": {
                            "type": "string",
                            "description": "(remember) What to record. (reflect) New content for MEMORY.md — omit to read current memory first."
                        },
                        "topic": {
                            "type": "string",
                            "description": "(update_world) Topic file to update (e.g., 'people', 'systems', 'preferences')"
                        }
                    },
                    "required": ["action"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: MemoryArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "recall" => self.action_recall(args.days.unwrap_or(3)).await,
            "remember" => {
                let content = args.content.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'content' is required for remember action".to_string(),
                    )
                })?;
                if content.is_empty() {
                    return Err(ToolError::InvalidArguments(
                        "content is required".to_string(),
                    ));
                }
                self.action_remember(&content).await
            }
            "reflect" => self.action_reflect(args.content).await,
            "update_world" => {
                let topic = args.topic.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'topic' is required for update_world action".to_string(),
                    )
                })?;
                let content = args.content.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'content' is required for update_world action".to_string(),
                    )
                })?;
                self.action_update_world(&topic, &content).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!(
                    "Unknown action: '{}'. Use: recall, remember, reflect, update_world",
                    other
                ),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl MemoryTool {
    async fn action_recall(&self, days: usize) -> Result<ToolResult, ToolError> {
        let memory = self.memory.clone();
        let content = tokio::task::spawn_blocking(move || memory.recall(days))
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
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

    async fn action_remember(&self, content: &str) -> Result<ToolResult, ToolError> {
        let memory = self.memory.clone();
        let content = content.to_string();
        let path = tokio::task::spawn_blocking(move || memory.append_daily(&content))
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("Remembered. Written to {}", path.display()),
        })
    }

    async fn action_reflect(&self, content: Option<String>) -> Result<ToolResult, ToolError> {
        let memory = self.memory.clone();
        match content {
            None => {
                let current = tokio::task::spawn_blocking(move || memory.read_memory())
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

                if current.is_empty() {
                    Ok(ToolResult {
                        success: true,
                        content: "MEMORY.md is empty. Call reflect with content to create it."
                            .to_string(),
                    })
                } else {
                    Ok(ToolResult {
                        success: true,
                        content: format!("Current MEMORY.md:\n\n{}", current),
                    })
                }
            }
            Some(content) => {
                let path = tokio::task::spawn_blocking(move || memory.write_memory(&content))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

                Ok(ToolResult {
                    success: true,
                    content: format!("Reflected. Updated {}", path.display()),
                })
            }
        }
    }

    async fn action_update_world(
        &self,
        topic: &str,
        content: &str,
    ) -> Result<ToolResult, ToolError> {
        let memory = self.memory.clone();
        let topic = topic.to_string();
        let content = content.to_string();
        let path = tokio::task::spawn_blocking(move || memory.write_world(&topic, &content))
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            success: true,
            content: format!("Updated world knowledge at {}", path.display()),
        })
    }
}

// ============================================================================
// Private Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct MemoryArgs {
    action: String,
    #[serde(default)]
    days: Option<usize>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    topic: Option<String>,
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
    async fn recall_empty_memory() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool.execute(r#"{"action": "recall"}"#).await.unwrap();

        assert!(result.success);
        assert_eq!(result.content, "No memories found");
    }

    #[tokio::test]
    async fn recall_with_days_param() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory.clone());

        memory.append_daily("Test content").unwrap();

        let result = tool
            .execute(r#"{"action": "recall", "days": 1}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Test content"));
    }

    #[tokio::test]
    async fn remember_requires_content() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool.execute(r#"{"action": "remember"}"#).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn remember_appends() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool
            .execute(r#"{"action": "remember", "content": "Learned something"}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Remembered"));
    }

    #[tokio::test]
    async fn reflect_reads_empty_memory() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool.execute(r#"{"action": "reflect"}"#).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("empty"));
    }

    #[tokio::test]
    async fn reflect_reads_existing_memory() {
        let (_temp, memory) = setup();
        memory.write_memory("Existing notes").unwrap();
        let tool = MemoryTool::new(memory);

        let result = tool.execute(r#"{"action": "reflect"}"#).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Existing notes"));
    }

    #[tokio::test]
    async fn reflect_writes_memory() {
        let (temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool
            .execute(r#"{"action": "reflect", "content": "My curated memory notes"}"#)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Reflected"));

        let path = temp.path().join("agent-memory/MEMORY.md");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn update_world_requires_topic_and_content() {
        let (_temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        assert!(tool.execute(r#"{"action": "update_world"}"#).await.is_err());
        assert!(
            tool.execute(r#"{"action": "update_world", "topic": "test"}"#)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn update_world_writes() {
        let (temp, memory) = setup();
        let tool = MemoryTool::new(memory);

        let result = tool
            .execute(
                r#"{"action": "update_world", "topic": "people", "content": "Alice - Role: Engineer"}"#,
            )
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
