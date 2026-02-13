//! Gateway message handler that routes messages to sessions.
//!
//! This handler bridges incoming gateway messages to the session system,
//! processing them through the LLM and returning responses.
//!
//! For agents with tools configured, messages are processed through the
//! agentic loop which supports tool execution and the approval flow.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use duragent_gateway_protocol::{
    CallbackQueryData, MessageContent, MessageReceivedData, RoutingContext, Sender,
};

use super::manager::GatewaySender;
use super::queue::{
    DrainResult, EnqueueResult, QueuedMessage, SessionMessageQueues, combine_messages,
};
use super::routing::{RoutingConfig, is_group_chat};
use super::{MessageHandler, build_approval_keyboard};
use crate::agent::access::{check_access, resolve_sender_disposition};
use crate::agent::{
    ActivationMode, AgentSpec, ContextBufferConfig, ContextBufferMode, PolicyLocks, QueueConfig,
    SenderDisposition,
};
use crate::api::SessionStatus;
use crate::context::{
    BlockSource, ContextBuilder, SystemBlock, TokenBudget, load_all_directives, priority,
};
use crate::scheduler::SchedulerHandle;
use crate::server::RuntimeServices;
use crate::session::{AgenticResult, ChatSessionCache, SessionHandle, run_agentic_loop};
use crate::sync::KeyedLocks;
use crate::tools::{ReloadDeps, ToolDependencies, ToolExecutionContext, build_executor};

// ============================================================================
// Gateway Message Handler
// ============================================================================

/// Configuration for creating a gateway message handler.
pub struct GatewayHandlerConfig {
    pub services: RuntimeServices,
    pub gateway_sender: GatewaySender,
    pub routing_config: RoutingConfig,
    pub policy_locks: PolicyLocks,
    pub scheduler: Option<SchedulerHandle>,
    pub chat_session_cache: ChatSessionCache,
}

/// Handler that routes gateway messages to sessions.
pub struct GatewayMessageHandler {
    pub(super) services: RuntimeServices,
    /// Send-only handle for gateway communication.
    pub(super) gateway_sender: GatewaySender,
    /// Shared cache mapping (gateway, chat_id, agent) to session_id.
    pub(super) chat_session_cache: ChatSessionCache,
    /// Per-session locks to serialize callback query processing.
    pub(super) message_locks: KeyedLocks,
    /// Per-session message queues for concurrent message handling.
    pub(super) session_queues: SessionMessageQueues,
    /// Routing configuration for agent selection.
    pub(super) routing_config: RoutingConfig,
    /// Per-agent locks for policy file writes.
    pub(super) policy_locks: PolicyLocks,
    /// Scheduler handle for schedule tools.
    pub(super) scheduler: Option<SchedulerHandle>,
}

// ============================================================================
// Public API
// ============================================================================

impl GatewayMessageHandler {
    /// Create a new gateway message handler.
    pub fn new(config: GatewayHandlerConfig) -> Self {
        let session_queues = SessionMessageQueues::new();
        session_queues
            .clone()
            .spawn_cleanup_task("gateway_session_queues");

        Self {
            services: config.services,
            gateway_sender: config.gateway_sender,
            chat_session_cache: config.chat_session_cache,
            message_locks: KeyedLocks::with_cleanup("gateway_callback_locks"),
            session_queues,
            routing_config: config.routing_config,
            policy_locks: config.policy_locks,
            scheduler: config.scheduler,
        }
    }

    /// Get the shared chat session cache.
    ///
    /// This is used by the scheduler to look up and share sessions.
    pub fn chat_session_cache(&self) -> &ChatSessionCache {
        &self.chat_session_cache
    }
}

// ============================================================================
// MessageHandler Trait Implementation
// ============================================================================

