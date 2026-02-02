//! Agentic loop for tool-using agents.
//!
//! This module implements the core agentic loop:
//! 1. Build ChatRequest with tools
//! 2. Call LLM
//! 3. If response has tool_calls: execute tools, add results, continue
//! 4. If no tool_calls: return final message
//! 5. Check iteration limit
//!
//! The loop can pause when a tool requires approval, returning `AwaitingApproval`.
//! Use `resume_agentic_loop` to continue after the user approves or denies.

use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use tracing::{debug, warn};

use crate::agent::AgentSpec;
use crate::llm::{ChatRequest, LLMProvider, Message, Role, StreamEvent, ToolCall, Usage};
use crate::session::handle::SessionHandle;
use crate::session::snapshot::PendingApproval;
use crate::tools::{ToolError, ToolExecutor, ToolResult};

/// Result of running the agentic loop.
#[derive(Debug)]
pub enum AgenticResult {
    /// The loop completed with a final response.
    Complete {
        /// Final assistant response content.
        content: String,
        /// Total token usage across all iterations.
        usage: Option<Usage>,
        /// Number of iterations executed.
        iterations: u32,
        /// Tool calls made during the loop.
        tool_calls_made: u32,
    },
    /// The loop is paused waiting for approval.
    AwaitingApproval {
        /// The pending approval details.
        pending: PendingApproval,
        /// Partial content accumulated so far.
        partial_content: String,
        /// Total token usage so far.
        usage: Option<Usage>,
        /// Number of iterations executed so far.
        iterations: u32,
        /// Tool calls made so far.
        tool_calls_made: u32,
    },
}

/// Error from the agentic loop.
#[derive(Debug, thiserror::Error)]
pub enum AgenticError {
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LLMError),

    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("max iterations ({0}) exceeded")]
    MaxIterationsExceeded(u32),
}

/// Run the agentic loop with tool execution.
///
/// This function implements the core loop for tool-using agents:
/// - Calls the LLM with available tools
/// - If the LLM returns tool calls, executes them and feeds results back
/// - Continues until the LLM returns a final response or max iterations is reached
///
/// If `tool_filter` is provided, only those tools will be visible to the LLM.
pub async fn run_agentic_loop(
    provider: Arc<dyn LLMProvider>,
    executor: &ToolExecutor,
    agent_spec: &AgentSpec,
    initial_messages: Vec<Message>,
    handle: &SessionHandle,
    tool_filter: Option<&HashSet<String>>,
) -> Result<AgenticResult, AgenticError> {
    let max_iterations = agent_spec.session.max_tool_iterations;
    let tool_definitions = executor.tool_definitions(tool_filter);

    let mut messages = initial_messages;
    let mut total_usage: Option<Usage> = None;
    let mut iterations = 0u32;
    let mut tool_calls_made = 0u32;

    loop {
        iterations += 1;

        if iterations > max_iterations {
            return Err(AgenticError::MaxIterationsExceeded(max_iterations));
        }

        debug!(
            iteration = iterations,
            max_iterations,
            messages_count = messages.len(),
            "Agentic loop iteration"
        );

        // Build request with tools
        let request = ChatRequest::with_tools(
            &agent_spec.model.name,
            messages.clone(),
            agent_spec.model.temperature,
            agent_spec.model.max_output_tokens,
            tool_definitions.clone(),
        );

        // Call LLM with streaming
        let mut stream = provider.chat_stream(request).await?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage: Option<Usage> = None;

        // Consume stream
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Token(token) => {
                    content.push_str(&token);
                }
                StreamEvent::ToolCalls(calls) => {
                    tool_calls = calls;
                }
                StreamEvent::Done { usage: u } => {
                    usage = u;
                }
                StreamEvent::Cancelled => {
                    break;
                }
            }
        }

        // Accumulate usage
        if let Some(u) = usage {
            total_usage = Some(match total_usage {
                Some(existing) => Usage {
                    prompt_tokens: existing.prompt_tokens + u.prompt_tokens,
                    completion_tokens: existing.completion_tokens + u.completion_tokens,
                    total_tokens: existing.total_tokens + u.total_tokens,
                },
                None => u,
            });
        }

        // If no tool calls, we're done
        if tool_calls.is_empty() {
            return Ok(AgenticResult::Complete {
                content,
                usage: total_usage,
                iterations,
                tool_calls_made,
            });
        }

        // Process tool calls
        debug!(tool_calls_count = tool_calls.len(), "Processing tool calls");

        // Add assistant message with tool calls
        let assistant_msg = if content.is_empty() {
            Message::assistant_tool_calls(tool_calls.clone())
        } else {
            Message {
                role: Role::Assistant,
                content: Some(content.clone()),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            }
        };
        messages.push(assistant_msg);

        // Execute tools sequentially to catch approval requirements
        // (We process one at a time so we can pause at the first approval request)
        for tool_call in &tool_calls {
            tool_calls_made += 1;

            // Record tool call event
            let arguments: serde_json::Value =
                serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();
            if let Err(e) = handle
                .enqueue_tool_call(
                    tool_call.id.clone(),
                    tool_call.function.name.clone(),
                    arguments.clone(),
                )
                .await
            {
                warn!(error = %e, "Failed to enqueue tool call event");
            }

            // Execute the tool
            let exec_result = executor.execute(tool_call).await;

            // Convert Result to ToolResult (handle errors gracefully)
            let result = match exec_result {
                Ok(r) => r,
                Err(ToolError::ApprovalRequired { call_id, command }) => {
                    // Record partial assistant content if any (before tool call)
                    if !content.is_empty()
                        && let Err(e) = handle
                            .enqueue_assistant_message(content.clone(), None)
                            .await
                    {
                        warn!(error = %e, "Failed to enqueue partial assistant message");
                    }

                    // Record approval required event
                    if let Err(e) = handle
                        .enqueue_approval_required(call_id.clone(), command.clone())
                        .await
                    {
                        warn!(error = %e, "Failed to enqueue approval required event");
                    }

                    // Create pending approval and return AwaitingApproval
                    let pending = PendingApproval::new(
                        call_id,
                        tool_call.function.name.clone(),
                        arguments,
                        command,
                        messages.clone(),
                    );

                    return Ok(AgenticResult::AwaitingApproval {
                        pending,
                        partial_content: content,
                        usage: total_usage,
                        iterations,
                        tool_calls_made,
                    });
                }
                Err(ToolError::PolicyDenied(command)) => {
                    // Policy denial - inform LLM
                    ToolResult {
                        success: false,
                        content: format!(
                            "Command '{}' was denied by policy. This command is not allowed.",
                            command
                        ),
                    }
                }
                Err(e) => {
                    // Other errors - feed back to LLM
                    ToolResult {
                        success: false,
                        content: format!("Tool execution failed: {}", e),
                    }
                }
            };

            // Record tool result event
            if let Err(e) = handle
                .enqueue_tool_result(tool_call.id.clone(), result.success, result.content.clone())
                .await
            {
                warn!(error = %e, "Failed to enqueue tool result event");
            }

            // Add tool result message
            let tool_result_msg = Message::tool_result(&tool_call.id, result.content);
            messages.push(tool_result_msg);
        }

        // Continue loop with updated messages
    }
}

