//! Schedule management tools for agents.
//!
//! Provides built-in tools for creating, listing, and cancelling schedules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::scheduler::{
    RetryConfig, Schedule, ScheduleDestination, SchedulePayload, ScheduleStatus, ScheduleTiming,
    SchedulerHandle,
};

use super::error::ToolError;
use super::executor::ToolResult;
use super::tool::Tool;

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
// Tool Structs
// ============================================================================

/// The schedule_task tool for creating scheduled tasks.
pub struct ScheduleTaskTool {
    scheduler: SchedulerHandle,
    ctx: ToolExecutionContext,
}

impl ScheduleTaskTool {
    /// Create a new schedule_task tool.
    pub fn new(scheduler: SchedulerHandle, ctx: ToolExecutionContext) -> Self {
        Self { scheduler, ctx }
    }
}

#[async_trait]
impl Tool for ScheduleTaskTool {
    fn name(&self) -> &str {
        "schedule_task"
    }

    fn definition(&self) -> ToolDefinition {
        schedule_task_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute_schedule_task(&self.scheduler, &self.ctx, arguments).await
    }
}

/// The list_schedules tool for listing active schedules.
pub struct ListSchedulesTool {
    scheduler: SchedulerHandle,
    ctx: ToolExecutionContext,
}

impl ListSchedulesTool {
    /// Create a new list_schedules tool.
    pub fn new(scheduler: SchedulerHandle, ctx: ToolExecutionContext) -> Self {
        Self { scheduler, ctx }
    }
}

#[async_trait]
impl Tool for ListSchedulesTool {
    fn name(&self) -> &str {
        "list_schedules"
    }

    fn definition(&self) -> ToolDefinition {
        list_schedules_definition()
    }

    async fn execute(&self, _arguments: &str) -> Result<ToolResult, ToolError> {
        execute_list_schedules(&self.scheduler, &self.ctx).await
    }
}

/// The cancel_schedule tool for cancelling schedules.
pub struct CancelScheduleTool {
    scheduler: SchedulerHandle,
    ctx: ToolExecutionContext,
}

impl CancelScheduleTool {
    /// Create a new cancel_schedule tool.
    pub fn new(scheduler: SchedulerHandle, ctx: ToolExecutionContext) -> Self {
        Self { scheduler, ctx }
    }
}

#[async_trait]
impl Tool for CancelScheduleTool {
    fn name(&self) -> &str {
        "cancel_schedule"
    }

    fn definition(&self) -> ToolDefinition {
        cancel_schedule_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute_cancel_schedule(&self.scheduler, &self.ctx, arguments).await
    }
}

// ============================================================================
// schedule_task Tool
// ============================================================================

/// Tool definition for schedule_task.
pub fn schedule_task_definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "schedule_task".to_string(),
            description: "Schedule a one-shot reminder or recurring task. For reminders and one-time actions, use 'at'. Only use 'cron' or 'every_seconds' when the user explicitly asks for something recurring.".to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "at": {
                        "type": "string",
                        "description": "One-shot (preferred for reminders): ISO8601 timestamp for when to fire (e.g., '2026-01-30T16:00:00Z'). Fires once then auto-completes."
                    },
                    "every_seconds": {
                        "type": "integer",
                        "description": "Recurring: Interval in seconds. Only use when user explicitly wants repetition."
                    },
                    "cron": {
                        "type": "string",
                        "description": "Recurring: Cron expression (e.g., '0 9 * * MON-FRI'). Only use when user explicitly wants a recurring schedule."
                    },
                    "message": {
                        "type": "string",
                        "description": "Simple message to deliver (no LLM call, just sends the text)"
                    },
                    "task": {
                        "type": "string",
                        "description": "Task to execute with LLM and tools, then report results"
                    },
                    "max_retries": {
                        "type": "integer",
                        "description": "Number of retry attempts on failure (default: 3). Set to enable retry with exponential backoff."
                    },
                    "retry_delay_ms": {
                        "type": "integer",
                        "description": "Initial delay before first retry in milliseconds (default: 1000)"
                    },
                    "max_retry_delay_ms": {
                        "type": "integer",
                        "description": "Maximum delay between retries in milliseconds (default: 30000)"
                    }
                },
                "required": []
            })),
        },
    }
}

