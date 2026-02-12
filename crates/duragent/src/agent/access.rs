//! Access control types and logic for agent-level message filtering.
//!
//! Types: `AccessConfig`, `DmAccessConfig`, `GroupAccessConfig`, queue/activation config.
//!
//! Two public functions:
//! - `check_access`: top-level gate (should this message be accepted at all?)
//! - `resolve_sender_disposition`: per-sender control within allowed groups

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// Access Control Types
// ============================================================================

/// Access control configuration for an agent.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AccessConfig {
    #[serde(default)]
    pub dm: DmAccessConfig,
    #[serde(default)]
    pub groups: GroupAccessConfig,
}

/// DM access policy configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DmAccessConfig {
    #[serde(default)]
    pub policy: DmPolicy,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

impl Default for DmAccessConfig {
    fn default() -> Self {
        Self {
            policy: DmPolicy::Open,
            allowlist: Vec::new(),
        }
    }
}

/// DM access policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Accept DMs from anyone (default).
    #[default]
    Open,
    /// Reject all DMs.
    Disabled,
    /// Only accept DMs from listed sender IDs.
    Allowlist,
}

/// Group access policy configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupAccessConfig {
    #[serde(default)]
    pub policy: GroupPolicy,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub sender_default: SenderDisposition,
    #[serde(default)]
    pub sender_overrides: HashMap<String, SenderDisposition>,
    /// How the agent is activated in groups.
    #[serde(default)]
    pub activation: ActivationMode,
    /// Configuration for the context buffer (silent messages injected on trigger).
    #[serde(default)]
    pub context_buffer: ContextBufferConfig,
    /// Queue configuration for handling concurrent messages.
    #[serde(default)]
    pub queue: QueueConfig,
}

impl Default for GroupAccessConfig {
    fn default() -> Self {
        Self {
            policy: GroupPolicy::Open,
            allowlist: Vec::new(),
            sender_default: SenderDisposition::Allow,
            sender_overrides: HashMap::new(),
            activation: ActivationMode::default(),
            context_buffer: ContextBufferConfig::default(),
            queue: QueueConfig::default(),
        }
    }
}

/// Group access policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Accept messages from any group (default).
    #[default]
    Open,
    /// Reject all group messages.
    Disabled,
    /// Only accept messages from listed group IDs.
    Allowlist,
}

/// Disposition for a sender within an allowed group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderDisposition {
    /// Message is visible to the LLM and triggers a response.
    #[default]
    Allow,
    /// Message is stored as a UserMessage (LLM sees it in future turns) but does not trigger a response.
    Passive,
    /// Message is stored in session history for audit but excluded from LLM conversation.
    Silent,
    /// Message is discarded entirely.
    Block,
}

/// Activation mode for group messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// Only respond when @mentioned or replied to (default).
    #[default]
    Mention,
    /// Respond to every allowed message.
    Always,
}

// ============================================================================
// Context Buffer Types
// ============================================================================

/// Configuration for the context buffer (messages from non-triggering senders).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextBufferConfig {
    /// How non-triggering messages are stored.
    #[serde(default)]
    pub mode: ContextBufferMode,
    /// Maximum number of recent silent messages to inject as context.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    /// Maximum age in hours for context buffer messages.
    #[serde(default = "default_max_age_hours")]
    pub max_age_hours: u64,
}

impl Default for ContextBufferConfig {
    fn default() -> Self {
        Self {
            mode: ContextBufferMode::default(),
            max_messages: default_max_messages(),
            max_age_hours: default_max_age_hours(),
        }
    }
}

fn default_max_messages() -> usize {
    100
}

fn default_max_age_hours() -> u64 {
    24
}

/// How non-triggering messages from `Allow` senders are stored in mention mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBufferMode {
    /// Ephemeral buffer: stored as SilentMessage, injected as a system block on trigger.
    #[default]
    Silent,
    /// Durable: stored as UserMessage in conversation history (no injection needed).
    Passive,
}

// ============================================================================
// Queue Types
// ============================================================================

