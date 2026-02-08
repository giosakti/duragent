//! Access control logic for agent-level message filtering.
//!
//! Two public functions:
//! - `check_access`: top-level gate (should this message be accepted at all?)
//! - `resolve_sender_disposition`: per-sender control within allowed groups

use super::spec::{AccessConfig, DmPolicy, GroupAccessConfig, GroupPolicy, SenderDisposition};

/// Check if the message should be accepted based on chat type and access policy.
///
/// Returns `true` if the message passes the top-level gate.
pub fn check_access(
    access: &AccessConfig,
    chat_type: &str,
    chat_id: &str,
    sender_id: &str,
) -> bool {
    match chat_type.to_ascii_lowercase().as_str() {
        "dm" | "private" => match access.dm.policy {
            DmPolicy::Open => true,
            DmPolicy::Disabled => false,
            DmPolicy::Allowlist => access.dm.allowlist.iter().any(|id| id == sender_id),
        },
        "group" | "supergroup" | "channel" => match access.groups.policy {
            GroupPolicy::Open => true,
            GroupPolicy::Disabled => false,
            GroupPolicy::Allowlist => access.groups.allowlist.iter().any(|id| id == chat_id),
        },
        // Unknown chat types: allow (forward compatibility)
        _ => true,
    }
}

/// Resolve the disposition for a sender within an allowed group.
///
/// Checks `sender_overrides` first, falls back to `sender_default`.
pub fn resolve_sender_disposition(
    groups: &GroupAccessConfig,
    sender_id: &str,
) -> SenderDisposition {
    groups
        .sender_overrides
        .get(sender_id)
        .copied()
        .unwrap_or(groups.sender_default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::spec::{DmAccessConfig, GroupAccessConfig};

    fn default_access() -> AccessConfig {
        AccessConfig::default()
    }

    // ========================================================================
    // check_access — DM policies
    // ========================================================================

    #[test]
    fn dm_open_allows_any_sender() {
        let access = default_access();
        assert!(check_access(&access, "dm", "chat1", "sender1"));
        assert!(check_access(&access, "private", "chat1", "sender1"));
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
        assert!(!check_access(&access, "dm", "chat1", "sender1"));
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
        assert!(check_access(&access, "dm", "chat1", "allowed_user"));
        assert!(!check_access(&access, "dm", "chat1", "other_user"));
    }

    // ========================================================================
    // check_access — Group policies
    // ========================================================================

    #[test]
    fn group_open_allows_any_group() {
        let access = default_access();
        assert!(check_access(&access, "group", "-100123", "sender1"));
        assert!(check_access(&access, "supergroup", "-100123", "sender1"));
        assert!(check_access(&access, "channel", "-100123", "sender1"));
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
        assert!(!check_access(&access, "group", "-100123", "sender1"));
    }

    #[test]
    fn group_allowlist_checks_chat_id() {
        let access = AccessConfig {
            groups: GroupAccessConfig {
                policy: GroupPolicy::Allowlist,
                allowlist: vec!["-100123".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(check_access(&access, "group", "-100123", "sender1"));
        assert!(!check_access(&access, "group", "-100999", "sender1"));
    }

    // ========================================================================
    // check_access — chat_type case insensitivity
    // ========================================================================

    #[test]
    fn chat_type_case_insensitive() {
        let access = default_access();
        assert!(check_access(&access, "DM", "chat1", "sender1"));
        assert!(check_access(&access, "Group", "-100123", "sender1"));
        assert!(check_access(&access, "PRIVATE", "chat1", "sender1"));
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
        assert!(check_access(&access, "thread", "chat1", "sender1"));
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