/// Resume the agentic loop after an approval decision.
///
/// This continues the loop from where it paused, injecting the tool result
/// (either the actual execution result or a denial message) and continuing
/// until completion or another approval is needed.
pub async fn resume_agentic_loop(
    provider: Arc<dyn LLMProvider>,
    executor: &ToolExecutor,
    agent_spec: &AgentSpec,
    pending: PendingApproval,
    tool_result: ToolResult,
    handle: &SessionHandle,
    tool_filter: Option<&HashSet<String>>,
) -> Result<AgenticResult, AgenticError> {
    // Restore messages from pending state
    let mut messages = pending.messages;

    // Record tool result event
    if let Err(e) = handle
        .enqueue_tool_result(
            pending.call_id.clone(),
            tool_result.success,
            tool_result.content.clone(),
        )
        .await
    {
        warn!(error = %e, "Failed to enqueue tool result event");
    }

    // Add tool result message
    let tool_result_msg = Message::tool_result(&pending.call_id, tool_result.content);
    messages.push(tool_result_msg);

    // Continue the loop with the updated messages
    run_agentic_loop(
        provider,
        executor,
        agent_spec,
        messages,
        handle,
        tool_filter,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentic_result_complete_debug() {
        let result = AgenticResult::Complete {
            content: "Hello".to_string(),
            usage: None,
            iterations: 1,
            tool_calls_made: 0,
        };
        assert!(format!("{:?}", result).contains("Hello"));
        assert!(format!("{:?}", result).contains("Complete"));
    }

    #[test]
    fn agentic_result_awaiting_approval_debug() {
        let pending = PendingApproval::new(
            "call_123".to_string(),
            "bash".to_string(),
            serde_json::json!({"command": "ls"}),
            "ls".to_string(),
            vec![],
        );
        let result = AgenticResult::AwaitingApproval {
            pending,
            partial_content: "".to_string(),
            usage: None,
            iterations: 1,
            tool_calls_made: 1,
        };
        assert!(format!("{:?}", result).contains("AwaitingApproval"));
        assert!(format!("{:?}", result).contains("call_123"));
    }
}
