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
//! Use `PendingApproval::into_messages()` to prepare messages, then call
//! `run_agentic_loop` again to continue after the user approves or denies.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use serde::{Deserialize, Serialize};

use crate::agent::{AgentSpec, ContextConfig, HooksConfig, ToolType};
use crate::context::{drop_oldest_iterations, mask_tool_results, truncate_tool_result};
use crate::llm::{ChatRequest, LLMError, LLMProvider, Message, Role, StreamEvent, ToolCall, Usage};
use crate::session::handle::SessionHandle;
use crate::tools::hooks::{GuardVerdict, HookContext, run_after_tool, run_before_tool};
use crate::tools::{ToolError, ToolExecutor, ToolResult, extract_action};

// ============================================================================
// Types
// ============================================================================

/// A message injected into a running agentic loop.
pub struct SteeringMessage {
    pub content: String,
    pub sender_id: Option<String>,
    pub sender_label: Option<String>,
    pub persisted: bool,
}

pub type SteeringSender = mpsc::UnboundedSender<SteeringMessage>;
pub type SteeringReceiver = mpsc::UnboundedReceiver<SteeringMessage>;

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

    #[error("llm call timed out after {0} seconds")]
    LlmTimeout(u64),
}

/// A pending approval waiting for user decision.
///
/// Approvals have no timeout — they wait indefinitely until the user
/// approves, denies, or sends a new message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    /// The tool call ID that needs approval.
    pub call_id: String,
    /// The tool name (e.g., "bash").
    pub tool_name: String,
    /// The tool call arguments.
    pub arguments: serde_json::Value,
    /// The command being approved (for display).
    pub command: String,
    /// The tool type (for saving "Allow Always" patterns).
    #[serde(default = "default_tool_type")]
    pub tool_type: ToolType,
    /// Accumulated messages to restore when resuming the loop.
    pub messages: Vec<Message>,
    /// Platform sender ID of the user who triggered this approval.
    /// Used in group chats to ensure only the requester can approve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_id: Option<String>,
}

/// Default tool type for backwards compatibility with persisted approvals.
fn default_tool_type() -> ToolType {
    ToolType::Bash
}

/// Context for resuming an agentic loop after a tool approval.
pub struct ResumeContext {
    /// The pending approval that was resolved.
    pub pending: PendingApproval,
    /// The result of executing (or denying) the tool.
    pub tool_result: ToolResult,
}

impl PendingApproval {
    /// Create a new pending approval.
    pub fn new(
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        command: String,
        tool_type: ToolType,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            call_id,
            tool_name,
            arguments,
            command,
            tool_type,
            messages,
            requester_id: None,
        }
    }

    /// Convert into initial messages for resuming the loop, appending the tool result.
    pub fn into_messages(self, tool_result_content: String) -> Vec<Message> {
        let mut messages = self.messages;
        messages.push(Message::tool_result(&self.call_id, tool_result_content));
        messages
    }
}

/// Outcome of executing a single tool call.
///
/// This represents the control flow decision after attempting to execute a tool:
/// - `Executed`: Tool ran (successfully or with error), continue the loop
/// - `AwaitingApproval`: Tool requires user approval, pause the loop
enum ToolCallOutcome {
    /// Tool executed, add this message to conversation and continue.
    Executed {
        tool_result_msg: Message,
        steering_msg: Option<Message>,
    },
    /// Tool requires approval, pause the loop with this pending state.
    AwaitingApproval(PendingApproval),
}