/// Queue configuration for group message handling.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueueConfig {
    /// How pending messages are processed when the session becomes idle.
    #[serde(default)]
    pub mode: QueueMode,
    /// Maximum number of pending messages in the queue.
    #[serde(default = "default_max_pending")]
    pub max_pending: usize,
    /// What happens when the queue is full.
    #[serde(default)]
    pub overflow: OverflowStrategy,
    /// Message sent back to the user when a message is rejected.
    #[serde(default)]
    pub reject_message: Option<String>,
    /// Debounce settings for batching rapid messages.
    #[serde(default)]
    pub debounce: DebounceConfig,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            mode: QueueMode::default(),
            max_pending: default_max_pending(),
            overflow: OverflowStrategy::default(),
            reject_message: None,
            debounce: DebounceConfig::default(),
        }
    }
}

fn default_max_pending() -> usize {
    10
}

/// Queue mode for group message handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// Batch all pending messages into one combined message (default).
    #[default]
    Batch,
    /// Process pending messages one at a time in order.
    Sequential,
    /// Drop all pending messages when session becomes idle.
    Drop,
}

/// Overflow strategy when the queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowStrategy {
    /// Drop oldest pending messages to make room (default).
    #[default]
    DropOld,
    /// Reject the new message (queue stays unchanged).
    DropNew,
    /// Reject with a user-visible message.
    Reject,
}

