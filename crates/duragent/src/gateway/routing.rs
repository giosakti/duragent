//! Routing configuration and session resolution for gateway messages.

use tracing::{debug, warn};

use duragent_gateway_protocol::RoutingContext;

use crate::agent::QueueConfig;
use crate::config::{RoutingMatch, RoutingRule};
use crate::session::SessionHandle;

use super::handler::GatewayMessageHandler;

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
// Routing Methods
// ============================================================================

impl GatewayMessageHandler {
    /// Get or create a session for a gateway chat.
    ///
    /// Uses the shared `ChatSessionCache` to atomically look up or create sessions by
    /// (gateway, chat_id, agent) tuple. This enables long-lived sessions
    /// that persist across gateway messages and scheduled tasks.
    ///
    /// The atomic get-or-insert pattern prevents race conditions where two concurrent
    /// callers could both miss the cache and create duplicate sessions.
    pub(super) async fn get_or_create_session(
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
        let silent_buffer_cap = agent
            .access
            .as_ref()
            .map(|a| a.groups.context_buffer.max_messages)
            .unwrap_or(crate::session::DEFAULT_SILENT_BUFFER_CAP);
        let msg_limit =
            crate::session::actor_message_limit(agent.model.effective_max_input_tokens());
        let compaction_override = agent.session.compaction;

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
                                crate::session::CreateSessionOpts {
                                    on_disconnect,
                                    gateway: Some(gateway.clone()),
                                    gateway_chat_id: Some(chat_id.clone()),
                                    silent_buffer_cap,
                                    actor_message_limit: msg_limit,
                                    compaction_override,
                                },
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
    pub(super) fn resolve_agent(&self, gateway: &str, routing: &RoutingContext) -> Option<String> {
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

    /// Find an existing session for a gateway chat.
    ///
    /// Used by callback queries where we know the gateway and chat_id but not
    /// which agent was routed to. Tries each agent in the routing rules until
    /// we find a cached session.
    pub(super) async fn find_session_for_chat(
        &self,
        gateway: &str,
        chat_id: &str,
    ) -> Option<SessionHandle> {
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

    /// Resolve the queue config for a message based on chat type.
    ///
    /// Group chats use the agent's configured queue settings.
    /// DMs use default config (effectively a simple mutex).
    pub(super) fn resolve_queue_config(
        &self,
        handle: &SessionHandle,
        routing: &RoutingContext,
    ) -> QueueConfig {
        if is_group_chat(&routing.chat_type)
            && let Some(agent) = self.services.agents.get(handle.agent())
            && let Some(ref access) = agent.access
        {
            return access.groups.queue.clone();
        }
        QueueConfig::default()
    }
}

// ============================================================================
// Free Functions
// ============================================================================

/// Check if a routing rule matches the given context.
///
/// Gateway and chat_type comparisons are case-insensitive to prevent
/// "Telegram" vs "telegram" bugs. Chat ID and sender ID remain case-sensitive
/// as they are exact identifiers.
pub(super) fn matches_rule(
    conditions: &RoutingMatch,
    gateway: &str,
    routing: &RoutingContext,
) -> bool {
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

/// Check if a chat type represents a group conversation.
pub(super) fn is_group_chat(chat_type: &str) -> bool {
    matches!(
        chat_type.to_ascii_lowercase().as_str(),
        "group" | "supergroup" | "channel"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::session::ChatSessionCache;

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
        assert_eq!(key, "telegram\x0012345\x00my-agent");
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

    // ------------------------------------------------------------------------
    // is_group_chat
    // ------------------------------------------------------------------------

    #[test]
    fn is_group_chat_recognizes_group_types() {
        assert!(is_group_chat("group"));
        assert!(is_group_chat("supergroup"));
        assert!(is_group_chat("channel"));
        assert!(is_group_chat("Group"));
        assert!(is_group_chat("SUPERGROUP"));
    }

    #[test]
    fn is_group_chat_rejects_non_group_types() {
        assert!(!is_group_chat("dm"));
        assert!(!is_group_chat("private"));
        assert!(!is_group_chat("thread"));
    }
}