#[async_trait]
impl MessageHandler for GatewayMessageHandler {
    async fn handle_message(&self, gateway: &str, data: &MessageReceivedData) -> Option<String> {
        let routing = &data.routing;
        let content = &data.content;
        let sender = &data.sender;

        // Check for slash commands before routing
        if let Some(text) = extract_text(content)
            && let Some(command) = text.trim().strip_prefix('/')
            && let Some(response) = self
                .handle_command(command, gateway, &routing.chat_id)
                .await
        {
            return Some(response);
        }

        // Get or create session for this chat
        let handle = self.get_or_create_session(gateway, routing).await?;

        // Check agent-level access control
        let agent = self.services.agents.get(handle.agent())?;
        if let Some(ref access) = agent.access {
            if !check_access(
                access,
                &routing.chat_type,
                &routing.channel,
                &routing.chat_id,
                &sender.id,
            ) {
                debug!(
                    gateway = %gateway,
                    chat_id = %routing.chat_id,
                    sender_id = %sender.id,
                    chat_type = %routing.chat_type,
                    "Message denied by agent access policy"
                );
                return None;
            }

            // For group messages, check sender disposition
            if is_group_chat(&routing.chat_type) {
                match resolve_sender_disposition(&access.groups, &sender.id) {
                    SenderDisposition::Block => {
                        debug!(
                            gateway = %gateway,
                            sender_id = %sender.id,
                            "Sender blocked in group"
                        );
                        return None;
                    }
                    SenderDisposition::Silent => {
                        // Store in events.jsonl for audit, but LLM never sees it
                        if let Some(text) = extract_text(content) {
                            let sender_label = resolve_sender_label(sender);
                            let prefixed = format!("{}: {}", sender_label, text);
                            if let Err(e) = handle
                                .add_silent_message(prefixed, sender.id.clone(), Some(sender_label))
                                .await
                            {
                                warn!(error = %e, "Failed to persist silent message");
                            }
                        }
                        return None;
                    }
                    SenderDisposition::Passive => {
                        // Store as regular UserMessage (LLM sees in future turns) but don't trigger a response
                        if let Some(text) = extract_text(content) {
                            let sender_label = resolve_sender_label(sender);
                            let prefixed = format!("{}: {}", sender_label, text);
                            if let Err(e) = handle
                                .add_user_message_with_sender(
                                    prefixed,
                                    Some(sender.id.clone()),
                                    Some(sender_label),
                                )
                                .await
                            {
                                warn!(error = %e, "Failed to persist passive message");
                            }
                        }
                        return None;
                    }
                    SenderDisposition::Allow => {
                        // Mention gating: in mention mode, non-triggered messages go to context buffer
                        if access.groups.activation == ActivationMode::Mention
                            && !data.mentions_bot
                            && !data.reply_to_bot
                        {
                            if let Some(text) = extract_text(content) {
                                let sender_label = resolve_sender_label(sender);
                                let prefixed = format!("{}: {}", sender_label, text);
                                match access.groups.context_buffer.mode {
                                    ContextBufferMode::Silent => {
                                        if let Err(e) = handle
                                            .add_silent_message(
                                                prefixed,
                                                sender.id.clone(),
                                                Some(sender_label),
                                            )
                                            .await
                                        {
                                            warn!(error = %e, "Failed to persist context buffer message");
                                        }
                                    }
                                    ContextBufferMode::Passive => {
                                        if let Err(e) = handle
                                            .add_user_message_with_sender(
                                                prefixed,
                                                Some(sender.id.clone()),
                                                Some(sender_label),
                                            )
                                            .await
                                        {
                                            warn!(error = %e, "Failed to persist context buffer message");
                                        }
                                    }
                                }
                            }
                            return None;
                        }
                    }
                }
            }
        }

        // Build the queued message
        let queued = QueuedMessage {
            gateway: gateway.to_string(),
            data: data.clone(),
            enqueued_at: Instant::now(),
        };

        let queue_config = self.resolve_queue_config(&handle, routing);
        let queue = self.session_queues.get(handle.id());

        // Debounce or direct enqueue based on config
        let (enqueue_result, start_timer) = if queue_config.debounce.enabled {
            queue
                .debounce_or_enqueue(queued.clone(), &queue_config)
                .await
        } else {
            (
                queue.try_enqueue(queued.clone(), &queue_config).await,
                false,
            )
        };

        // Start debounce timer if this is the first message from this sender
        if start_timer {
            let queue_clone = Arc::clone(&queue);
            let config_clone = queue_config.clone();
            let sender_id = sender.id.clone();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(
                    config_clone.debounce.window_ms,
                ))
                .await;
                // Flush debounce buffer into the pending queue.
                // If the session is idle, flush_debounce sends a wake notification
                // which will be picked up by the drain loop's wake listener.
                let _ = queue_clone.flush_debounce(&sender_id, &config_clone).await;
            });
        }

        match enqueue_result {
            EnqueueResult::ProcessNow => {
                // Process the message directly
                let response = self.process_queued_message(&queued, &handle).await;

                // Run drain loop for any messages that arrived while processing
                self.drain_loop(&handle, &queue_config).await;

                response
            }
            EnqueueResult::Queued | EnqueueResult::Debounced => {
                // Message is queued/debounced — response will come later via drain loop
                None
            }
            EnqueueResult::DroppedNew => {
                // Silently dropped
                None
            }
            EnqueueResult::Rejected => {
                // Return reject message if configured
                queue_config.reject_message.clone()
            }
        }
    }

    async fn handle_callback_query(
        &self,
        gateway: &str,
        data: &CallbackQueryData,
    ) -> Option<String> {
        self.process_callback_query(gateway, data).await
    }
}

