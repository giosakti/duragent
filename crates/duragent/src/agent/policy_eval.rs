//! Tool policy evaluation logic.
//!
//! Domain types live in `duragent-types`. This module provides the evaluation
//! methods via the `ToolPolicyEval` extension trait.

use crate::agent::{PolicyDecision, PolicyMode, ToolPolicy, ToolType};

/// Extension trait for policy evaluation on [`ToolPolicy`].
pub trait ToolPolicyEval {
    /// Check if a tool invocation is allowed by the policy.
    ///
    /// Deny list is always checked first (air-gap safety). If a command matches
    /// the deny list, it is denied regardless of mode or allow list.
    fn check(&self, tool_type: ToolType, invocation: &str) -> PolicyDecision;

    /// Check if a tool invocation should trigger a notification.
    fn should_notify(&self, tool_type: ToolType, invocation: &str) -> bool;

    /// Merge another policy into this one (other overrides self).
    ///
    /// Merge rules:
    /// - `mode`: Other overrides if not `Dangerous` (the default)
    /// - `deny/allow`: Union of both lists
    /// - `notify.enabled`: Other overrides
    /// - `notify.patterns`: Union of both lists
    /// - `notify.delivery`: Other overrides if not default
    #[must_use]
    fn merge(self, other: Self) -> Self;

    /// Add a pattern to the allow list.
    fn add_allow_pattern(&mut self, tool_type: ToolType, pattern: &str);
}

impl ToolPolicyEval for ToolPolicy {
    fn check(&self, tool_type: ToolType, invocation: &str) -> PolicyDecision {
        let tool_str = tool_type.as_str();

        // Deny list always takes precedence (air-gap safety)
        if !self.deny.is_empty() && matches_any_typed(&self.deny, tool_str, invocation) {
            return PolicyDecision::Deny;
        }

        match self.mode {
            PolicyMode::Dangerous => PolicyDecision::Allow,
            PolicyMode::Restrict => {
                if matches_any_typed(&self.allow, tool_str, invocation) {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Deny
                }
            }
            PolicyMode::Ask => {
                if matches_any_typed(&self.allow, tool_str, invocation) {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Ask
                }
            }
        }
    }

    fn should_notify(&self, tool_type: ToolType, invocation: &str) -> bool {
        let tool_str = tool_type.as_str();
        self.notify.enabled
            && (self.notify.patterns.is_empty()
                || matches_any_typed(&self.notify.patterns, tool_str, invocation))
    }

    fn merge(mut self, other: Self) -> Self {
        // Mode: other overrides if not default (Dangerous)
        if other.mode != PolicyMode::Dangerous {
            self.mode = other.mode;
        }

        // Lists: union
        self.deny.extend(other.deny);
        self.allow.extend(other.allow);

        // Notify: merge
        self.notify.enabled = other.notify.enabled || self.notify.enabled;
        self.notify.patterns.extend(other.notify.patterns);
        // Deliveries: extend with deduplication
        for delivery in other.notify.deliveries {
            if !self.notify.deliveries.contains(&delivery) {
                self.notify.deliveries.push(delivery);
            }
        }

        self
    }

    fn add_allow_pattern(&mut self, tool_type: ToolType, pattern: &str) {
        let full_pattern = format!("{}:{}", tool_type.as_str(), pattern);
        if !self.allow.contains(&full_pattern) {
            self.allow.push(full_pattern);
        }
    }
}

// ============================================================================
// Private helpers
// ============================================================================

/// Check if an invocation matches any pattern in the list.
fn matches_any_typed(patterns: &[String], tool_type: &str, invocation: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| matches_typed_pattern(pattern, tool_type, invocation))
}

/// Check if an invocation matches a single typed pattern.
///
/// Pattern format: `tool_type:pattern`
/// - Tool type can be `*` to match any tool type
/// - Pattern uses glob-style `*` matching
fn matches_typed_pattern(pattern: &str, tool_type: &str, invocation: &str) -> bool {
    let (pattern_tool_type, pattern_rest) = match pattern.split_once(':') {
        Some((t, p)) => (t, p),
        None => ("*", pattern),
    };

    if pattern_tool_type != "*" && !matches_pattern(pattern_tool_type, tool_type) {
        return false;
    }

    matches_pattern(pattern_rest, invocation)
}

