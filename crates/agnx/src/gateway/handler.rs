//! Gateway message handler that routes messages to sessions.
//!
//! This handler bridges incoming gateway messages to the session system,
//! processing them through the LLM and returning responses.
//!
//! For agents with tools configured, messages are processed through the
//! agentic loop which supports tool execution and the approval flow.

use async_trait::async_trait;
use tracing::{debug, error, warn};

use agnx_gateway_protocol::{CallbackQueryData, MessageContent, RoutingContext};

use super::{MessageHandler, build_approval_keyboard};
use crate::agent::PolicyLocks;
use crate::api::SessionStatus;
use crate::config::{RoutingMatch, RoutingRule};
use crate::context::{ContextBuilder, load_all_directives};
use crate::scheduler::SchedulerHandle;
use crate::server::RuntimeServices;
use crate::session::{
    AgenticResult, ApprovalDecisionType, ChatSessionCache, SessionHandle, resume_agentic_loop,
    run_agentic_loop,
};
use crate::sync::KeyedLocks;
use crate::tools::{ToolDependencies, ToolExecutionContext, ToolResult, build_executor};

// ============================================================================
// Routing Config
// ============================================================================

/// Routing configuration for agent selection.
///
/// Contains global routing rules evaluated in order (first match wins).
/// A rule with empty/no match conditions acts as a catch-all.
#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// Routing rules (evaluated in order, first match wins).
    /// Last rule with empty match acts as default/catch-all.
    pub rules: Vec<RoutingRule>,
}

impl RoutingConfig {
    /// Create a config from a list of routing rules.
    pub fn new(rules: Vec<RoutingRule>) -> Self {
        Self { rules }
    }

    /// Create an empty config (no routes - messages will be dropped).
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }
}

// ============================================================================
// Gateway Message Handler
// ============================================================================

/// Configuration for creating a gateway message handler.
pub struct GatewayHandlerConfig {
    pub services: RuntimeServices,
    pub routing_config: RoutingConfig,
    pub policy_locks: PolicyLocks,
    pub scheduler: Option<SchedulerHandle>,
    pub chat_session_cache: ChatSessionCache,
}

/// Handler that routes gateway messages to sessions.
pub struct GatewayMessageHandler {
    services: RuntimeServices,
    /// Shared cache mapping (gateway, chat_id, agent) to session_id.
    chat_session_cache: ChatSessionCache,
    /// Per-session locks to serialize message and callback processing.
    message_locks: KeyedLocks,
    /// Routing configuration for agent selection.
    routing_config: RoutingConfig,
    /// Per-agent locks for policy file writes.
    policy_locks: PolicyLocks,
    /// Scheduler handle for schedule tools.
    scheduler: Option<SchedulerHandle>,
}

impl GatewayMessageHandler {
    /// Create a new gateway message handler.
    pub fn new(config: GatewayHandlerConfig) -> Self {
        Self {
            services: config.services,
            chat_session_cache: config.chat_session_cache,
            message_locks: KeyedLocks::with_cleanup("gateway_message_locks"),
            routing_config: config.routing_config,
            policy_locks: config.policy_locks,
            scheduler: config.scheduler,
        }
    }

