//! Session tool — peek into other sessions of the same agent.

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use crate::api::SessionStatus;
use crate::llm::{FunctionDefinition, Role, ToolDefinition};
use crate::session::SessionRegistry;
use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

// ============================================================================
// Tool struct
// ============================================================================

pub struct SessionTool {
    session_registry: SessionRegistry,
    session_id: String,
    agent_name: String,
}

impl SessionTool {
    pub fn new(session_registry: SessionRegistry, session_id: String, agent_name: String) -> Self {
        Self {
            session_registry,
            session_id,
            agent_name,
        }
    }
}

// ============================================================================
// Tool trait implementation
// ============================================================================

#[async_trait]
impl Tool for SessionTool {
    fn name(&self) -> &str {
        "session"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "session".to_string(),
                description: "Peek into other sessions of the same agent. Use 'list' to see sibling sessions, 'read' to view their messages. Useful for recalling context from conversations on other channels.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "read"],
                            "description": "Action: 'list' other sessions, or 'read' messages from one."
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Target session ID for 'read' action."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max messages to return for 'read' (default 20, max 50)."
                        }
                    },
                    "required": ["action"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: SessionArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "list" => self.action_list().await,
            "read" => {
                let target_id = args.session_id.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'session_id' is required for read action".to_string(),
                    )
                })?;
                let limit = args.limit.unwrap_or(20).clamp(1, 50) as usize;
                self.action_read(&target_id, limit).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!("Unknown action: '{}'. Use: list, read", other),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl SessionTool {
    async fn action_list(&self) -> Result<ToolResult, ToolError> {
        let all = self.session_registry.list().await;

        let mut siblings: Vec<_> = all
            .into_iter()
            .filter(|m| m.agent == self.agent_name)
            .filter(|m| m.id != self.session_id)
            .filter(|m| m.status != SessionStatus::Completed)
            .collect();

        siblings.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        siblings.truncate(10);

        if siblings.is_empty() {
            return Ok(ToolResult {
                success: true,
                content: format!("No other active sessions for agent '{}'.", self.agent_name),
            });
        }

        let now = Utc::now();
        let mut output = format!("Other sessions for agent '{}':\n", self.agent_name);
        for s in &siblings {
            let gateway = s.gateway.as_deref().unwrap_or("unknown");
            let age = format_relative_time(now, s.updated_at);
            output.push_str(&format!(
                "- {} ({}) — {} — {}\n",
                s.id, gateway, s.status, age
            ));
        }

        Ok(ToolResult {
            success: true,
            content: output,
        })
    }

    async fn action_read(&self, target_id: &str, limit: usize) -> Result<ToolResult, ToolError> {
        let handle = match self.session_registry.get(target_id) {
            Some(h) => h,
            None => {
                return Ok(ToolResult {
                    success: false,
                    content: format!("Session '{}' not found.", target_id),
                });
            }
        };

        if handle.agent() != self.agent_name {
            return Ok(ToolResult {
                success: false,
                content: "Cannot peek into sessions of a different agent.".to_string(),
            });
        }

        let messages = handle.get_messages().await.map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to read session messages: {}", e))
        })?;

        // Take the last N user/assistant messages (skip tool messages)
        let relevant: Vec<_> = messages
            .iter()
            .filter(|m| matches!(m.role, Role::User | Role::Assistant))
            .collect();

        let start = relevant.len().saturating_sub(limit);
        let slice = &relevant[start..];

        if slice.is_empty() {
            return Ok(ToolResult {
                success: true,
                content: format!("Session '{}' has no messages yet.", target_id),
            });
        }

        let mut output = String::new();
        for msg in slice {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                _ => continue,
            };
            let content = msg.content_str();
            // Truncate very long messages to keep output manageable
            let truncated = if content.len() > 500 {
                format!("{}...", &content[..500])
            } else {
                content.to_string()
            };
            output.push_str(&format!("[{}]: {}\n", role, truncated));
        }

        Ok(ToolResult {
            success: true,
            content: output,
        })
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn format_relative_time(now: chrono::DateTime<Utc>, then: chrono::DateTime<Utc>) -> String {
    let duration = now - then;
    let minutes = duration.num_minutes();

    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{}m ago", minutes)
    } else if minutes < 1440 {
        format!("{}h ago", minutes / 60)
    } else {
        format!("{}d ago", minutes / 1440)
    }
}

// ============================================================================
// Argument types
// ============================================================================

#[derive(Debug, Deserialize)]
struct SessionArgs {
    action: String,
    session_id: Option<String>,
    limit: Option<i64>,
}
