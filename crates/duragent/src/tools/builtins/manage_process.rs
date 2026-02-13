//! manage_process tool — interact with running background processes.

use async_trait::async_trait;
use serde::Deserialize;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::process::ProcessRegistryHandle;
use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

// ============================================================================
// Tool struct
// ============================================================================

pub struct ProcessTool {
    registry: ProcessRegistryHandle,
    session_id: String,
}

impl ProcessTool {
    pub fn new(registry: ProcessRegistryHandle, session_id: String) -> Self {
        Self {
            registry,
            session_id,
        }
    }
}

// ============================================================================
// Tool trait implementation
// ============================================================================

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "manage_process"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "manage_process".to_string(),
                description: "Interact with background processes. Actions: list (all processes), status (check one), log (read output), capture (interactive screen), send_keys (interactive input), write (stdin), kill (terminate).".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "status", "log", "capture", "send_keys", "write", "kill"],
                            "description": "Action to perform"
                        },
                        "handle": {
                            "type": "string",
                            "description": "Process handle (required for all actions except 'list')"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Line offset for 'log' action (default: 0)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max lines for 'log' action (default: 100)"
                        },
                        "keys": {
                            "type": "string",
                            "description": "Keystrokes for 'send_keys' action (key names like 'C-c', 'Enter', etc.)"
                        },
                        "press_enter": {
                            "type": "boolean",
                            "description": "If true, press Enter after sending keys. Default: true"
                        },
                        "input": {
                            "type": "string",
                            "description": "Text for 'write' action (written to stdin)"
                        }
                    },
                    "required": ["action"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: ProcessArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "list" => self.action_list().await,
            "status" => {
                let handle = require_handle(&args)?;
                self.action_status(&handle).await
            }
            "log" => {
                let handle = require_handle(&args)?;
                let offset = args.offset.unwrap_or(0).max(0) as usize;
                let limit = args.limit.unwrap_or(100).max(0) as usize;
                self.action_log(&handle, offset, limit).await
            }
            "capture" => {
                let handle = require_handle(&args)?;
                self.action_capture(&handle).await
            }
            "send_keys" => {
                let handle = require_handle(&args)?;
                let keys = args.keys.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'keys' is required for send_keys action".to_string(),
                    )
                })?;
                let press_enter = args.press_enter.unwrap_or(true);
                self.action_send_keys(&handle, &keys, press_enter).await
            }
            "write" => {
                let handle = require_handle(&args)?;
                let input = args.input.ok_or_else(|| {
                    ToolError::InvalidArguments("'input' is required for write action".to_string())
                })?;
                self.action_write(&handle, &input).await
            }
            "kill" => {
                let handle = require_handle(&args)?;
                self.action_kill(&handle).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!(
                    "Unknown action: '{}'. Use: list, status, log, capture, send_keys, write, kill",
                    other
                ),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl ProcessTool {
    async fn action_list(&self) -> Result<ToolResult, ToolError> {
        let processes = self.registry.list_by_session(&self.session_id);
        if processes.is_empty() {
            return Ok(ToolResult {
                success: true,
                content: "No background processes for this session.".to_string(),
            });
        }

        let mut output = String::new();
        for p in &processes {
            let label = p.label.as_deref().unwrap_or("-");
            let interactive = p
                .tmux_session
                .as_deref()
                .map(|s| format!(" [interactive: {}]", s))
                .unwrap_or_default();
            output.push_str(&format!(
                "- {} | {} | {} | {}{}\n",
                p.handle, p.command, p.status, label, interactive
            ));
        }

        Ok(ToolResult {
            success: true,
            content: output,
        })
    }

    async fn action_status(&self, handle: &str) -> Result<ToolResult, ToolError> {
        match self.registry.get_status(handle, &self.session_id) {
            Ok(meta) => {
                let json = serde_json::to_string_pretty(&meta)
                    .unwrap_or_else(|_| format!("Status: {}", meta.status));
                Ok(ToolResult {
                    success: true,
                    content: json,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_log(
        &self,
        handle: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ToolResult, ToolError> {
        match self
            .registry
            .read_log(handle, &self.session_id, offset, limit)
            .await
        {
            Ok(log_content) => Ok(ToolResult {
                success: true,
                content: if log_content.is_empty() {
                    "(no output yet)".to_string()
                } else {
                    log_content
                },
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_capture(&self, handle: &str) -> Result<ToolResult, ToolError> {
        match self.registry.capture(handle, &self.session_id).await {
            Ok(content) => Ok(ToolResult {
                success: true,
                content,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_send_keys(
        &self,
        handle: &str,
        keys: &str,
        press_enter: bool,
    ) -> Result<ToolResult, ToolError> {
        match self
            .registry
            .send_keys(handle, &self.session_id, keys, press_enter)
            .await
        {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Sent keys to {}", handle),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_write(&self, handle: &str, input: &str) -> Result<ToolResult, ToolError> {
        match self
            .registry
            .write_stdin(handle, &self.session_id, input)
            .await
        {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Wrote to stdin of {}", handle),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_kill(&self, handle: &str) -> Result<ToolResult, ToolError> {
        match self.registry.kill(handle, &self.session_id).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Killed process {}", handle),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }
}

// ============================================================================
// Argument types
// ============================================================================

#[derive(Debug, Deserialize)]
struct ProcessArgs {
    action: String,
    handle: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
    keys: Option<String>,
    press_enter: Option<bool>,
    input: Option<String>,
}

fn require_handle(args: &ProcessArgs) -> Result<String, ToolError> {
    args.handle.clone().ok_or_else(|| {
        ToolError::InvalidArguments("'handle' is required for this action".to_string())
    })
}