// ============================================================================
// Message Processing
// ============================================================================

impl GatewayMessageHandler {
    /// Build a context buffer system block from recent silent messages.
    ///
    /// Returns None if there are no recent silent messages.
    async fn build_context_buffer_block(
        &self,
        handle: &SessionHandle,
        config: &ContextBufferConfig,
    ) -> Option<SystemBlock> {
        let max_age = chrono::Duration::hours(config.max_age_hours as i64);
        let entries = handle
            .get_recent_silent_messages(config.max_messages, max_age)
            .await
            .ok()?;

        if entries.is_empty() {
            return None;
        }

        let mut buffer = String::from("Recent group conversation before you were mentioned:\n\n");
        for entry in &entries {
            buffer.push_str(&entry.content);
            buffer.push('\n');
        }

        Some(SystemBlock {
            content: buffer,
            label: "group_context".to_string(),
            source: BlockSource::Session,
            priority: priority::SESSION,
        })
    }

    /// Check if context buffer injection should apply for this routing context.
    ///
    /// Returns `None` for passive mode since those messages are already in conversation history.
    fn should_inject_context_buffer<'a>(
        &self,
        agent: &'a AgentSpec,
        routing: &RoutingContext,
    ) -> Option<&'a ContextBufferConfig> {
        if !is_group_chat(&routing.chat_type) {
            return None;
        }
        let access = agent.access.as_ref()?;
        if access.groups.activation != ActivationMode::Mention {
            return None;
        }
        if access.groups.context_buffer.mode == ContextBufferMode::Passive {
            return None;
        }
        Some(&access.groups.context_buffer)
    }

    /// Process a queued message and return the response text.
    ///
    /// This handles the actual message processing (typing indicator, text extraction,
    /// LLM call) for a single message, whether it's the initial message or a drained one.
    async fn process_queued_message(
        &self,
        queued: &QueuedMessage,
        handle: &SessionHandle,
    ) -> Option<String> {
        let routing = &queued.data.routing;
        let content = &queued.data.content;
        let sender = &queued.data.sender;
        let gateway = &queued.gateway;

        // Show typing indicator
        let _ = self
            .gateway_sender
            .send_typing(gateway, &routing.chat_id)
            .await;

        let should_prefix = is_group_chat(&routing.chat_type);

        match content {
            MessageContent::Text { text } => {
                let text = if should_prefix {
                    format!("{}: {}", resolve_sender_label(sender), text)
                } else {
                    text.clone()
                };
                self.process_text_message(gateway, &routing.chat_id, handle, &text, sender, routing)
                    .await
            }
            MessageContent::Media { caption, .. } => {
                if let Some(caption) = caption
                    && !caption.is_empty()
                {
                    let text = if should_prefix {
                        format!("{}: {}", resolve_sender_label(sender), caption)
                    } else {
                        caption.clone()
                    };
                    self.process_text_message(
                        gateway,
                        &routing.chat_id,
                        handle,
                        &text,
                        sender,
                        routing,
                    )
                    .await
                } else {
                    None
                }
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

    /// Run the drain loop after processing a message.
    ///
    /// Keeps draining the queue and processing/sending responses until the queue
    /// is empty. Responses from drained messages are sent directly via the gateway.
    async fn drain_loop(&self, handle: &SessionHandle, config: &QueueConfig) {
        let queue = self.session_queues.get(handle.id());

        loop {
            let drain_result = queue.drain(config).await;

            match drain_result {
                DrainResult::Idle => break,
                DrainResult::Batched(messages) => {
                    let combined = combine_messages(messages);
                    if let Some(response) = self.process_queued_message(&combined, handle).await {
                        let _ = self
                            .gateway_sender
                            .send_message(
                                &combined.gateway,
                                &combined.data.routing.chat_id,
                                &response,
                                None,
                            )
                            .await;
                    }
                }
                DrainResult::Sequential(msg) => {
                    if let Some(response) = self.process_queued_message(&msg, handle).await {
                        let _ = self
                            .gateway_sender
                            .send_message(&msg.gateway, &msg.data.routing.chat_id, &response, None)
                            .await;
                    }
                }
            }
        }
    }

    /// Process a text message and return the response.
    async fn process_text_message(
        &self,
        gateway: &str,
        chat_id: &str,
        handle: &SessionHandle,
        text: &str,
        sender: &Sender,
        routing: &RoutingContext,
    ) -> Option<String> {
        let agent = self.services.agents.get(handle.agent())?;

        // Persist user message via actor (with sender attribution for gateway messages)
        if let Err(e) = handle
            .add_user_message_with_sender(
                text.to_string(),
                Some(sender.id.clone()),
                Some(resolve_sender_label(sender)),
            )
            .await
        {
            error!(session_id = %handle.id(), error = %e, "Failed to persist user message");
            return None;
        }

        // Route to agentic loop if agent has tools configured
        if !agent.tools.is_empty() {
            return self
                .process_text_message_agentic(gateway, chat_id, handle, text, &sender.id, routing)
                .await;
        }

        // Simple single-turn for agents without tools
        let provider = self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())
            .await?;

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
        let mut builder = ContextBuilder::new()
            .from_agent_spec(&agent)
            .with_messages(history)
            .with_directives(directives);

        // Inject context buffer for mention-activated group chats
        if let Some(config) = self.should_inject_context_buffer(&agent, routing)
            && let Some(block) = self.build_context_buffer_block(handle, config).await
        {
            builder = builder.add_block(block);
        }

        let budget = TokenBudget {
            max_input_tokens: agent.model.effective_max_input_tokens(),
            max_output_tokens: agent.model.max_output_tokens.unwrap_or(4096),
            max_history_tokens: agent.session.context.max_history_tokens,
        };
        let chat_request = builder.build().render_with_budget(
            &agent.model.name,
            agent.model.temperature,
            agent.model.max_output_tokens,
            vec![],
            &budget,
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
        sender_id: &str,
        routing: &RoutingContext,
    ) -> Option<String> {
        let agent = self.services.agents.get(handle.agent())?;

        let provider = self
            .services
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())
            .await?;

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
            workspace_tools_dir: Some(self.services.workspace_tools_path.clone()),
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
        let mut builder = ContextBuilder::new()
            .from_agent_spec(&agent)
            .with_messages(history)
            .with_directives(directives);

        // Inject context buffer for mention-activated group chats
        if let Some(config) = self.should_inject_context_buffer(&agent, routing)
            && let Some(block) = self.build_context_buffer_block(handle, config).await
        {
            builder = builder.add_block(block);
        }

        let budget = TokenBudget {
            max_input_tokens: agent.model.effective_max_input_tokens(),
            max_output_tokens: agent.model.max_output_tokens.unwrap_or(4096),
            max_history_tokens: agent.session.context.max_history_tokens,
        };
        let built_context = builder.build();
        let tool_refs = built_context.tool_refs.clone();
        let messages = built_context
            .render_with_budget(
                &agent.model.name,
                agent.model.temperature,
                agent.model.max_output_tokens,
                vec![],
                &budget,
            )
            .messages;

        // Run agentic loop
        let result = match run_agentic_loop(
            provider,
            &mut executor,
            &agent,
            messages,
            handle,
            tool_refs.as_ref(),
        )
        .await
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
                // Persist final assistant message via actor
                if let Err(e) = handle.add_assistant_message(content.clone(), usage).await {
                    error!(error = %e, "Failed to persist assistant message");
                }
                Some(content)
            }
            AgenticResult::AwaitingApproval {
                mut pending,
                partial_content: _,
                usage: _,
                iterations: _,
                tool_calls_made: _,
            } => {
                // Tag with requester so only they can approve in group chats
                pending.requester_id = Some(sender_id.to_string());

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
                    .gateway_sender
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

// ============================================================================
// Helper Functions
// ============================================================================

/// Resolve the display label for a sender.
///
/// Priority: display_name > username > id
fn resolve_sender_label(sender: &Sender) -> String {
    if let Some(ref name) = sender.display_name
        && !name.is_empty()
    {
        return name.clone();
    }
    if let Some(ref username) = sender.username
        && !username.is_empty()
    {
        return username.clone();
    }
    sender.id.clone()
}

/// Extract text content from a message.
fn extract_text(content: &MessageContent) -> Option<&str> {
    match content {
        MessageContent::Text { text } => Some(text),
        MessageContent::Media { caption, .. } => caption.as_deref().filter(|c| !c.is_empty()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // resolve_sender_label
    // ------------------------------------------------------------------------

    #[test]
    fn sender_label_prefers_display_name() {
        let sender = Sender {
            id: "123".to_string(),
            username: Some("alice_bot".to_string()),
            display_name: Some("Alice".to_string()),
        };
        assert_eq!(resolve_sender_label(&sender), "Alice");
    }

    #[test]
    fn sender_label_falls_back_to_username() {
        let sender = Sender {
            id: "123".to_string(),
            username: Some("alice_bot".to_string()),
            display_name: None,
        };
        assert_eq!(resolve_sender_label(&sender), "alice_bot");
    }

    #[test]
    fn sender_label_falls_back_to_id() {
        let sender = Sender {
            id: "123".to_string(),
            username: None,
            display_name: None,
        };
        assert_eq!(resolve_sender_label(&sender), "123");
    }

    #[test]
    fn sender_label_skips_empty_display_name() {
        let sender = Sender {
            id: "123".to_string(),
            username: Some("alice_bot".to_string()),
            display_name: Some("".to_string()),
        };
        assert_eq!(resolve_sender_label(&sender), "alice_bot");
    }

    // ------------------------------------------------------------------------
    // extract_text
    // ------------------------------------------------------------------------

    #[test]
    fn extract_text_from_text_message() {
        let content = MessageContent::Text {
            text: "hello".to_string(),
        };
        assert_eq!(extract_text(&content), Some("hello"));
    }

    #[test]
    fn extract_text_from_media_with_caption() {
        let content = MessageContent::Media {
            media_type: "photo".to_string(),
            url: None,
            caption: Some("nice photo".to_string()),
        };
        assert_eq!(extract_text(&content), Some("nice photo"));
    }

    #[test]
    fn extract_text_from_media_without_caption() {
        let content = MessageContent::Media {
            media_type: "photo".to_string(),
            url: None,
            caption: None,
        };
        assert_eq!(extract_text(&content), None);
    }
}
