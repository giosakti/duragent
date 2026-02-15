//! Schedule management tool for agents.
//!
//! Consolidated tool for creating, listing, and cancelling schedules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::scheduler::{
    RetryConfig, Schedule, ScheduleDestination, SchedulePayload, ScheduleStatus, ScheduleTiming,
    SchedulerHandle,
};

use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

/// Context needed for schedule tool execution.
#[derive(Debug, Clone)]
pub struct ToolExecutionContext {
    /// Gateway the message came from.
    pub gateway: Option<String>,
    /// Chat ID the message came from.
    pub chat_id: Option<String>,
    /// Agent name.
    pub agent: String,
    /// Session ID.
    pub session_id: String,
}

// ============================================================================
// Tool Struct
// ============================================================================

/// Consolidated schedule tool with actions: create, list, cancel.
pub struct ScheduleTool {
    scheduler: SchedulerHandle,
    ctx: ToolExecutionContext,
}

impl ScheduleTool {
    /// Create a new schedule tool.
    pub fn new(scheduler: SchedulerHandle, ctx: ToolExecutionContext) -> Self {
        Self { scheduler, ctx }
    }
}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "schedule".to_string(),
                description: "Manage scheduled tasks. Actions: 'create' a one-shot reminder or recurring task, 'list' active schedules, 'cancel' a schedule.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["create", "list", "cancel"],
                            "description": "Action to perform"
                        },
                        "at": {
                            "type": "string",
                            "description": "(create) One-shot: ISO8601 timestamp for when to fire (e.g., '2026-01-30T16:00:00Z'). Fires once then auto-completes."
                        },
                        "every_seconds": {
                            "type": "integer",
                            "description": "(create) Recurring: Interval in seconds. Only use when user explicitly wants repetition."
                        },
                        "cron": {
                            "type": "string",
                            "description": "(create) Recurring: Cron expression (e.g., '0 9 * * MON-FRI'). Only use when user explicitly wants a recurring schedule."
                        },
                        "message": {
                            "type": "string",
                            "description": "(create) Simple message to deliver (no LLM call, just sends the text)"
                        },
                        "task": {
                            "type": "string",
                            "description": "(create) Task to execute with LLM and tools, then report results"
                        },
                        "max_retries": {
                            "type": "integer",
                            "description": "(create) Number of retry attempts on failure (default: 3)"
                        },
                        "retry_delay_ms": {
                            "type": "integer",
                            "description": "(create) Initial delay before first retry in milliseconds (default: 1000)"
                        },
                        "max_retry_delay_ms": {
                            "type": "integer",
                            "description": "(create) Maximum delay between retries in milliseconds (default: 30000)"
                        },
                        "process_handle": {
                            "type": "string",
                            "description": "(create) Link to a background process handle. The schedule is auto-cancelled when the process exits."
                        },
                        "schedule_id": {
                            "type": "string",
                            "description": "(cancel) The ID of the schedule to cancel"
                        }
                    },
                    "required": ["action"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: ScheduleArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "create" => self.action_create(&args).await,
            "list" => self.action_list().await,
            "cancel" => {
                let id = args.schedule_id.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'schedule_id' is required for cancel action".to_string(),
                    )
                })?;
                self.action_cancel(&id).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!("Unknown action: '{}'. Use: create, list, cancel", other),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl ScheduleTool {
    async fn action_create(&self, args: &ScheduleArgs) -> Result<ToolResult, ToolError> {
        // Require gateway context
        let gateway = self
            .ctx
            .gateway
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("No gateway context".to_string()))?;
        let chat_id = self
            .ctx
            .chat_id
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("No chat_id context".to_string()))?;

        // Validate: exactly one timing option
        let timing_count = [
            args.at.is_some(),
            args.every_seconds.is_some(),
            args.cron.is_some(),
        ]
        .iter()
        .filter(|&&x| x)
        .count();

        if timing_count == 0 {
            return Ok(ToolResult {
                success: false,
                content: "Error: Must specify one of: at, every_seconds, or cron".to_string(),
            });
        }
        if timing_count > 1 {
            return Ok(ToolResult {
                success: false,
                content: "Error: Can only specify one timing option (at, every_seconds, or cron)"
                    .to_string(),
            });
        }
        if let Some(every) = args.every_seconds {
            if every == 0 {
                return Ok(ToolResult {
                    success: false,
                    content: "Error: every_seconds must be >= 1".to_string(),
                });
            }
        }

        // Validate: exactly one payload option
        let payload_count = [args.message.is_some(), args.task.is_some()]
            .iter()
            .filter(|&&x| x)
            .count();

        if payload_count == 0 {
            return Ok(ToolResult {
                success: false,
                content: "Error: Must specify one of: message or task".to_string(),
            });
        }
        if payload_count > 1 {
            return Ok(ToolResult {
                success: false,
                content: "Error: Can only specify one payload option (message or task)".to_string(),
            });
        }

        // Build timing
        let timing = if let Some(ref at_str) = args.at {
            let at: DateTime<Utc> = at_str.parse().map_err(|e| {
                ToolError::InvalidArguments(format!("Invalid timestamp '{}': {}", at_str, e))
            })?;
            ScheduleTiming::At { at }
        } else if let Some(every) = args.every_seconds {
            ScheduleTiming::Every {
                every_seconds: every,
                anchor: Some(Utc::now()),
            }
        } else if let Some(ref cron) = args.cron {
            ScheduleTiming::Cron {
                expr: cron.clone(),
                tz: None,
            }
        } else {
            return Ok(ToolResult {
                success: false,
                content: "Error: no timing option resolved".to_string(),
            });
        };

        // Build payload
        let payload = if let Some(ref message) = args.message {
            SchedulePayload::Message {
                message: message.clone(),
            }
        } else if let Some(ref task) = args.task {
            SchedulePayload::Task { task: task.clone() }
        } else {
            return Ok(ToolResult {
                success: false,
                content: "Error: no payload option resolved".to_string(),
            });
        };

        // Build retry config if any retry parameters are provided
        let retry = if args.max_retries.is_some()
            || args.retry_delay_ms.is_some()
            || args.max_retry_delay_ms.is_some()
        {
            let default = RetryConfig::default();
            Some(RetryConfig {
                max_retries: args.max_retries.unwrap_or(default.max_retries),
                initial_delay_ms: args.retry_delay_ms.unwrap_or(default.initial_delay_ms),
                max_delay_ms: args.max_retry_delay_ms.unwrap_or(default.max_delay_ms),
            })
        } else {
            None
        };

        // Create schedule
        let schedule = Schedule {
            id: Schedule::generate_id(),
            agent: self.ctx.agent.clone(),
            created_by_session: self.ctx.session_id.clone(),
            destination: ScheduleDestination {
                gateway: gateway.clone(),
                chat_id: chat_id.clone(),
            },
            timing,
            payload,
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry,
            process_handle: args.process_handle.clone(),
        };

        let id = schedule.id.clone();
        let is_recurring = schedule.is_recurring();

        match self.scheduler.create_schedule(schedule).await {
            Ok(_) => {
                let schedule_type = if is_recurring {
                    "recurring"
                } else {
                    "one-shot"
                };
                Ok(ToolResult {
                    success: true,
                    content: format!("Created {} schedule: {}", schedule_type, id),
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("Failed to create schedule: {}", e),
            }),
        }
    }

    async fn action_list(&self) -> Result<ToolResult, ToolError> {
        let schedules = self.scheduler.list_schedules(&self.ctx.agent).await;

        if schedules.is_empty() {
            return Ok(ToolResult {
                success: true,
                content: "No active schedules.".to_string(),
            });
        }

        #[derive(Serialize)]
        struct ScheduleSummary {
            id: String,
            timing: String,
            payload_type: String,
            created_at: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            process_handle: Option<String>,
        }

        let summaries: Vec<ScheduleSummary> = schedules
            .iter()
            .map(|s| ScheduleSummary {
                id: s.id.clone(),
                timing: format_timing(&s.timing),
                payload_type: match &s.payload {
                    SchedulePayload::Message { .. } => "message".to_string(),
                    SchedulePayload::Task { .. } => "task".to_string(),
                },
                created_at: s.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                process_handle: s.process_handle.clone(),
            })
            .collect();

        let content = serde_json::to_string_pretty(&summaries)
            .unwrap_or_else(|_| format!("{} active schedules", schedules.len()));

        Ok(ToolResult {
            success: true,
            content: format!("Active schedules:\n{}", content),
        })
    }

    async fn action_cancel(&self, schedule_id: &str) -> Result<ToolResult, ToolError> {
        match self
            .scheduler
            .cancel_schedule(schedule_id, &self.ctx.agent)
            .await
        {
            Ok(()) => Ok(ToolResult {
                success: true,
                content: format!("Cancelled schedule: {}", schedule_id),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                content: format!("Failed to cancel schedule: {}", e),
            }),
        }
    }
}

