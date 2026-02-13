//! spawn_process tool — start a background command.

use async_trait::async_trait;
use serde::Deserialize;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::process::ProcessRegistryHandle;
use crate::process::registry::{SpawnConfig, SpawnOrWait};
use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

// ============================================================================
// Tool struct
// ============================================================================

pub struct SpawnProcessTool {
    registry: ProcessRegistryHandle,
    session_id: String,
    agent: String,
    gateway: Option<String>,
    chat_id: Option<String>,
}

impl SpawnProcessTool {
    pub fn new(
        registry: ProcessRegistryHandle,
        session_id: String,
        agent: String,
        gateway: Option<String>,
        chat_id: Option<String>,
    ) -> Self {
        Self {
            registry,
            session_id,
            agent,
            gateway,
            chat_id,
        }
    }
}

// ============================================================================
// Tool trait implementation
// ============================================================================

#[async_trait]
impl Tool for SpawnProcessTool {
    fn name(&self) -> &str {
        "spawn_process"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "spawn_process".to_string(),
                description: "Spawn a background process. Use wait:true to block until completion, or wait:false to get a handle for later monitoring. Use interactive:true to enable human observation and agent interaction via capture/send_keys.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute (passed to bash -c)"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Working directory for the command"
                        },
                        "wait": {
                            "type": "boolean",
                            "description": "If true, block until the process completes and return its output. Default: false"
                        },
                        "interactive": {
                            "type": "boolean",
                            "description": "If true, run in interactive mode (terminal multiplexer) for human observation and agent interaction via capture/send_keys. Requires tmux. Default: false"
                        },
                        "label": {
                            "type": "string",
                            "description": "Human-readable label for this process"
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum runtime in seconds before the process is killed. Default: 1800 (30 minutes)"
                        }
                    },
                    "required": ["command"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: SpawnArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let wait = args.wait.unwrap_or(false);
        let interactive = args.interactive.unwrap_or(false);
        let timeout = args.timeout_seconds.unwrap_or(1800);

        match self
            .registry
            .spawn(SpawnConfig {
                command: &args.command,
                workdir: args.workdir.as_deref(),
                wait,
                interactive,
                label: args.label.as_deref(),
                timeout_seconds: timeout,
                session_id: &self.session_id,
                agent: &self.agent,
                gateway: self.gateway.as_deref(),
                chat_id: self.chat_id.as_deref(),
            })
            .await
        {
            Ok(SpawnOrWait::Spawned(result)) => Ok(ToolResult {
                success: true,
                content: serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| format!("Process spawned: {}", result.handle)),
            }),
            Ok(SpawnOrWait::Waited(result)) => Ok(ToolResult {
                success: result.exit_code == 0,
                content: serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| format!("Process finished: exit {}", result.exit_code)),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("Failed to spawn process: {}", e),
            }),
        }
    }
}

// ============================================================================
// Argument types
// ============================================================================

#[derive(Debug, Deserialize)]
struct SpawnArgs {
    command: String,
    workdir: Option<String>,
    wait: Option<bool>,
    interactive: Option<bool>,
    label: Option<String>,
    timeout_seconds: Option<u64>,
}
