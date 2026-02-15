//! Approval flow processing for tool execution callbacks.

use tracing::{debug, error, warn};

use duragent_gateway_protocol::CallbackQueryData;

use super::build_approval_keyboard;
use super::handler::GatewayMessageHandler;
use crate::agent::ToolPolicy;
use crate::api::SessionStatus;
use crate::context::ContextBuilder;
use crate::llm::{FunctionCall, ToolCall};
use crate::session::{AgenticResult, ApprovalDecisionType, ResumeContext, resume_agentic_loop};
use crate::tools::{
    ReloadDeps, ToolDependencies, ToolExecutionContext, ToolResult, build_executor,
};

// ============================================================================
// Approval Processing
// ============================================================================

impl GatewayMessageHandler {
    /// Process a callback query for tool approval decisions.
    ///
    /// Handles the full approval flow: parse decision, verify requester,
    /// execute or deny tool, and resume the agentic loop.
    pub(super) async fn process_callback_query(
        &self,
        gateway: &str,
        data: &CallbackQueryData,
    ) -> Option<String> {
        debug!(
            gateway = %gateway,
            chat_id = %data.chat_id,
            callback_data = %data.data,
            "Processing callback query for approval"
        );

        // Parse callback data format: "approve:{decision}"
        let parts: Vec<&str> = data.data.split(':').collect();
        if parts.len() < 2 || parts[0] != "approve" {
            debug!(callback_data = %data.data, "Ignoring non-approval callback");
            return None;
        }

        let decision = parts[1];

        // Look up session from chat_id - returns SessionHandle
        let handle = match self.find_session_for_chat(gateway, &data.chat_id).await {
            Some(h) => h,
            None => {
                warn!(chat_id = %data.chat_id, "No session found for chat");
                return Some("No active session".to_string());
            }
        };
        let lock = self.message_locks.get(handle.id());
        let _guard = lock.lock().await;

        // Load pending approval via actor
        let pending = match handle.get_pending_approval().await {
            Ok(Some(p)) => p,
            Ok(None) => {
                warn!(session_id = %handle.id(), "No pending approval found");
                return Some("No pending approval".to_string());
            }
            Err(e) => {
                error!(error = %e, "Failed to load pending approval");
                return Some("Failed to load approval".to_string());
            }
        };

        // Verify the approver matches the original requester (group chat safety)
        if let Some(ref requester_id) = pending.requester_id
            && data.sender.id != *requester_id
        {
            debug!(
                sender = %data.sender.id,
                requester = %requester_id,
                "Approval rejected: sender is not the original requester"
            );
            return Some("Only the original requester can approve this command".to_string());
        }

        // Map decision string to event type
        let decision_type = match decision {
            "allow_once" => ApprovalDecisionType::AllowOnce,
            "allow_always" => ApprovalDecisionType::AllowAlways,
            "deny" => ApprovalDecisionType::Deny,
            _ => {
                warn!(decision = %decision, "Unknown approval decision");
                return Some("Invalid decision".to_string());
            }
        };

        // Build toast message for immediate feedback
        let command_preview = truncate_command(&pending.command, 40);
        let toast = match decision_type {
            ApprovalDecisionType::AllowOnce => format!("✓ Running: {}", command_preview),
            ApprovalDecisionType::AllowAlways => format!("✓ Always allowed: {}", command_preview),
            ApprovalDecisionType::Deny => format!("✗ Denied: {}", command_preview),
        };

        // Record the approval decision event via actor
        if let Err(e) = handle
            .record_approval_decision(pending.call_id.clone(), decision_type)
            .await
        {
            error!(error = %e, "Failed to record approval decision");
            return Some("Failed to process approval".to_string());
        }

        // Get agent spec for tool execution
        let agent = match self.services.agents.get(handle.agent()) {
            Some(a) => a,
            None => {
                error!(agent = %handle.agent(), "Agent not found");
                return Some("Agent configuration error".to_string());
            }
        };

        // If allow_always, save pattern to policy
        if decision_type == ApprovalDecisionType::AllowAlways
            && let Err(e) = ToolPolicy::add_pattern_and_save(
                self.services.policy_store.as_ref(),
                handle.agent(),
                pending.tool_type,
                &pending.command,
                &self.policy_locks,
            )
            .await
        {
            debug!(
                error = %e,
                command = %pending.command,
                "Failed to save allow pattern to policy"
            );
        }

        // Load policy from store (picks up runtime changes from AllowAlways)
        // This is loaded AFTER add_pattern_and_save so it includes any newly saved pattern
        let policy = self.services.policy_store.load(handle.agent()).await;

        // Determine tool result based on decision
        let tool_result = if decision_type == ApprovalDecisionType::Deny {
            ToolResult {
                success: false,
                content: format!(
                    "Command '{}' was denied by the user. Please try a different approach.",
                    pending.command
                ),
            }
        } else {
            // Execute the tool
            let execution_context = self.scheduler.as_ref().map(|_| ToolExecutionContext {
                gateway: Some(gateway.to_string()),
                chat_id: Some(data.chat_id.to_string()),
                agent: handle.agent().to_string(),
                session_id: handle.id().to_string(),
            });
            let deps = ToolDependencies {
                sandbox: self.services.sandbox.clone(),
                agent_dir: agent.agent_dir.clone(),
                scheduler: self.scheduler.clone(),
                execution_context,
                workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
                process_registry: self.process_registry.clone(),
                session_id: Some(handle.id().to_string()),
                agent_name: Some(handle.agent().to_string()),
                session_registry: Some(self.services.session_registry.clone()),
            };
            let executor = build_executor(
                &agent,
                handle.agent(),
                handle.id(),
                policy.clone(),
                deps,
                &self.services.world_memory_path,
            );

            // Build tool call from pending approval
            let tool_call = ToolCall {
                id: pending.call_id.clone(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: pending.tool_name.clone(),
                    arguments: pending.arguments.to_string(),
                },
            };

            match executor.execute_bypassing_policy(&tool_call).await {
                Ok(result) => result,
                Err(e) => ToolResult {
                    success: false,
                    content: format!("Tool execution failed: {}", e),
                },
            }
        };

        // Clear pending approval via actor
        if let Err(e) = handle.clear_pending_approval().await {
            debug!(error = %e, "Failed to clear pending approval");
        }

        // Get provider for resuming the loop
        let provider = match self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())
            .await
        {
            Some(p) => p,
            None => {
                error!(provider = %agent.model.provider, "Provider not configured");
                return Some("Provider configuration error".to_string());
            }
        };