    /// Get or create a session for a gateway chat.
    ///
    /// Uses the shared `ChatSessionCache` to atomically look up or create sessions by
    /// (gateway, chat_id, agent) tuple. This enables long-lived sessions
    /// that persist across gateway messages and scheduled tasks.
    ///
    /// The atomic get-or-insert pattern prevents race conditions where two concurrent
    /// callers could both miss the cache and create duplicate sessions.
    async fn get_or_create_session(
        &self,
        gateway: &str,
        routing: &RoutingContext,
    ) -> Option<SessionHandle> {
        // Resolve agent first - we need it for the cache key
        let agent_name = self.resolve_agent(gateway, routing)?;

        // Get the agent spec for session creation (needed if we create a new session)
        let agent = self.services.agents.get(&agent_name)?;

        // Clone values needed in closures
        let registry = self.services.session_registry.clone();
        let agent_name_clone = agent_name.clone();
        let gateway_clone = gateway.to_string();
        let chat_id_clone = routing.chat_id.clone();
        let on_disconnect = agent.session.on_disconnect;

        // Use atomic get-or-insert to prevent race conditions
        let session_id = match self
            .chat_session_cache
            .get_or_insert_with(
                gateway,
                &routing.chat_id,
                &agent_name,
                // Validator: check if the cached session still exists
                |session_id| {
                    let registry = registry.clone();
                    async move { registry.contains(&session_id) }
                },
                // Creator: create a new session if needed
                || {
                    let registry = registry.clone();
                    let agent_name = agent_name_clone.clone();
                    let gateway = gateway_clone.clone();
                    let chat_id = chat_id_clone.clone();
                    async move {
                        let handle = registry
                            .create(
                                &agent_name,
                                on_disconnect,
                                Some(gateway.clone()),
                                Some(chat_id.clone()),
                            )
                            .await?;

                        let session_id = handle.id().to_string();

                        debug!(
                            gateway = %gateway,
                            chat_id = %chat_id,
                            session_id = %session_id,
                            agent = %agent_name,
                            "Created new session for gateway chat"
                        );

                        Ok::<_, crate::session::ActorError>(session_id)
                    }
                },
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    gateway = %gateway,
                    chat_id = %routing.chat_id,
                    error = %e,
                    "Failed to create session for gateway chat"
                );
                return None;
            }
        };

        // Get handle from registry
        self.services.session_registry.get(&session_id)
    }

    /// Resolve the agent to use for new sessions based on routing rules.
    ///
    /// Rules are evaluated in order; first match wins.
    /// A rule with empty match conditions acts as a catch-all.
    fn resolve_agent(&self, gateway: &str, routing: &RoutingContext) -> Option<String> {
        // Check routing rules in order (first match wins)
        for rule in &self.routing_config.rules {
            if matches_rule(&rule.match_conditions, gateway, routing) {
                if self.services.agents.get(&rule.agent).is_some() {
                    return Some(rule.agent.clone());
                }
                warn!(agent = %rule.agent, "Routing rule agent not found");
            }
        }

        // No matching rule - fail closed
        warn!(
            gateway = %gateway,
            channel = %routing.channel,
            chat_id = %routing.chat_id,
            "No matching route found, message dropped"
        );
        None
    }

    /// Get the shared chat session cache.
    ///
    /// This is used by the scheduler to look up and share sessions.
    pub fn chat_session_cache(&self) -> &ChatSessionCache {
        &self.chat_session_cache
    }

    /// Find an existing session for a gateway chat.
    ///
    /// Used by callback queries where we know the gateway and chat_id but not
    /// which agent was routed to. Tries each agent in the routing rules until
    /// we find a cached session.
    async fn find_session_for_chat(&self, gateway: &str, chat_id: &str) -> Option<SessionHandle> {
        // Check each agent that could have been routed to this chat
        for rule in &self.routing_config.rules {
            if let Some(session_id) = self
                .chat_session_cache
                .get(gateway, chat_id, &rule.agent)
                .await
            {
                // Verify session still exists and return handle
                if let Some(handle) = self.services.session_registry.get(&session_id) {
                    return Some(handle);
                }
            }
        }
        None
    }

    /// Process a text message and return the response.
    async fn process_text_message(
        &self,
        gateway: &str,
        chat_id: &str,
        handle: &SessionHandle,
        text: &str,
    ) -> Option<String> {
        let agent = self.services.agents.get(handle.agent())?;

        // Persist user message via actor
        if let Err(e) = handle.add_user_message(text.to_string()).await {
            error!(session_id = %handle.id(), error = %e, "Failed to persist user message");
            return None;
        }

        // Route to agentic loop if agent has tools configured
        if !agent.tools.is_empty() {
            return self
                .process_text_message_agentic(gateway, chat_id, handle, text)
                .await;
        }

        // Simple single-turn for agents without tools
        let provider = self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())?;

        let history = match handle.get_messages().await {
            Ok(msgs) => msgs,
            Err(e) => {
                error!(error = %e, "Failed to get messages");
                return None;
            }
        };

        // Build structured context and render to ChatRequest
        let directives =
            load_all_directives(&self.services.workspace_directives_path, &agent.agent_dir);
        let chat_request = ContextBuilder::new()
            .from_agent_spec(agent)
            .with_messages(history)
            .with_directives(directives)
            .build()
            .render(
                &agent.model.name,
                agent.model.temperature,
                agent.model.max_output_tokens,
                vec![],
            );

        let response = match provider.chat(chat_request).await {
            Ok(resp) => resp,
            Err(e) => {
                error!(error = %e, "LLM request failed");
                return None;
            }
        };

        let assistant_content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        if let Err(e) = handle
            .add_assistant_message(assistant_content.clone(), response.usage)
            .await
        {
            error!(error = %e, "Failed to persist assistant message");
        }

        Some(assistant_content)
    }

    /// Process a text message using the agentic loop with tool support.
    async fn process_text_message_agentic(
        &self,
        gateway: &str,
        chat_id: &str,
        handle: &SessionHandle,
        _text: &str,
    ) -> Option<String> {
        let agent = self.services.agents.get(handle.agent())?;

        let provider = self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())?;

        // Load policy from store (picks up runtime changes from AllowAlways)
        let policy = self.services.policy_store.load(handle.agent()).await;

        // Create tool executor with execution context for schedule tools
        let execution_context = self.scheduler.as_ref().map(|_| ToolExecutionContext {
            gateway: Some(gateway.to_string()),
            chat_id: Some(chat_id.to_string()),
            agent: handle.agent().to_string(),
            session_id: handle.id().to_string(),
        });
        let deps = ToolDependencies {
            sandbox: self.services.sandbox.clone(),
            agent_dir: agent.agent_dir.clone(),
            scheduler: self.scheduler.clone(),
            execution_context,
        };
        let executor = build_executor(
            agent,
            handle.agent(),
            handle.id(),
            policy,
            deps,
            &self.services.world_memory_path,
        );

        // Build initial messages from history using StructuredContext
        let history = match handle.get_messages().await {
            Ok(msgs) => msgs,
            Err(e) => {
                error!(error = %e, "Failed to get messages");
                return Some(format!("Error: {}", e));
            }
        };
        let directives =
            load_all_directives(&self.services.workspace_directives_path, &agent.agent_dir);
        let messages = ContextBuilder::new()
            .from_agent_spec(agent)
            .with_messages(history)
            .with_directives(directives)
            .build()
            .render(
                &agent.model.name,
                agent.model.temperature,
                agent.model.max_output_tokens,
                vec![],
            )
            .messages;

        // Run agentic loop
        let result =
            match run_agentic_loop(provider, &executor, agent, messages, handle, None).await {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "Agentic loop failed");
                    return Some(format!("Error processing request: {}", e));
                }
            };

        match result {
            AgenticResult::Complete {
                content,
                usage,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Persist final assistant message via actor
                if let Err(e) = handle.add_assistant_message(content.clone(), usage).await {
                    error!(error = %e, "Failed to persist assistant message");
                }
                Some(content)
            }
            AgenticResult::AwaitingApproval {
                pending,
                partial_content: _,
                usage: _,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Persist pending approval via actor
                if let Err(e) = handle.set_pending_approval(pending.clone()).await {
                    error!(error = %e, "Failed to persist pending approval");
                    return Some("Error: Failed to save approval request".to_string());
                }

                // Set session status to Paused via actor
                if let Err(e) = handle.set_status(SessionStatus::Paused).await {
                    debug!(error = %e, "Failed to set session status to Paused");
                }

                // Build and send approval keyboard
                let keyboard = build_approval_keyboard();
                let approval_msg =
                    format!("Command requires approval:\n```\n{}\n```", pending.command);

                if let Err(e) = self
                    .services
                    .gateways
                    .send_message_with_keyboard(
                        gateway,
                        chat_id,
                        &approval_msg,
                        None,
                        Some(keyboard),
                    )
                    .await
                {
                    error!(error = %e, "Failed to send approval keyboard");
                }

                // Return None since we sent the message with keyboard directly
                None
            }
        }
    }
}

