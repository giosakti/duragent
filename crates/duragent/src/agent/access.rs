//! Access control logic for agent-level message filtering.
//!
//! Two public functions:
//! - `check_access`: top-level gate (should this message be accepted at all?)
//! - `resolve_sender_disposition`: per-sender control within allowed groups

use super::spec::{AccessConfig, DmPolicy, GroupAccessConfig, GroupPolicy, SenderDisposition};

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