        // Create executor for resume (uses same policy loaded above)
        let execution_context = self.scheduler.as_ref().map(|_| ToolExecutionContext {
            gateway: Some(gateway.to_string()),
            chat_id: Some(data.chat_id.to_string()),
            agent: handle.agent().to_string(),
            session_id: handle.id().to_string(),
        });
        let deps = ToolDependencies {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            scheduler: self.scheduler.clone(),
            execution_context,
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
            process_registry: self.process_registry.clone(),
            session_id: Some(handle.id().to_string()),
            agent_name: Some(handle.agent().to_string()),
            session_registry: Some(self.services.session_registry.clone()),
        };
        let mut executor = build_executor(
            &agent,
            handle.agent(),
            handle.id(),
            policy,
            deps,
            &self.services.world_memory_path,
        )
        .with_reload_deps(ReloadDeps {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
            agent_tool_configs: agent.tools.clone(),
        });

        // Extract tool_refs from agent spec (consistent with run path)
        let tool_refs = ContextBuilder::new()
            .from_agent_spec(&agent)
            .build()
            .tool_refs;

        // Acquire per-session agentic loop lock to prevent concurrent loops
        let loop_lock = self.services.agentic_loop_locks.get(handle.id());
        let _loop_guard = loop_lock.lock().await;