// ============================================================================
// Public API
// ============================================================================

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
    executor: &mut ToolExecutor,
    agent_spec: &AgentSpec,
    initial_messages: Vec<Message>,
    handle: &SessionHandle,
    tool_filter: Option<&HashSet<String>>,
    steering_rx: Option<SteeringReceiver>,
) -> Result<AgenticResult, AgenticError> {
    let max_iterations = agent_spec.session.max_tool_iterations;
    let llm_timeout = Duration::from_secs(agent_spec.session.llm_timeout_seconds);
    let context_config = &agent_spec.session.context;

    let mut steering_rx = steering_rx;
    let mut messages = initial_messages;
    let conversation_end_idx = messages.len();
    let mut total_usage: Option<Usage> = None;
    let mut iterations = 0u32;
    let mut tool_calls_made = 0u32;

    // Compute loop budget for iteration group dropping (Layer 3c)
    let max_input = agent_spec.model.effective_max_input_tokens();
    let output_reserve = agent_spec.model.max_output_tokens.unwrap_or(4096);
    let safety_margin = max_input / 10;
    let loop_token_budget = max_input.saturating_sub(output_reserve + safety_margin);

    loop {
        iterations += 1;

        if iterations > max_iterations {
            return Err(AgenticError::MaxIterationsExceeded(max_iterations));
        }

        // Drain steering messages (skip first iteration — we just started)
        if iterations > 1
            && let Some(ref mut rx) = steering_rx
        {
            let steered = drain_steering(rx);
            if !steered.is_empty() {
                inject_steered_messages(handle, &mut messages, steered).await;
            }
        }

        // Layer 3b: Mask old tool results (after first iteration)
        if iterations > 1 {
            mask_tool_results(
                &mut messages,
                conversation_end_idx,
                context_config.tool_result_keep_first,
                context_config.tool_result_keep_last,
            );
        }

        // Layer 3c: Drop oldest iteration groups if over budget
        drop_oldest_iterations(&mut messages, conversation_end_idx, loop_token_budget);

        // Refresh tool definitions each iteration (picks up reload_tools changes)
        let tool_definitions = executor.tool_definitions(tool_filter);

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
            tool_definitions,
        );

        // Call LLM with streaming (retry on rate limit) + consume stream,
        // all under a single timeout covering the full LLM round-trip.
        let llm_timeout_secs = agent_spec.session.llm_timeout_seconds;
        let (content, tool_calls, usage) = tokio::time::timeout(llm_timeout, async {
            let mut stream = {
                const MAX_RETRIES: u32 = 3;
                let mut attempt = 0;
                loop {
                    match provider.chat_stream(request.clone()).await {
                        Ok(s) => break s,
                        Err(LLMError::RateLimit { retry_after }) if attempt < MAX_RETRIES => {
                            attempt += 1;
                            let delay = retry_after.unwrap_or(2u64.pow(attempt));
                            warn!(attempt, delay_secs = delay, "Rate limited, retrying");
                            tokio::time::sleep(Duration::from_secs(delay)).await;
                        }
                        Err(e) => return Err(AgenticError::from(e)),
                    }
                }
            };

            let mut content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut usage: Option<Usage> = None;

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

            Ok((content, tool_calls, usage))
        })
        .await
        .map_err(|_| AgenticError::LlmTimeout(llm_timeout_secs))??;

        // Accumulate usage
        total_usage = accumulate_usage(total_usage, usage);

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
        let assistant_msg = build_assistant_message(&content, &tool_calls);
        messages.push(assistant_msg);

        // Execute tools sequentially to catch approval requirements
        // (We process one at a time so we can pause at the first approval request)
        let mut reload_requested = false;
        for (i, tool_call) in tool_calls.iter().enumerate() {
            tool_calls_made += 1;

            if tool_call.function.name == "reload_tools" {
                reload_requested = true;
            }

            let outcome = execute_tool_call(
                executor,
                handle,
                tool_call,
                &messages,
                &content,
                context_config,
                &agent_spec.hooks,
            )
            .await;

            match outcome {
                ToolCallOutcome::Executed {
                    tool_result_msg,
                    steering_msg,
                } => {
                    messages.push(tool_result_msg);
                    if let Some(msg) = steering_msg {
                        messages.push(msg);
                    }
                }
                ToolCallOutcome::AwaitingApproval(pending) => {
                    return Ok(AgenticResult::AwaitingApproval {
                        pending,
                        partial_content: content,
                        usage: total_usage,
                        iterations,
                        tool_calls_made,
                    });
                }
            }

            // Check for steering between tool calls
            if let Some(ref mut rx) = steering_rx {
                let steered = drain_steering(rx);
                if !steered.is_empty() {
                    // Skip remaining tool calls with synthetic results
                    for remaining in &tool_calls[i + 1..] {
                        let arguments = parse_tool_arguments(
                            &remaining.function.name,
                            &remaining.function.arguments,
                        );
                        if let Err(e) = handle
                            .enqueue_tool_call(
                                remaining.id.clone(),
                                remaining.function.name.clone(),
                                arguments,
                            )
                            .await
                        {
                            warn!(error = %e, "Failed to enqueue tool call event");
                        }
                        if let Err(e) = handle
                            .enqueue_tool_result(remaining.id.clone(), false, "Skipped".into())
                            .await
                        {
                            warn!(error = %e, "Failed to enqueue tool result event");
                        }
                        messages.push(Message::tool_result(
                            &remaining.id,
                            "Skipped: new user message received",
                        ));
                    }
                    // Inject steered messages
                    inject_steered_messages(handle, &mut messages, steered).await;
                    break;
                }
            }
        }

        // Rebuild executor if reload_tools was called
        if reload_requested {
            executor.rebuild();
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
    executor: &mut ToolExecutor,
    agent_spec: &AgentSpec,
    resume: ResumeContext,
    handle: &SessionHandle,
    tool_filter: Option<&HashSet<String>>,
    steering_rx: Option<SteeringReceiver>,
) -> Result<AgenticResult, AgenticError> {
    // Record tool result event
    if let Err(e) = handle
        .enqueue_tool_result(
            resume.pending.call_id.clone(),
            resume.tool_result.success,
            resume.tool_result.content.clone(),
        )
        .await
    {
        warn!(error = %e, "Failed to enqueue tool result event");
    }

    // Build messages and continue the loop
    let messages = resume.pending.into_messages(resume.tool_result.content);
    run_agentic_loop(
        provider,
        executor,
        agent_spec,
        messages,
        handle,
        tool_filter,
        steering_rx,
    )
    .await
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Execute a single tool call and return the outcome.
///
/// This handles:
/// - Running before-tool hooks (guards)
/// - Recording the tool call event
/// - Executing the tool
/// - Handling approval requirements (pausing the loop)
/// - Handling errors (converting to tool result messages)
/// - Recording the tool result event
/// - Running after-tool hooks (steering)
async fn execute_tool_call(
    executor: &ToolExecutor,
    handle: &SessionHandle,
    tool_call: &ToolCall,
    messages: &[Message],
    content: &str,
    context_config: &ContextConfig,
    hooks: &HooksConfig,
) -> ToolCallOutcome {
    // Parse arguments (empty string is valid — means no arguments)
    let raw_args = &tool_call.function.arguments;
    let arguments = parse_tool_arguments(&tool_call.function.name, raw_args);

    // Extract action for hook context
    let action_str = extract_action(raw_args);
    let hook_ctx = HookContext {
        tool_name: &tool_call.function.name,
        action: action_str.as_deref(),
        arguments: &arguments,
        messages,
    };

    // Run before-tool hooks (guards)
    if let GuardVerdict::Reject(reason) = run_before_tool(hooks, &hook_ctx) {
        let result = ToolResult {
            success: false,
            content: reason,
        };

        // Record tool call + rejected result events
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
        if let Err(e) = handle
            .enqueue_tool_result(tool_call.id.clone(), false, result.content.clone())
            .await
        {
            warn!(error = %e, "Failed to enqueue tool result event");
        }

        return ToolCallOutcome::Executed {
            tool_result_msg: Message::tool_result(&tool_call.id, result.content),
            steering_msg: None,
        };
    }

    // Record tool call event
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

    // Handle the execution result
    let result = match exec_result {
        Ok(r) => r,
        Err(ToolError::ApprovalRequired {
            call_id,
            command,
            tool_type,
        }) => {
            let pending = PendingApproval::new(
                call_id,
                tool_call.function.name.clone(),
                arguments.clone(),
                command,
                tool_type,
                messages.to_vec(),
            );
            return handle_approval_required(handle, pending, content).await;
        }
        Err(ToolError::PolicyDenied(command)) => ToolResult {
            success: false,
            content: format!(
                "Command '{}' was denied by policy. This command is not allowed.",
                command
            ),
        },
        Err(e) => ToolResult {
            success: false,
            content: format!("Tool execution failed: {}", e),
        },
    };

    // Layer 3a: Truncate tool result if over budget
    let truncated_content = truncate_tool_result(
        &result.content,
        context_config.max_tool_result_tokens,
        context_config.tool_result_truncation,
    );

    // Record tool result event (with truncated content)
    if let Err(e) = handle
        .enqueue_tool_result(
            tool_call.id.clone(),
            result.success,
            truncated_content.clone(),
        )
        .await
    {
        warn!(error = %e, "Failed to enqueue tool result event");
    }

    // Run after-tool hooks (steering)
    let steering_msg = run_after_tool(hooks, &hook_ctx, &result)
        .map(|msg| Message::steering(format!("[steering] {}", msg)));

    // Return the tool result message (with truncated content)
    let tool_result_msg = Message::tool_result(&tool_call.id, truncated_content);
    ToolCallOutcome::Executed {
        tool_result_msg,
        steering_msg,
    }
}

/// Handle a tool that requires approval.
///
/// Records events and returns the pending approval state.
async fn handle_approval_required(
    handle: &SessionHandle,
    pending: PendingApproval,
    content: &str,
) -> ToolCallOutcome {
    // Record partial assistant content if any (before tool call)
    if !content.is_empty()
        && let Err(e) = handle
            .enqueue_assistant_message(content.to_string(), None)
            .await
    {
        warn!(error = %e, "Failed to enqueue partial assistant message");
    }

    // Record approval required event
    if let Err(e) = handle
        .enqueue_approval_required(pending.call_id.clone(), pending.command.clone())
        .await
    {
        warn!(error = %e, "Failed to enqueue approval required event");
    }

    ToolCallOutcome::AwaitingApproval(pending)
}

/// Build the assistant message for a response with tool calls.
fn build_assistant_message(content: &str, tool_calls: &[ToolCall]) -> Message {
    if content.is_empty() {
        Message::assistant_tool_calls(tool_calls.to_vec())
    } else {
        Message {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: Some(tool_calls.to_vec()),
            tool_call_id: None,
        }
    }
}

/// Inject steered messages into the conversation, persisting when needed.
async fn inject_steered_messages(
    handle: &SessionHandle,
    messages: &mut Vec<Message>,
    steered: Vec<SteeringMessage>,
) {
    for msg in steered {
        let content = msg.content;
        if !msg.persisted {
            handle
                .add_user_message_with_sender(
                    content.clone(),
                    msg.sender_id.clone(),
                    msg.sender_label.clone(),
                )
                .await
                .ok();
        }
        messages.push(Message::text(Role::User, &content));
    }
}

/// Drain all pending steering messages from the channel.
fn drain_steering(rx: &mut SteeringReceiver) -> Vec<SteeringMessage> {
    let mut msgs = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        msgs.push(msg);
    }
    msgs
}

/// Parse tool arguments JSON, handling empty or malformed inputs.
fn parse_tool_arguments(tool_name: &str, raw_args: &str) -> serde_json::Value {
    if raw_args.trim().is_empty() {
        return serde_json::Value::Object(Default::default());
    }

    match serde_json::from_str(raw_args) {
        Ok(v) => v,
        Err(e) => {
            let truncated: String = raw_args.chars().take(200).collect();
            warn!(
                tool = %tool_name,
                error = %e,
                raw = %truncated,
                "Malformed tool call arguments, using empty object"
            );
            serde_json::Value::default()
        }
    }
}

/// Accumulate token usage across iterations.
fn accumulate_usage(existing: Option<Usage>, new: Option<Usage>) -> Option<Usage> {
    match (existing, new) {
        (Some(e), Some(n)) => Some(Usage {
            prompt_tokens: e.prompt_tokens + n.prompt_tokens,
            completion_tokens: e.completion_tokens + n.completion_tokens,
            total_tokens: e.total_tokens + n.total_tokens,
        }),
        (Some(e), None) => Some(e),
        (None, Some(n)) => Some(n),
        (None, None) => None,
    }
}

// ============================================================================
// Tests
// ============================================================================

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
            ToolType::Bash,
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

    #[test]
    fn drain_steering_collects_pending_messages() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(SteeringMessage {
            content: "hello".into(),
            sender_id: None,
            sender_label: None,
            persisted: false,
        })
        .unwrap();
        tx.send(SteeringMessage {
            content: "world".into(),
            sender_id: None,
            sender_label: None,
            persisted: false,
        })
        .unwrap();

        let msgs = drain_steering(&mut rx);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].content, "world");

        // Channel empty now
        let empty = drain_steering(&mut rx);
        assert!(empty.is_empty());
    }

    #[test]
    fn accumulate_usage_both_some() {
        let a = Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        });
        let b = Some(Usage {
            prompt_tokens: 20,
            completion_tokens: 10,
            total_tokens: 30,
        });
        let result = accumulate_usage(a, b).unwrap();
        assert_eq!(result.prompt_tokens, 30);
        assert_eq!(result.completion_tokens, 15);
        assert_eq!(result.total_tokens, 45);
    }

    #[test]
    fn accumulate_usage_one_none() {
        let a = Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        });
        assert!(accumulate_usage(a.clone(), None).is_some());
        assert!(accumulate_usage(None, a).is_some());
        assert!(accumulate_usage(None, None).is_none());
    }

    #[test]
    fn build_assistant_message_with_content() {
        let tool_calls = vec![ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        let msg = build_assistant_message("Some content", &tool_calls);
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, Some("Some content".to_string()));
        assert!(msg.tool_calls.is_some());
    }

    #[test]
    fn build_assistant_message_empty_content() {
        let tool_calls = vec![ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        let msg = build_assistant_message("", &tool_calls);
        assert_eq!(msg.role, Role::Assistant);
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_some());
    }
}
