//! Bash tool implementation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::sandbox::Sandbox;

use super::error::ToolError;
use super::executor::ToolResult;
use super::tool::Tool;

/// The bash tool for executing shell commands.
pub struct BashTool {
    sandbox: Arc<dyn Sandbox>,
    agent_dir: PathBuf,
}

impl BashTool {
    /// Create a new bash tool.
    pub fn new(sandbox: Arc<dyn Sandbox>, agent_dir: PathBuf) -> Self {
        Self { sandbox, agent_dir }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn definition(&self) -> ToolDefinition {
        definition()
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute(&self.sandbox, &self.agent_dir, arguments).await
    }
}

/// Execute the bash tool.
pub async fn execute(
    sandbox: &Arc<dyn Sandbox>,
    agent_dir: &Path,
    arguments: &str,
) -> Result<ToolResult, ToolError> {
    // Parse arguments JSON
    let args: BashArgs =
        serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

    // Execute via sandbox (uses default timeout)
    let result = sandbox
        .exec(
            "bash",
            &["-c".to_string(), args.command],
            Some(agent_dir),
            None,
        )
        .await?;

    Ok(ToolResult::from_exec(result))
}

/// Generate the tool definition for the bash tool.
pub fn definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "bash".to_string(),
            description: "Execute a bash command. Use this to run shell commands, interact with the filesystem, or execute scripts.".to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    }
                },
                "required": ["command"]
            })),
        },
    }
}

// ============================================================================
// Private Types
// ============================================================================

/// Arguments for the bash tool.
#[derive(serde::Deserialize)]
struct BashArgs {
    command: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_command() {
        let def = definition();
        assert_eq!(def.function.name, "bash");
        assert!(def.function.parameters.is_some());

        let params = def.function.parameters.unwrap();
        assert!(
            params["required"]
                .as_array()
                .unwrap()
                .contains(&"command".into())
        );
    }
}
