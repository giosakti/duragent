//! Gateway message handler that routes messages to sessions.
//!
//! This handler bridges incoming gateway messages to the session system,
//! processing them through the LLM and returning responses.
//!
//! For agents with tools configured, messages are processed through the
//! agentic loop which supports tool execution and the approval flow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

use agnx_gateway_protocol::{CallbackQueryData, MessageContent, RoutingContext};

use super::{GatewayManager, MessageHandler, build_approval_keyboard};
use crate::agent::{AgentStore, PolicyLocks};
use crate::api::SessionStatus;
use crate::config::{RoutingMatch, RoutingRule};
use crate::llm::{Message, ProviderRegistry, Role};
use crate::sandbox::Sandbox;
use crate::session::{
    AgenticResult, ApprovalDecisionType, EventContext, SessionContext, SessionEventPayload,
    SessionLocks, SessionStore, build_chat_request, build_system_message, clear_pending_approval,
    commit_event, get_pending_approval, load_snapshot, persist_assistant_message, record_event,
    resume_agentic_loop, run_agentic_loop, set_pending_approval,
};
use crate::tools::{ToolExecutor, ToolResult};

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

/// Handler that routes gateway messages to sessions.
pub struct GatewayMessageHandler {
    agents: AgentStore,
    providers: ProviderRegistry,
    sessions: SessionStore,
    sessions_path: PathBuf,
    /// Mapping from gateway chat key to session ID.
    chat_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Routing configuration for agent selection.
    routing_config: RoutingConfig,
    /// Sandbox for tool execution.
    sandbox: Arc<dyn Sandbox>,
    /// Gateway manager for sending messages with keyboards.
    gateway_manager: GatewayManager,
    /// Per-session locks for disk I/O.
    session_locks: SessionLocks,
    /// Per-agent locks for policy file writes.
    policy_locks: PolicyLocks,
}

impl GatewayMessageHandler {
    /// Create a new gateway message handler.
    pub fn new(
        agents: AgentStore,
        providers: ProviderRegistry,
        sessions: SessionStore,
        sessions_path: PathBuf,
        routing_config: RoutingConfig,
        sandbox: Arc<dyn Sandbox>,
        gateway_manager: GatewayManager,
        session_locks: SessionLocks,
        policy_locks: PolicyLocks,
    ) -> Self {
        Self {
            agents,
            providers,
            sessions,
            sessions_path,
            chat_sessions: Arc::new(RwLock::new(HashMap::new())),
            routing_config,
            sandbox,
            gateway_manager,
            session_locks,
            policy_locks,
        }
    }

    /// Get the chat key for session mapping.
    fn chat_key(gateway: &str, chat_id: &str) -> String {
        format!("{}:{}", gateway, chat_id)
    }

    /// Get or create a session for a gateway chat.
    async fn get_or_create_session(
        &self,
        gateway: &str,
        routing: &RoutingContext,
    ) -> Option<String> {
        let key = Self::chat_key(gateway, &routing.chat_id);

        // Check if we already have a session for this chat
        {
            let sessions = self.chat_sessions.read().await;
            if let Some(session_id) = sessions.get(&key) {
                // Verify session still exists
                if self.sessions.get(session_id).await.is_some() {
                    return Some(session_id.clone());
                }
            }
        }

        // Create a new session
        let agent_name = self.resolve_agent(gateway, routing)?;
        let agent = self.agents.get(&agent_name)?;

        let session = self.sessions.create(&agent_name).await;
        let session_id = session.id.clone();

        // Persist session start event with gateway routing info
        let ctx = SessionContext {
            sessions: &self.sessions,
            sessions_path: &self.sessions_path,
            session_id: &session_id,
            agent: &session.agent,
            created_at: session.created_at,
            status: SessionStatus::Active,
            on_disconnect: agent.session.on_disconnect,
            gateway: Some(gateway),
            gateway_chat_id: Some(&routing.chat_id),
            pending_approval: None,
            session_locks: &self.session_locks,
        };
        if let Err(e) = commit_event(
            &ctx,
            SessionEventPayload::SessionStart {
                session_id: session_id.clone(),
                agent: session.agent.clone(),
            },
        )
        .await
        {
            error!(error = %e, "Failed to persist gateway session start");
        }

        // Store the mapping
        {
            let mut sessions = self.chat_sessions.write().await;
            sessions.insert(key, session_id.clone());
        }

        debug!(
            gateway = %gateway,
            chat_id = %routing.chat_id,
            session_id = %session_id,
            agent = %agent_name,
            "Created new session for gateway chat"
        );

        Some(session_id)
    }

