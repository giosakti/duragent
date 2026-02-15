//! Background process tool — spawn and manage background processes.
//!
//! Consolidated tool with actions: spawn, list, status, log, capture, send_keys, write, kill.

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

/// Consolidated background process tool.
pub struct BackgroundProcessTool {
    registry: ProcessRegistryHandle,
    session_id: String,
    agent: String,
    gateway: Option<String>,
    chat_id: Option<String>,
}

impl BackgroundProcessTool {
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
impl Tool for BackgroundProcessTool {
    fn name(&self) -> &str {
        "background_process"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "background_process".to_string(),
                description: "Manage background processes. Actions: 'spawn' (start a command), 'list' (all processes), 'status' (check one), 'log' (read output), 'capture' (interactive screen), 'send_keys' (send tmux key names), 'write' (type literal text), 'kill' (terminate), 'watch' (start screen watcher for tmux process — fires callback when screen stops changing), 'unwatch' (stop screen watcher). After spawning, you will receive a completion notification when the process finishes — do not poll in a loop. Kill an existing process before spawning a replacement.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["spawn", "list", "status", "log", "capture", "send_keys", "write", "kill", "watch", "unwatch"],
                            "description": "Action to perform"
                        },
                        "command": {
                            "type": "string",
                            "description": "(spawn) Shell command to execute (passed to bash -c)"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "(spawn) Working directory for the command"
                        },
                        "wait": {
                            "type": "boolean",
                            "description": "(spawn) If true, block until the process completes and return its output. Default: false"
                        },
                        "interactive": {
                            "type": "boolean",
                            "description": "(spawn) If true, run in interactive mode (terminal multiplexer). Default: false"
                        },
                        "label": {
                            "type": "string",
                            "description": "(spawn) Human-readable label for this process"
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "(spawn) Maximum runtime in seconds before the process is killed. Default: 1800"
                        },
                        "handle": {
                            "type": "string",
                            "description": "(status/log/capture/send_keys/write/kill) Process handle"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "(log) Line offset (default: 0)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "(log) Max lines (default: 100)"
                        },
                        "keys": {
                            "type": "string",
                            "description": "(send_keys) Space-separated tmux key names (e.g., 'Down Enter', 'C-c')"
                        },
                        "press_enter": {
                            "type": "boolean",
                            "description": "(send_keys) If true, press Enter after sending keys. Default: true"
                        },
                        "input": {
                            "type": "string",
                            "description": "(write) Literal text to write to the process (tmux or stdin). Append '\\n' to press Enter."
                        },
                        "interval_seconds": {
                            "type": "integer",
                            "description": "(watch) Silence timeout in seconds — fires callback after this long with no new output. Default: 5"
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
            "spawn" => {
                let command = args.command.as_deref().ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'command' is required for spawn action".to_string(),
                    )
                })?;
                self.action_spawn(command, &args).await
            }
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
            "watch" => {
                let handle = require_handle(&args)?;
                let interval = args.interval_seconds.unwrap_or(5);
                self.action_watch(&handle, interval).await
            }
            "unwatch" => {
                let handle = require_handle(&args)?;
                self.action_unwatch(&handle).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!(
                    "Unknown action: '{}'. Use: spawn, list, status, log, capture, send_keys, write, kill, watch, unwatch",
                    other
                ),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl BackgroundProcessTool {
    async fn action_spawn(
        &self,
        command: &str,
        args: &ProcessArgs,
    ) -> Result<ToolResult, ToolError> {
        let wait = args.wait.unwrap_or(false);
        let interactive = args.interactive.unwrap_or(false);
        let timeout = args.timeout_seconds.unwrap_or(1800);

        match self
            .registry
            .spawn(SpawnConfig {
                command,
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
            .write_input(handle, &self.session_id, input)
            .await
        {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Wrote to {}", handle),
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

    async fn action_watch(
        &self,
        handle: &str,
        interval_seconds: u64,
    ) -> Result<ToolResult, ToolError> {
        match self
            .registry
            .start_watcher(handle, &self.session_id, interval_seconds)
        {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!(
                    "Screen watcher started for {} (silence timeout: {}s)",
                    handle, interval_seconds
                ),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("{}", e),
            }),
        }
    }

    async fn action_unwatch(&self, handle: &str) -> Result<ToolResult, ToolError> {
        match self.registry.stop_watcher(handle, &self.session_id) {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Screen watcher stopped for {}", handle),
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
    command: Option<String>,
    workdir: Option<String>,
    wait: Option<bool>,
    interactive: Option<bool>,
    label: Option<String>,
    timeout_seconds: Option<u64>,
    handle: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
    keys: Option<String>,
    press_enter: Option<bool>,
    input: Option<String>,
    interval_seconds: Option<u64>,
}

fn require_handle(args: &ProcessArgs) -> Result<String, ToolError> {
    args.handle.clone().ok_or_else(|| {
        ToolError::InvalidArguments("'handle' is required for this action".to_string())
    })
}
