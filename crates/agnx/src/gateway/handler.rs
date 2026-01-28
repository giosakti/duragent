//! Gateway message handler that routes messages to sessions.
//!
//! This handler bridges incoming gateway messages to the session system,
//! processing them through the LLM and returning responses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

use agnx_gateway_protocol::{MessageContent, RoutingContext};

use super::MessageHandler;
use crate::agent::AgentStore;
use crate::api::SessionStatus;
use crate::config::{RoutingMatch, RoutingRule};
use crate::llm::{Message, ProviderRegistry, Role};
use crate::session::{
    SessionContext, SessionEventPayload, SessionStore, build_chat_request, build_system_message,
    commit_event, load_snapshot, persist_assistant_message, record_event,
};

// ============================================================================
// Gateway Message Handler
// ============================================================================

/// Handler that routes gateway messages to sessions.
///
/// Maps gateway chats to sessions using a compound key of `{gateway}:{chat_id}`.
/// Creates new sessions for unknown chats using the configured default agent.
pub struct GatewayMessageHandler {
    agents: AgentStore,
    providers: ProviderRegistry,
    sessions: SessionStore,
    sessions_path: PathBuf,
    /// Mapping from gateway chat key to session ID.
    chat_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Routing configuration for agent selection.
    routing_config: RoutingConfig,
}

impl GatewayMessageHandler {
    /// Create a new gateway message handler.
    pub fn new(
        agents: AgentStore,
        providers: ProviderRegistry,
        sessions: SessionStore,
        sessions_path: PathBuf,
        routing_config: RoutingConfig,
    ) -> Self {
        Self {
            agents,
            providers,
            sessions,
            sessions_path,
            chat_sessions: Arc::new(RwLock::new(HashMap::new())),
            routing_config,
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
        let agent_name = self.resolve_agent(routing)?;
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
    fn resolve_agent(&self, routing: &RoutingContext) -> Option<String> {
        // Check routing rules in order
        for rule in &self.routing_config.rules {
            if matches_rule(&rule.match_conditions, routing) {
                if self.agents.get(&rule.agent).is_some() {
                    return Some(rule.agent.clone());
                }
                warn!(agent = %rule.agent, "Routing rule agent not found");
            }
        }

        // Fall back to default agent
        if let Some(ref default) = self.routing_config.default_agent {
            if self.agents.get(default).is_some() {
                return Some(default.clone());
            }
            warn!(agent = %default, "Configured default agent not found");
        }

        // No agent configured - fail closed
        warn!(
            channel = %routing.channel,
            chat_id = %routing.chat_id,
            "No agent configured for gateway routing, message dropped"
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
    async fn process_text_message(&self, session_id: &str, text: &str) -> Option<String> {
        let session = self.sessions.get(session_id).await?;
        let agent = self.agents.get(&session.agent)?;

        // Add user message to session
        let user_message = Message {
            role: Role::User,
            content: text.to_string(),
        };
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
        )
        .await
        {
            error!(error = %e, "Failed to persist user message event");
        }

        // Get provider
        let provider = self
            .providers
            .get(&agent.model.provider, agent.model.base_url.as_deref())?;

        // Build chat request
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

        // Call LLM
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
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Persist assistant message
        if let Err(e) = persist_assistant_message(
            &self.sessions,
            &self.sessions_path,
            session_id,
            assistant_content.clone(),
            response.usage,
        )
        .await
        {
            error!(error = %e, "Failed to persist assistant message");
        }

        Some(assistant_content)
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
            MessageContent::Text { text } => self.process_text_message(&session_id, text).await,
            MessageContent::Media { caption, .. } => {
                // For media, process the caption if present
                if let Some(caption) = caption
                    && !caption.is_empty()
                {
                    return self.process_text_message(&session_id, caption).await;
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
}

// ============================================================================
// Routing Config
// ============================================================================

/// Routing configuration for agent selection.
#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// Default agent to use when no rule matches.
    pub default_agent: Option<String>,
    /// Routing rules (evaluated in order, first match wins).
    pub rules: Vec<RoutingRule>,
}

impl RoutingConfig {
    /// Create a simple config with just a default agent.
    pub fn with_default(agent: Option<String>) -> Self {
        Self {
            default_agent: agent,
            rules: Vec::new(),
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a routing rule matches the given context.
fn matches_rule(conditions: &RoutingMatch, routing: &RoutingContext) -> bool {
    // All specified conditions must match (AND logic)
    if let Some(ref chat_type) = conditions.chat_type
        && chat_type != &routing.chat_type
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
        assert!(matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_chat_type_match() {
        let conditions = RoutingMatch {
            chat_type: Some("dm".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_chat_type_no_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_chat_id_match() {
        let conditions = RoutingMatch {
            chat_id: Some("123".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_chat_id_no_match() {
        let conditions = RoutingMatch {
            chat_id: Some("999".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_sender_id_match() {
        let conditions = RoutingMatch {
            sender_id: Some("456".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_sender_id_no_match() {
        let conditions = RoutingMatch {
            sender_id: Some("999".to_string()),
            ..Default::default()
        };
        let routing = make_routing_context("dm", "123", "456");
        assert!(!matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_multiple_conditions_all_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            chat_id: Some("-100123".to_string()),
            sender_id: Some("456".to_string()),
        };
        let routing = make_routing_context("group", "-100123", "456");
        assert!(matches_rule(&conditions, &routing));
    }

    #[test]
    fn test_matches_rule_multiple_conditions_partial_match() {
        let conditions = RoutingMatch {
            chat_type: Some("group".to_string()),
            chat_id: Some("-100123".to_string()),
            sender_id: Some("456".to_string()),
        };
        // chat_type matches, but sender_id doesn't
        let routing = make_routing_context("group", "-100123", "789");
        assert!(!matches_rule(&conditions, &routing));
    }
}