/// Check if a string matches a single pattern.
///
/// Simple glob matching: `*` matches any sequence of characters.
/// Pattern parts must appear in order in the string.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        return text.contains(pattern);
    }

    let mut pos = 0;
    for part in &parts {
        if part.is_empty() {
            continue;
        }

        match text[pos..].find(part) {
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }

    true
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Delivery, NotifyConfig};

    #[test]
    fn check_dangerous_mode_allows_everything() {
        let policy = ToolPolicy::default();
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "echo hello"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "github:create_issue"),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn check_restrict_mode() {
        let policy = ToolPolicy {
            mode: PolicyMode::Restrict,
            allow: vec!["bash:echo*".to_string(), "bash:ls*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo hello"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "ls -la"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /"),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn check_ask_mode() {
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:echo*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo hello"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /"),
            PolicyDecision::Ask
        );
    }

    #[test]
    fn deny_list_takes_precedence_in_any_mode() {
        let policy = ToolPolicy {
            mode: PolicyMode::Restrict,
            allow: vec!["bash:*".to_string()],
            deny: vec!["bash:rm -rf*".to_string(), "*:*sudo*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo hello"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Bash, "sudo apt install"),
            PolicyDecision::Deny
        );

        let policy = ToolPolicy {
            mode: PolicyMode::Dangerous,
            deny: vec!["bash:rm -rf*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo hello"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /"),
            PolicyDecision::Deny
        );

        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:rm*".to_string()],
            deny: vec!["bash:rm -rf /*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "rm temp.txt"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "rm -rf /*"),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn wildcard_tool_type_matches_all() {
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            deny: vec!["*:*password*".to_string()],
            allow: vec!["*:read*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo password123"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "vault:get_password"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Bash, "cat file.txt"),
            PolicyDecision::Ask
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "filesystem:read_file"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Bash, "cat readme.txt"),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn mcp_tool_patterns() {
        let policy = ToolPolicy {
            mode: PolicyMode::Restrict,
            allow: vec![
                "mcp:github:*".to_string(),
                "mcp:filesystem:read*".to_string(),
            ],
            deny: vec!["mcp:*:delete*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Mcp, "github:create_issue"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "filesystem:read_file"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "filesystem:write_file"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "github:delete_repo"),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn pattern_without_colon_matches_any_tool() {
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            deny: vec!["*secret*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Bash, "echo secret"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "vault:get_secret"),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn pattern_matching_no_wildcard() {
        assert!(matches_pattern("echo", "echo hello"));
        assert!(matches_pattern("echo", "please echo this"));
        assert!(!matches_pattern("echo", "ECHO hello"));
    }

    #[test]
    fn pattern_matching_with_wildcards() {
        assert!(matches_pattern("echo*", "echo hello"));
        assert!(matches_pattern("*echo", "please echo"));
        assert!(matches_pattern("*echo*", "please echo this"));
        assert!(matches_pattern("npm*install", "npm install lodash"));
        assert!(matches_pattern("npm*install", "npm ci install"));
        assert!(!matches_pattern("npm*install", "yarn install"));
    }

    #[test]
    fn pattern_matching_edge_cases() {
        assert!(matches_pattern("*", "anything"));
        assert!(matches_pattern("**", "anything"));
        assert!(matches_pattern("a*b*c", "abc"));
        assert!(matches_pattern("a*b*c", "aXXbYYc"));
        assert!(!matches_pattern("a*b*c", "acb"));
    }

    #[test]
    fn should_notify_respects_enabled() {
        let policy = ToolPolicy {
            notify: NotifyConfig {
                enabled: false,
                patterns: vec!["*:*".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!policy.should_notify(ToolType::Bash, "any command"));

        let policy = ToolPolicy {
            notify: NotifyConfig {
                enabled: true,
                patterns: vec![],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(policy.should_notify(ToolType::Bash, "any command"));
    }

    #[test]
    fn should_notify_respects_patterns() {
        let policy = ToolPolicy {
            notify: NotifyConfig {
                enabled: true,
                patterns: vec!["bash:rm*".to_string(), "*:*sudo*".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(policy.should_notify(ToolType::Bash, "rm -rf /"));
        assert!(policy.should_notify(ToolType::Bash, "sudo apt install"));
        assert!(!policy.should_notify(ToolType::Bash, "echo hello"));
    }

    #[test]
    fn merge_mode_override() {
        let base = ToolPolicy {
            mode: PolicyMode::Restrict,
            ..Default::default()
        };
        let local = ToolPolicy {
            mode: PolicyMode::Ask,
            ..Default::default()
        };

        let merged = base.merge(local);
        assert_eq!(merged.mode, PolicyMode::Ask);
    }

    #[test]
    fn merge_mode_preserves_base_if_local_is_dangerous() {
        let base = ToolPolicy {
            mode: PolicyMode::Restrict,
            ..Default::default()
        };
        let local = ToolPolicy::default(); // Dangerous

        let merged = base.merge(local);
        assert_eq!(merged.mode, PolicyMode::Restrict);
    }

    #[test]
    fn merge_lists_union() {
        let base = ToolPolicy {
            allow: vec!["bash:echo*".to_string()],
            deny: vec!["bash:rm*".to_string()],
            ..Default::default()
        };
        let local = ToolPolicy {
            allow: vec!["bash:ls*".to_string()],
            deny: vec!["*:*sudo*".to_string()],
            ..Default::default()
        };

        let merged = base.merge(local);
        assert_eq!(merged.allow, vec!["bash:echo*", "bash:ls*"]);
        assert_eq!(merged.deny, vec!["bash:rm*", "*:*sudo*"]);
    }

    #[test]
    fn merge_deliveries_deduplicates() {
        let base = ToolPolicy {
            notify: NotifyConfig {
                enabled: true,
                deliveries: vec![
                    Delivery::Log,
                    Delivery::Webhook {
                        url: "https://example.com/hook".to_string(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let local = ToolPolicy {
            notify: NotifyConfig {
                enabled: true,
                deliveries: vec![
                    Delivery::Log, // Duplicate
                    Delivery::Webhook {
                        url: "https://example.com/hook".to_string(), // Duplicate
                    },
                    Delivery::Webhook {
                        url: "https://other.com/hook".to_string(), // New
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = base.merge(local);
        assert_eq!(merged.notify.deliveries.len(), 3);
        assert!(merged.notify.deliveries.contains(&Delivery::Log));
        assert!(merged.notify.deliveries.contains(&Delivery::Webhook {
            url: "https://example.com/hook".to_string()
        }));
        assert!(merged.notify.deliveries.contains(&Delivery::Webhook {
            url: "https://other.com/hook".to_string()
        }));
    }

    #[test]
    fn add_allow_pattern_with_tool_type() {
        let mut policy = ToolPolicy {
            allow: vec!["bash:echo*".to_string()],
            ..Default::default()
        };

        policy.add_allow_pattern(ToolType::Bash, "echo*"); // Duplicate
        policy.add_allow_pattern(ToolType::Bash, "ls*");
        policy.add_allow_pattern(ToolType::Mcp, "github:*");

        assert_eq!(policy.allow.len(), 3);
        assert!(policy.allow.contains(&"bash:echo*".to_string()));
        assert!(policy.allow.contains(&"bash:ls*".to_string()));
        assert!(policy.allow.contains(&"mcp:github:*".to_string()));
    }

    #[test]
    fn cli_tool_type_pattern_matching() {
        let policy = ToolPolicy {
            mode: PolicyMode::Restrict,
            allow: vec!["cli:code-search".to_string(), "cli:deploy*".to_string()],
            deny: vec!["cli:*dangerous*".to_string()],
            ..Default::default()
        };

        assert_eq!(
            policy.check(ToolType::Cli, "code-search"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Cli, "deploy-prod"),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.check(ToolType::Cli, "unknown-tool"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Cli, "dangerous-tool"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Builtin, "code-search"),
            PolicyDecision::Deny
        );
    }
}