        // Resume the agentic loop
        let result = match resume_agentic_loop(
            provider,
            &mut executor,
            &agent,
            ResumeContext {
                pending,
                tool_result,
            },
            &handle,
            tool_refs.as_ref(),
            None,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "Agentic loop resume failed");
                // Set session back to Active on error via actor
                let _ = handle.set_status(SessionStatus::Active).await;
                return Some(format!("Error resuming: {}", e));
            }
        };

        match result {
            AgenticResult::Complete {
                content,
                usage,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Set session back to Active via actor
                let _ = handle.set_status(SessionStatus::Active).await;

                // Persist and send (skip empty responses)
                if !content.trim().is_empty() {
                    if let Err(e) = handle.add_assistant_message(content.clone(), usage).await {
                        error!(error = %e, "Failed to persist assistant message");
                    }

                    if let Err(e) = self
                        .gateway_sender
                        .send_message(gateway, &data.chat_id, &content, None)
                        .await
                    {
                        error!(error = %e, "Failed to send response");
                    }
                }

                // Return toast to confirm the approval
                Some(toast)
            }
            AgenticResult::AwaitingApproval {
                pending: mut new_pending,
                partial_content,
                usage: _,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Tag with the same requester who approved the previous command
                new_pending.requester_id = Some(data.sender.id.clone());

                // Persist new pending approval via actor
                if let Err(e) = handle.set_pending_approval(new_pending.clone()).await {
                    error!(error = %e, "Failed to persist pending approval");
                    return Some("Error: Failed to save approval request".to_string());
                }

                // Send partial content (LLM text before tool call) if present
                let trimmed = partial_content.trim();
                if !trimmed.is_empty()
                    && let Err(e) = self
                        .gateway_sender
                        .send_message(gateway, &data.chat_id, trimmed, None)
                        .await
                {
                    error!(error = %e, "Failed to send partial content");
                }

                // Build and send new approval keyboard
                let keyboard = build_approval_keyboard();
                let approval_msg = format!(
                    "Command requires approval:\n```\n{}\n```",
                    new_pending.command
                );

                if let Err(e) = self
                    .gateway_sender
                    .send_message_with_keyboard(
                        gateway,
                        &data.chat_id,
                        &approval_msg,
                        None,
                        Some(keyboard),
                    )
                    .await
                {
                    error!(error = %e, "Failed to send approval keyboard");
                }

                // Return toast to confirm the approval (next command needs approval too)
                Some(toast)
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Truncate a command string for display in toast notifications.
///
/// Keeps the first `max_len` characters and appends "..." if truncated.
pub(super) fn truncate_command(command: &str, max_len: usize) -> String {
    // Take first line only for multi-line commands
    let first_line = command.lines().next().unwrap_or(command);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        let boundary = first_line.floor_char_boundary(max_len);
        format!("{}...", &first_line[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // truncate_command - Command display truncation
    // ------------------------------------------------------------------------

    #[test]
    fn truncate_command_short_command_unchanged() {
        let result = truncate_command("ls -la", 50);
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn truncate_command_long_command_truncated() {
        let result = truncate_command("npm install --save-dev some-very-long-package-name", 20);
        assert_eq!(result, "npm install --save-d...");
    }

    #[test]
    fn truncate_command_multiline_uses_first_line() {
        let command = "echo 'hello'\necho 'world'\necho 'goodbye'";
        let result = truncate_command(command, 50);
        assert_eq!(result, "echo 'hello'");
    }

    #[test]
    fn truncate_command_multiline_first_line_truncated() {
        let command = "npm install --save-dev package\nmore stuff";
        let result = truncate_command(command, 15);
        assert_eq!(result, "npm install --s...");
    }

    #[test]
    fn truncate_command_exact_length() {
        let result = truncate_command("12345", 5);
        assert_eq!(result, "12345");
    }
}