/// Execute the schedule_task tool.
pub async fn execute_schedule_task(
    scheduler: &SchedulerHandle,
    ctx: &ToolExecutionContext,
    arguments: &str,
) -> Result<ToolResult, ToolError> {
    // Require gateway context
    let gateway = ctx
        .gateway
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed("No gateway context".to_string()))?;
    let chat_id = ctx
        .chat_id
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed("No chat_id context".to_string()))?;

    // Parse arguments
    let args: ScheduleTaskArgs =
        serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

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
    let timing = if let Some(at_str) = args.at {
        let at: DateTime<Utc> = at_str.parse().map_err(|e| {
            ToolError::InvalidArguments(format!("Invalid timestamp '{}': {}", at_str, e))
        })?;
        ScheduleTiming::At { at }
    } else if let Some(every) = args.every_seconds {
        ScheduleTiming::Every {
            every_seconds: every,
            anchor: Some(Utc::now()),
        }
    } else if let Some(cron) = args.cron {
        ScheduleTiming::Cron {
            expr: cron,
            tz: None,
        }
    } else {
        return Ok(ToolResult {
            success: false,
            content: "Error: no timing option resolved".to_string(),
        });
    };

    // Build payload
    let payload = if let Some(message) = args.message {
        SchedulePayload::Message { message }
    } else if let Some(task) = args.task {
        SchedulePayload::Task { task }
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
        agent: ctx.agent.clone(),
        created_by_session: ctx.session_id.clone(),
        destination: ScheduleDestination {
            gateway: gateway.clone(),
            chat_id: chat_id.clone(),
        },
        timing,
        payload,
        created_at: Utc::now(),
        status: ScheduleStatus::Active,
        retry,
    };

    let id = schedule.id.clone();
    let is_recurring = schedule.is_recurring();

    match scheduler.create_schedule(schedule).await {
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

/// Arguments for the schedule_task tool.
#[derive(Debug, Deserialize)]
struct ScheduleTaskArgs {
    /// One-shot timestamp (ISO8601).
    #[serde(default)]
    at: Option<String>,
    /// Recurring interval in seconds.
    #[serde(default)]
    every_seconds: Option<u64>,
    /// Cron expression.
    #[serde(default)]
    cron: Option<String>,
    /// Message to send (no LLM call).
    #[serde(default)]
    message: Option<String>,
    /// Task to execute (with LLM and tools).
    #[serde(default)]
    task: Option<String>,
    /// Maximum number of retry attempts on failure.
    #[serde(default)]
    max_retries: Option<u8>,
    /// Initial delay before first retry in milliseconds.
    #[serde(default)]
    retry_delay_ms: Option<u64>,
    /// Maximum delay between retries in milliseconds.
    #[serde(default)]
    max_retry_delay_ms: Option<u64>,
}

// ============================================================================
// list_schedules Tool
// ============================================================================

/// Tool definition for list_schedules.
pub fn list_schedules_definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "list_schedules".to_string(),
            description: "List all active schedules created by this agent.".to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        },
    }
}

/// Execute the list_schedules tool.
pub async fn execute_list_schedules(
    scheduler: &SchedulerHandle,
    ctx: &ToolExecutionContext,
) -> Result<ToolResult, ToolError> {
    let schedules = scheduler.list_schedules(&ctx.agent).await;

    if schedules.is_empty() {
        return Ok(ToolResult {
            success: true,
            content: "No active schedules.".to_string(),
        });
    }

    // Format schedules for display
    #[derive(Serialize)]
    struct ScheduleSummary {
        id: String,
        timing: String,
        payload_type: String,
        created_at: String,
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
        })
        .collect();

    let content = serde_json::to_string_pretty(&summaries)
        .unwrap_or_else(|_| format!("{} active schedules", schedules.len()));

    Ok(ToolResult {
        success: true,
        content: format!("Active schedules:\n{}", content),
    })
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
// cancel_schedule Tool
// ============================================================================

/// Tool definition for cancel_schedule.
pub fn cancel_schedule_definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "cancel_schedule".to_string(),
            description: "Cancel an active schedule.".to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "schedule_id": {
                        "type": "string",
                        "description": "The ID of the schedule to cancel"
                    }
                },
                "required": ["schedule_id"]
            })),
        },
    }
}

/// Execute the cancel_schedule tool.
pub async fn execute_cancel_schedule(
    scheduler: &SchedulerHandle,
    ctx: &ToolExecutionContext,
    arguments: &str,
) -> Result<ToolResult, ToolError> {
    let args: CancelScheduleArgs =
        serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

    match scheduler
        .cancel_schedule(&args.schedule_id, &ctx.agent)
        .await
    {
        Ok(()) => Ok(ToolResult {
            success: true,
            content: format!("Cancelled schedule: {}", args.schedule_id),
        }),
        Err(e) => Ok(ToolResult {
            success: false,
            content: format!("Failed to cancel schedule: {}", e),
        }),
    }
}

/// Arguments for the cancel_schedule tool.
#[derive(Debug, Deserialize)]
struct CancelScheduleArgs {
    schedule_id: String,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_task_definition_has_timing_options() {
        let def = schedule_task_definition();
        assert_eq!(def.function.name, "schedule_task");

        let params = def.function.parameters.unwrap();
        let props = params["properties"].as_object().unwrap();
        assert!(props.contains_key("at"));
        assert!(props.contains_key("every_seconds"));
        assert!(props.contains_key("cron"));
        assert!(props.contains_key("message"));
        assert!(props.contains_key("task"));
        assert!(props.contains_key("max_retries"));
        assert!(props.contains_key("retry_delay_ms"));
        assert!(props.contains_key("max_retry_delay_ms"));
    }

    #[test]
    fn list_schedules_definition_is_valid() {
        let def = list_schedules_definition();
        assert_eq!(def.function.name, "list_schedules");
    }

    #[test]
    fn cancel_schedule_definition_requires_id() {
        let def = cancel_schedule_definition();
        assert_eq!(def.function.name, "cancel_schedule");

        let params = def.function.parameters.unwrap();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("schedule_id")));
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