    /// Resolve the agent to use for new sessions based on routing rules.
    ///
    /// Rules are evaluated in order; first match wins.
    /// A rule with empty match conditions acts as a catch-all.
    fn resolve_agent(&self, gateway: &str, routing: &RoutingContext) -> Option<String> {
        // Check routing rules in order (first match wins)
        for rule in &self.routing_config.rules {
            if matches_rule(&rule.match_conditions, gateway, routing) {
                if self.agents.get(&rule.agent).is_some() {
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

    /// Rebuild gateway routes from recovered sessions.
    ///
    /// Call this after session recovery to restore chat-to-session mappings.
    /// Scans all active sessions and rebuilds the routing cache from their
    /// stored gateway info.
    pub async fn rebuild_routes_from_sessions(&self) {
        let sessions = self.sessions.list().await;
        let mut routes_rebuilt = 0;

        for session in sessions {
            // Skip non-active sessions
            if session.status != SessionStatus::Active && session.status != SessionStatus::Paused {
                continue;
            }

            // Load snapshot to get gateway info
            let snapshot = match load_snapshot(&self.sessions_path, &session.id).await {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        session_id = %session.id,
                        error = %e,
                        "Failed to load snapshot for route rebuild"
                    );
                    continue;
                }
            };

            // Check if this session has gateway routing info
            if let (Some(gateway), Some(chat_id)) =
                (&snapshot.config.gateway, &snapshot.config.gateway_chat_id)
            {
                let key = Self::chat_key(gateway, chat_id);
                let mut chat_sessions = self.chat_sessions.write().await;
                chat_sessions.insert(key, session.id.clone());
                routes_rebuilt += 1;

                debug!(
                    gateway = %gateway,
                    chat_id = %chat_id,
                    session_id = %session.id,
                    "Rebuilt gateway route from session"
                );
            }
        }

        if routes_rebuilt > 0 {
            tracing::info!(
                routes = routes_rebuilt,
                "Rebuilt gateway routes from sessions"
            );
        }
    }

    /// Process a text message and return the response.
    async fn process_text_message(
        &self,
        gateway: &str,
        chat_id: &str,
        session_id: &str,
        text: &str,
    ) -> Option<String> {
        let session = self.sessions.get(session_id).await?;
        let agent = self.agents.get(&session.agent)?;

        // Add user message to session
        let user_message = Message::text(Role::User, text);
        if self
            .sessions
            .add_message(session_id, user_message)
            .await
            .is_err()
        {
            error!(session_id = %session_id, "Failed to add user message");
            return None;
        }

        // Record user message event
        if let Err(e) = record_event(
            &self.sessions,
            &self.sessions_path,
            session_id,
            SessionEventPayload::UserMessage {
                content: text.to_string(),
            },
            &self.session_locks,
        )
        .await
        {
            error!(error = %e, "Failed to persist user message event");
        }

        // Route to agentic loop if agent has tools configured
        if !agent.tools.is_empty() {
            return self
                .process_text_message_agentic(gateway, chat_id, session_id, text)
                .await;
        }

        // Simple single-turn for agents without tools
        let provider = self
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())?;

        let history = self
            .sessions
            .get_messages(session_id)
            .await
            .unwrap_or_default();
        let system_message = build_system_message(agent);
        let chat_request = build_chat_request(
            &agent.model.name,
            system_message.as_deref(),
            &history,
            agent.model.temperature,
            agent.model.max_output_tokens,
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

        if let Err(e) = persist_assistant_message(
            &self.sessions,
            &self.sessions_path,
            session_id,
            &session.agent,
            assistant_content.clone(),
            response.usage,
            &self.session_locks,
        )
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
        session_id: &str,
        _text: &str,
    ) -> Option<String> {
        let session = self.sessions.get(session_id).await?;
        let agent = self.agents.get(&session.agent)?;

        let provider = self
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())?;

        // Create tool executor
        let executor = ToolExecutor::new(
            agent.tools.clone(),
            self.sandbox.clone(),
            agent.agent_dir.clone(),
            agent.policy.clone(),
            session.agent.clone(),
        )
        .with_session_id(session_id.to_string());

        // Build initial messages from history
        let history = self
            .sessions
            .get_messages(session_id)
            .await
            .unwrap_or_default();
        let system_message = build_system_message(agent);
        let mut messages = Vec::new();
        if let Some(sys) = system_message {
            messages.push(Message::text(Role::System, sys));
        }
        messages.extend(history);

        // Run agentic loop
        let event_ctx = EventContext {
            sessions: self.sessions.clone(),
            sessions_path: self.sessions_path.clone(),
            session_id: session_id.to_string(),
            session_locks: self.session_locks.clone(),
        };
        let result = match run_agentic_loop(provider, &executor, agent, messages, &event_ctx).await
        {
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
                // Persist final assistant message
                if let Err(e) = persist_assistant_message(
                    &self.sessions,
                    &self.sessions_path,
                    session_id,
                    &session.agent,
                    content.clone(),
                    usage,
                    &self.session_locks,
                )
                .await
                {
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
                // Persist pending approval to snapshot
                if let Err(e) = set_pending_approval(
                    &self.sessions,
                    &self.sessions_path,
                    session_id,
                    &pending,
                    &self.session_locks,
                )
                .await
                {
                    error!(error = %e, "Failed to persist pending approval");
                    return Some("Error: Failed to save approval request".to_string());
                }

                // Set session status to Paused
                if let Err(e) = self
                    .sessions
                    .set_status(session_id, SessionStatus::Paused)
                    .await
                {
                    debug!(error = %e, "Failed to set session status to Paused");
                }

                // Build and send approval keyboard
                let keyboard = build_approval_keyboard();
                let approval_msg =
                    format!("Command requires approval:\n```\n{}\n```", pending.command);

                if let Err(e) = self
                    .gateway_manager
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
        let session_id = self.get_or_create_session(gateway, routing).await?;

        // Process based on content type
        match content {
            MessageContent::Text { text } => {
                self.process_text_message(gateway, &routing.chat_id, &session_id, text)
                    .await
            }
            MessageContent::Media { caption, .. } => {
                // For media, process the caption if present
                if let Some(caption) = caption
                    && !caption.is_empty()
                {
                    return self
                        .process_text_message(gateway, &routing.chat_id, &session_id, caption)
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

        // Look up session from chat_id
        let key = Self::chat_key(gateway, &data.chat_id);
        let session_id = {
            let sessions = self.chat_sessions.read().await;
            sessions.get(&key).cloned()
        };

        let session_id = match session_id {
            Some(id) => id,
            None => {
                warn!(chat_id = %data.chat_id, "No session found for chat");
                return Some("No active session".to_string());
            }
        };

        // Verify session exists
        let session = match self.sessions.get(&session_id).await {
            Some(s) => s,
            None => {
                warn!(session_id = %session_id, "Session not found for approval callback");
                return Some("Session not found".to_string());
            }
        };

        // Load pending approval from snapshot
        let pending = match get_pending_approval(&self.sessions_path, &session_id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                warn!(session_id = %session_id, "No pending approval found");
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

        // Record the approval decision event
        if let Err(e) = record_event(
            &self.sessions,
            &self.sessions_path,
            &session_id,
            SessionEventPayload::ApprovalDecision {
                call_id: pending.call_id.clone(),
                decision: decision_type,
            },
            &self.session_locks,
        )
        .await
        {
            error!(error = %e, "Failed to record approval decision");
            return Some("Failed to process approval".to_string());
        }

        // Get agent spec for tool execution
        let agent = match self.agents.get(&session.agent) {
            Some(a) => a,
            None => {
                error!(agent = %session.agent, "Agent not found");
                return Some("Agent configuration error".to_string());
            }
        };

        // If allow_always, save pattern to policy.local.yaml
        if decision_type == ApprovalDecisionType::AllowAlways {
            if let Err(e) = crate::agent::ToolPolicy::add_pattern_and_save(
                &agent.policy,
                &agent.agent_dir,
                &session.agent,
                crate::agent::ToolType::Bash,
                &pending.command,
                &self.policy_locks,
            )
            .await
            {
                debug!(
                    error = %e,
                    command = %pending.command,
                    "Failed to save allow pattern to policy.local.yaml"
                );
            }
        }

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
            let executor = ToolExecutor::new(
                agent.tools.clone(),
                self.sandbox.clone(),
                agent.agent_dir.clone(),
                agent.policy.clone(),
                session.agent.clone(),
            )
            .with_session_id(session_id.to_string());

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

        // Clear pending approval
        if let Err(e) = clear_pending_approval(
            &self.sessions,
            &self.sessions_path,
            &session_id,
            &self.session_locks,
        )
        .await
        {
            debug!(error = %e, "Failed to clear pending approval");
        }

        // Get provider for resuming the loop
        let provider = match self
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())
        {
            Some(p) => p,
            None => {
                error!(provider = %agent.model.provider, "Provider not configured");
                return Some("Provider configuration error".to_string());
            }
        };

        // Create executor for resume
        let executor = ToolExecutor::new(
            agent.tools.clone(),
            self.sandbox.clone(),
            agent.agent_dir.clone(),
            agent.policy.clone(),
            session.agent.clone(),
        )
        .with_session_id(session_id.to_string());

        // Resume the agentic loop
        let event_ctx = EventContext {
            sessions: self.sessions.clone(),
            sessions_path: self.sessions_path.clone(),
            session_id: session_id.to_string(),
            session_locks: self.session_locks.clone(),
        };
        let result =
            match resume_agentic_loop(provider, &executor, agent, pending, tool_result, &event_ctx)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "Agentic loop resume failed");
                    // Set session back to Active on error
                    let _ = self
                        .sessions
                        .set_status(&session_id, SessionStatus::Active)
                        .await;
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
                // Set session back to Active
                let _ = self
                    .sessions
                    .set_status(&session_id, SessionStatus::Active)
                    .await;

                // Persist final assistant message
                if let Err(e) = persist_assistant_message(
                    &self.sessions,
                    &self.sessions_path,
                    &session_id,
                    &session.agent,
                    content.clone(),
                    usage,
                    &self.session_locks,
                )
                .await
                {
                    error!(error = %e, "Failed to persist assistant message");
                }

                // Send response via gateway
                if let Err(e) = self
                    .gateway_manager
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
                // Persist new pending approval
                if let Err(e) = set_pending_approval(
                    &self.sessions,
                    &self.sessions_path,
                    &session_id,
                    &new_pending,
                    &self.session_locks,
                )
                .await
                {
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
                    .gateway_manager
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
    fn test_chat_key() {
        let key = GatewayMessageHandler::chat_key("telegram", "12345");
        assert_eq!(key, "telegram:12345");
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
}