/// Debounce configuration for batching rapid messages from the same sender.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DebounceConfig {
    /// Whether debouncing is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Idle window in milliseconds before flushing the debounce buffer.
    #[serde(default = "default_debounce_window_ms")]
    pub window_ms: u64,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            window_ms: default_debounce_window_ms(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_debounce_window_ms() -> u64 {
    1500
}

// ============================================================================
// Access Check Logic
// ============================================================================

/// Check if the message should be accepted based on chat type and access policy.
///
/// Returns `true` if the message passes the top-level gate.
///
/// Group allowlists match against `{channel}:{chat_id}` composite keys
/// (e.g. `telegram:-100123`). DM allowlists match against bare `sender_id`.
/// Both support trailing `*` wildcards.
pub fn check_access(
    access: &AccessConfig,
    chat_type: &str,
    channel: &str,
    chat_id: &str,
    sender_id: &str,
) -> bool {
    match chat_type.to_ascii_lowercase().as_str() {
        "dm" | "private" => match access.dm.policy {
            DmPolicy::Open => true,
            DmPolicy::Disabled => false,
            DmPolicy::Allowlist => access
                .dm
                .allowlist
                .iter()
                .any(|p| matches_pattern(p, sender_id)),
        },
        "group" | "supergroup" | "channel" => match access.groups.policy {
            GroupPolicy::Open => true,
            GroupPolicy::Disabled => false,
            GroupPolicy::Allowlist => {
                let composite = format!("{channel}:{chat_id}");
                access
                    .groups
                    .allowlist
                    .iter()
                    .any(|p| matches_pattern(p, &composite))
            }
        },
        // Unknown chat types: allow (forward compatibility)
        _ => true,
    }
}

/// Resolve the disposition for a sender within an allowed group.
///
/// Resolution order: exact match in `sender_overrides`, then longest
/// matching wildcard pattern, then `sender_default`.
pub fn resolve_sender_disposition(
    groups: &GroupAccessConfig,
    sender_id: &str,
) -> SenderDisposition {
    // Exact match first
    if let Some(&d) = groups.sender_overrides.get(sender_id) {
        return d;
    }
    // Wildcard pattern match — longest prefix wins for deterministic ordering
    let mut best: Option<(usize, SenderDisposition)> = None;
    for (pattern, &d) in &groups.sender_overrides {
        if pattern.contains('*') && matches_pattern(pattern, sender_id) {
            let len = pattern.len();
            if best.is_none() || len > best.unwrap().0 {
                best = Some((len, d));
            }
        }
    }
    if let Some((_, d)) = best {
        return d;
    }
    groups.sender_default
}

// ============================================================================
// Private helpers
// ============================================================================

/// Match a pattern with optional trailing `*` wildcard against a value.
///
/// `telegram:*` matches any string starting with `telegram:`.
/// `*` alone matches everything. Exact strings require exact match.
fn matches_pattern(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_access() -> AccessConfig {
        AccessConfig::default()
    }

    // ========================================================================
    // check_access — DM policies
    // ========================================================================

    #[test]
    fn dm_open_allows_any_sender() {
        let access = default_access();
        assert!(check_access(&access, "dm", "telegram", "chat1", "sender1"));
        assert!(check_access(
            &access, "private", "telegram", "chat1", "sender1"
        ));
    }

    #[test]
    fn dm_disabled_blocks_all() {
        let access = AccessConfig {
            dm: DmAccessConfig {
                policy: DmPolicy::Disabled,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!check_access(&access, "dm", "telegram", "chat1", "sender1"));
    }

    #[test]
    fn dm_allowlist_checks_sender_id() {
        let access = AccessConfig {
            dm: DmAccessConfig {
                policy: DmPolicy::Allowlist,
                allowlist: vec!["allowed_user".to_string()],
            },
            ..Default::default()
        };
        assert!(check_access(
            &access,
            "dm",
            "telegram",
            "chat1",
            "allowed_user"
        ));
        assert!(!check_access(
            &access,
            "dm",
            "telegram",
            "chat1",
            "other_user"
        ));
    }

    #[test]
    fn dm_allowlist_wildcard_matches() {
        let access = AccessConfig {
            dm: DmAccessConfig {
                policy: DmPolicy::Allowlist,
                allowlist: vec!["user_*".to_string()],
            },
            ..Default::default()
        };
        assert!(check_access(&access, "dm", "telegram", "chat1", "user_123"));
        assert!(check_access(&access, "dm", "telegram", "chat1", "user_abc"));
        assert!(!check_access(&access, "dm", "telegram", "chat1", "admin_1"));
    }

    // ========================================================================
    // check_access — Group policies
    // ========================================================================

    #[test]
    fn group_open_allows_any_group() {
        let access = default_access();
        assert!(check_access(
            &access, "group", "telegram", "-100123", "sender1"
        ));
        assert!(check_access(
            &access,
            "supergroup",
            "telegram",
            "-100123",
            "sender1"
        ));
        assert!(check_access(
            &access, "channel", "telegram", "-100123", "sender1"
        ));
    }

    #[test]
    fn group_disabled_blocks_all() {
        let access = AccessConfig {
            groups: GroupAccessConfig {
                policy: GroupPolicy::Disabled,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!check_access(
            &access, "group", "telegram", "-100123", "sender1"
        ));
    }

    #[test]
    fn group_allowlist_checks_composite_key() {
        let access = AccessConfig {
            groups: GroupAccessConfig {
                policy: GroupPolicy::Allowlist,
                allowlist: vec!["telegram:-100123".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(check_access(
            &access, "group", "telegram", "-100123", "sender1"
        ));
        assert!(!check_access(
            &access, "group", "telegram", "-100999", "sender1"
        ));
        assert!(!check_access(
            &access, "group", "discord", "-100123", "sender1"
        ));
    }

    #[test]
    fn group_allowlist_wildcard_matches() {
        let access = AccessConfig {
            groups: GroupAccessConfig {
                policy: GroupPolicy::Allowlist,
                allowlist: vec!["telegram:*".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(check_access(
            &access, "group", "telegram", "-100123", "sender1"
        ));
        assert!(check_access(
            &access, "group", "telegram", "-100999", "sender1"
        ));
        assert!(!check_access(
            &access, "group", "discord", "12345", "sender1"
        ));
    }

    #[test]
    fn wildcard_star_alone_matches_everything() {
        let access = AccessConfig {
            groups: GroupAccessConfig {
                policy: GroupPolicy::Allowlist,
                allowlist: vec!["*".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(check_access(
            &access, "group", "telegram", "-100123", "sender1"
        ));
        assert!(check_access(
            &access, "group", "discord", "12345", "sender1"
        ));
    }

    // ========================================================================
    // check_access — chat_type case insensitivity
    // ========================================================================

    #[test]
    fn chat_type_case_insensitive() {
        let access = default_access();
        assert!(check_access(&access, "DM", "telegram", "chat1", "sender1"));
        assert!(check_access(
            &access, "Group", "telegram", "-100123", "sender1"
        ));
        assert!(check_access(
            &access, "PRIVATE", "telegram", "chat1", "sender1"
        ));
    }

    // ========================================================================
    // check_access — unknown chat types (forward compat)
    // ========================================================================

    #[test]
    fn unknown_chat_type_allowed() {
        let access = AccessConfig {
            dm: DmAccessConfig {
                policy: DmPolicy::Disabled,
                ..Default::default()
            },
            groups: GroupAccessConfig {
                policy: GroupPolicy::Disabled,
                ..Default::default()
            },
        };
        // Even with everything disabled, unknown types pass through
        assert!(check_access(
            &access, "thread", "telegram", "chat1", "sender1"
        ));
    }

    // ========================================================================
    // resolve_sender_disposition
    // ========================================================================

    #[test]
    fn sender_default_allow() {
        let groups = GroupAccessConfig::default();
        assert_eq!(
            resolve_sender_disposition(&groups, "anyone"),
            SenderDisposition::Allow
        );
    }

    #[test]
    fn sender_override_takes_priority_over_default() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("vip".to_string(), SenderDisposition::Allow);
        overrides.insert("spammer".to_string(), SenderDisposition::Block);

        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Silent,
            sender_overrides: overrides,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "vip"),
            SenderDisposition::Allow
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "spammer"),
            SenderDisposition::Block
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "someone_else"),
            SenderDisposition::Silent
        );
    }

    #[test]
    fn sender_override_passive() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("observer".to_string(), SenderDisposition::Passive);

        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Allow,
            sender_overrides: overrides,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "observer"),
            SenderDisposition::Passive
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "anyone_else"),
            SenderDisposition::Allow
        );
    }

    #[test]
    fn sender_override_wildcard_matches() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("admin_*".to_string(), SenderDisposition::Allow);

        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Silent,
            sender_overrides: overrides,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "admin_bob"),
            SenderDisposition::Allow
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "admin_alice"),
            SenderDisposition::Allow
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "user_bob"),
            SenderDisposition::Silent
        );
    }

    #[test]
    fn sender_override_longest_wildcard_wins() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("a*".to_string(), SenderDisposition::Block);
        overrides.insert("admin_*".to_string(), SenderDisposition::Allow);

        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Silent,
            sender_overrides: overrides,
            ..Default::default()
        };
        // "admin_*" (len 7) beats "a*" (len 2)
        assert_eq!(
            resolve_sender_disposition(&groups, "admin_bob"),
            SenderDisposition::Allow
        );
        // Only "a*" matches
        assert_eq!(
            resolve_sender_disposition(&groups, "abc"),
            SenderDisposition::Block
        );
    }

    #[test]
    fn sender_override_exact_takes_priority_over_wildcard() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("admin_*".to_string(), SenderDisposition::Allow);
        overrides.insert("admin_blocked".to_string(), SenderDisposition::Block);

        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Silent,
            sender_overrides: overrides,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "admin_blocked"),
            SenderDisposition::Block
        );
        assert_eq!(
            resolve_sender_disposition(&groups, "admin_other"),
            SenderDisposition::Allow
        );
    }

    #[test]
    fn sender_default_passive() {
        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Passive,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "someone"),
            SenderDisposition::Passive
        );
    }

    #[test]
    fn sender_default_silent() {
        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Silent,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "someone"),
            SenderDisposition::Silent
        );
    }

    #[test]
    fn sender_default_block() {
        let groups = GroupAccessConfig {
            sender_default: SenderDisposition::Block,
            ..Default::default()
        };
        assert_eq!(
            resolve_sender_disposition(&groups, "someone"),
            SenderDisposition::Block
        );
    }
}