// ============================================================================
// Private Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct ScheduleArgs {
    action: String,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    every_seconds: Option<u64>,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    max_retries: Option<u8>,
    #[serde(default)]
    retry_delay_ms: Option<u64>,
    #[serde(default)]
    max_retry_delay_ms: Option<u64>,
    #[serde(default)]
    process_handle: Option<String>,
    #[serde(default)]
    schedule_id: Option<String>,
}

/// Format timing for display.
fn format_timing(timing: &ScheduleTiming) -> String {
    match timing {
        ScheduleTiming::At { at } => format!("at {}", at.format("%Y-%m-%d %H:%M UTC")),
        ScheduleTiming::Every { every_seconds, .. } => {
            if *every_seconds >= 3600 {
                format!("every {} hours", every_seconds / 3600)
            } else if *every_seconds >= 60 {
                format!("every {} minutes", every_seconds / 60)
            } else {
                format!("every {} seconds", every_seconds)
            }
        }
        ScheduleTiming::Cron { expr, .. } => format!("cron: {}", expr),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schedule_args_create() {
        let args: ScheduleArgs = serde_json::from_str(
            r#"{"action": "create", "at": "2026-01-30T16:00:00Z", "message": "hello"}"#,
        )
        .unwrap();
        assert_eq!(args.action, "create");
        assert!(args.at.is_some());
        assert!(args.message.is_some());
    }

    #[test]
    fn parse_schedule_args_cancel() {
        let args: ScheduleArgs =
            serde_json::from_str(r#"{"action": "cancel", "schedule_id": "sched_123"}"#).unwrap();
        assert_eq!(args.action, "cancel");
        assert_eq!(args.schedule_id.unwrap(), "sched_123");
    }

    #[test]
    fn parse_schedule_args_list() {
        let args: ScheduleArgs = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
        assert_eq!(args.action, "list");
    }

    #[test]
    fn parse_schedule_args_with_process_handle() {
        let args: ScheduleArgs = serde_json::from_str(
            r#"{"action": "create", "every_seconds": 30, "task": "check status", "process_handle": "01hqxyz123abc"}"#,
        )
        .unwrap();
        assert_eq!(args.action, "create");
        assert_eq!(args.process_handle, Some("01hqxyz123abc".to_string()));
    }

    #[test]
    fn format_timing_at() {
        let at = DateTime::parse_from_rfc3339("2026-01-30T16:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let timing = ScheduleTiming::At { at };
        assert!(format_timing(&timing).contains("2026-01-30"));
    }

    #[test]
    fn format_timing_every_hours() {
        let timing = ScheduleTiming::Every {
            every_seconds: 7200,
            anchor: None,
        };
        assert!(format_timing(&timing).contains("2 hours"));
    }

    #[test]
    fn format_timing_every_minutes() {
        let timing = ScheduleTiming::Every {
            every_seconds: 1800,
            anchor: None,
        };
        assert!(format_timing(&timing).contains("30 minutes"));
    }

    #[test]
    fn format_timing_cron() {
        let timing = ScheduleTiming::Cron {
            expr: "0 9 * * MON-FRI".to_string(),
            tz: None,
        };
        assert!(format_timing(&timing).contains("0 9 * * MON-FRI"));
    }
}