#[async_trait]
impl MessageHandler for GatewayMessageHandler {
    async fn handle_message(
        &self,
        gateway: &str,
        routing: &RoutingContext,
        content: &MessageContent,
    ) -> Option<String> {
        // Get or create session for this chat
        let handle = self.get_or_create_session(gateway, routing).await?;
        let lock = self.message_locks.get(handle.id());
        let _guard = lock.lock().await;

        // Process based on content type
        match content {
            MessageContent::Text { text } => {
                self.process_text_message(gateway, &routing.chat_id, &handle, text)
                    .await
            }
            MessageContent::Media { caption, .. } => {
                // For media, process the caption if present
                if let Some(caption) = caption
                    && !caption.is_empty()
                {
                    return self
                        .process_text_message(gateway, &routing.chat_id, &handle, caption)
                        .await;
                }
                None
            }
            _ => {
                debug!(
                    gateway = %gateway,
                    content_type = ?content,
                    "Ignoring non-text message content"
                );
                None
            }
        }
    }

    async fn handle_callback_query(
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
            && let Err(e) = crate::agent::ToolPolicy::add_pattern_and_save(
                self.services.policy_store.as_ref(),
                handle.agent(),
                crate::agent::ToolType::Bash,
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
            };
            let executor = build_executor(
                agent,
                handle.agent(),
                handle.id(),
                policy.clone(),
                deps,
                &self.services.world_memory_path,
            );

            // Build tool call from pending approval
            let tool_call = crate::llm::ToolCall {
                id: pending.call_id.clone(),
                tool_type: "function".to_string(),
                function: crate::llm::FunctionCall {
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
        };
        let executor = build_executor(
            agent,
            handle.agent(),
            handle.id(),
            policy,
            deps,
            &self.services.world_memory_path,
        );

        // Resume the agentic loop
        let result = match resume_agentic_loop(
            provider,
            &executor,
            agent,
            pending,
            tool_result,
            &handle,
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

                // Persist final assistant message via actor
                if let Err(e) = handle.add_assistant_message(content.clone(), usage).await {
                    error!(error = %e, "Failed to persist assistant message");
                }

                // Send response via gateway
                if let Err(e) = self
                    .services
                    .gateways
                    .send_message(gateway, &data.chat_id, &content, None)
                    .await
                {
                    error!(error = %e, "Failed to send response");
                }

                // Return toast to confirm the approval
                Some(toast)
            }
            AgenticResult::AwaitingApproval {
                pending: new_pending,
                partial_content: _,
                usage: _,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Persist new pending approval via actor
                if let Err(e) = handle.set_pending_approval(new_pending.clone()).await {
                    error!(error = %e, "Failed to persist pending approval");
                    return Some("Error: Failed to save approval request".to_string());
                }

                // Build and send new approval keyboard
                let keyboard = build_approval_keyboard();
                let approval_msg = format!(
                    "Command requires approval:\n```\n{}\n```",
                    new_pending.command
                );

                if let Err(e) = self
                    .services
                    .gateways
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
fn truncate_command(command: &str, max_len: usize) -> String {
    // Take first line only for multi-line commands
    let first_line = command.lines().next().unwrap_or(command);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max_len])
    }
}

/// Check if a routing rule matches the given context.
///
/// Gateway and chat_type comparisons are case-insensitive to prevent
/// "Telegram" vs "telegram" bugs. Chat ID and sender ID remain case-sensitive
/// as they are exact identifiers.
fn matches_rule(conditions: &RoutingMatch, gateway: &str, routing: &RoutingContext) -> bool {
    // All specified conditions must match (AND logic)
    if let Some(ref gw) = conditions.gateway
        && !gw.eq_ignore_ascii_case(gateway)
    {
        return false;
    }
    if let Some(ref chat_type) = conditions.chat_type
        && !chat_type.eq_ignore_ascii_case(&routing.chat_type)
    {
        return false;
    }
    if let Some(ref chat_id) = conditions.chat_id
        && chat_id != &routing.chat_id
    {
        return false;
    }
    if let Some(ref sender_id) = conditions.sender_id
        && sender_id != &routing.sender_id
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn make_routing_context(chat_type: &str, chat_id: &str, sender_id: &str) -> RoutingContext {
        RoutingContext {
            channel: "telegram".to_string(),
            chat_type: chat_type.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: sender_id.to_string(),
            extra: HashMap::new(),
        }
    }

    #[test]
    fn test_chat_session_cache_key() {
        // Cache key now includes agent for routing rule changes
        let key = ChatSessionCache::key("telegram", "12345", "my-agent");
        assert_eq!(key, "telegram:12345:my-agent");
    }

    #[test]
    fn test_matches_rule_empty_conditions() {
        let conditions = RoutingMatch::default();
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_chat_type_match() {
        let conditions = RoutingMatch {
            chat_type: Some("dm".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_chat_type_no_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_chat_id_match() {
        let conditions = RoutingMatch {
            chat_id: Some("123".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_chat_id_no_match() {
        let conditions = RoutingMatch {
            chat_id: Some("999".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_sender_id_match() {
        let conditions = RoutingMatch {
            sender_id: Some("456".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_sender_id_no_match() {
        let conditions = RoutingMatch {
            sender_id: Some("999".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_multiple_conditions_all_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            chat_id: Some("-100123".to_string()),
            sender_id: Some("456".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("group", "-100123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_multiple_conditions_partial_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            chat_id: Some("-100123".to_string()),
            sender_id: Some("456".to_string()),
            ..Default::default()
        };
        // chat_type matches, but sender_id doesn't
        let routing = make_routing_context("group", "-100123", "789");
        assert!(!matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_gateway_match() {
        let conditions = RoutingMatch {
            gateway: Some("telegram".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_gateway_no_match() {
        let conditions = RoutingMatch {
            gateway: Some("discord".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, "telegram", &routing));
    }

    #[test]
    fn test_matches_rule_gateway_case_insensitive() {
        let conditions = RoutingMatch {
            gateway: Some("Telegram".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        // "Telegram" in config should match "telegram" gateway
        assert!(matches_rule(&conditions, "telegram", &routing));
        assert!(matches_rule(&conditions, "TELEGRAM", &routing));
        assert!(matches_rule(&conditions, "TeleGram", &routing));
    }

    #[test]
    fn test_matches_rule_chat_type_case_insensitive() {
        let conditions = RoutingMatch {
            chat_type: Some("DM".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        // "DM" in config should match "dm" from routing context
        assert!(matches_rule(&conditions, "telegram", &routing));

        let routing_upper = make_routing_context("DM", "123", "456");
        assert!(matches_rule(&conditions, "telegram", &routing_upper));
    }

    #[test]
    fn test_matches_rule_chat_id_case_sensitive() {
        // Chat IDs should remain case-sensitive (they're exact identifiers)
        let conditions = RoutingMatch {
            chat_id: Some("ABC123".to_string()),
            ..Default::default()
        };
        let routing_lower = make_routing_context("dm", "abc123", "456");
        let routing_exact = make_routing_context("dm", "ABC123", "456");

        assert!(!matches_rule(&conditions, "telegram", &routing_lower));
        assert!(matches_rule(&conditions, "telegram", &routing_exact));
    }

    #[test]
    fn test_matches_rule_sender_id_case_sensitive() {
        // Sender IDs should remain case-sensitive (they're exact identifiers)
        let conditions = RoutingMatch {
            sender_id: Some("User123".to_string()),
            ..Default::default()
        };
        let routing_lower = make_routing_context("dm", "123", "user123");
        let routing_exact = make_routing_context("dm", "123", "User123");

        assert!(!matches_rule(&conditions, "telegram", &routing_lower));
        assert!(matches_rule(&conditions, "telegram", &routing_exact));
    }

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

    // ------------------------------------------------------------------------
    // RoutingConfig - Configuration
    // ------------------------------------------------------------------------

    #[test]
    fn routing_config_new_from_rules() {
        use crate::config::RoutingRule;

        let rules = vec![
            RoutingRule {
                agent: "agent1".to_string(),
                match_conditions: RoutingMatch {
                    gateway: Some("telegram".to_string()),
                    ..Default::default()
                },
            },
            RoutingRule {
                agent: "default".to_string(),
                match_conditions: RoutingMatch::default(),
            },
        ];

        let config = RoutingConfig::new(rules);
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].agent, "agent1");
    }

    #[test]
    fn routing_config_empty() {
        let config = RoutingConfig::empty();
        assert!(config.rules.is_empty());
    }
}
