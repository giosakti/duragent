//! Tool policy configuration for command filtering and approval.
//!
//! Policies control which tools agents can execute:
//! - `mode: dangerous` — Trust everything (only deny list blocks)
//! - `mode: ask` — Allow list runs, everything else requires approval
//! - `mode: restrict` — Only allow list runs, everything else denied
//!
//! **Important:** The deny list is always checked first, regardless of mode.
//! This provides an "air-gap" safety mechanism — commands matching the deny
//! list are blocked even if they match the allow list or mode is `dangerous`.
//!
//! ## Pattern Format
//!
//! Patterns use the format `tool_type:pattern` where:
//! - `bash:cargo *` — bash command starting with cargo
//! - `mcp:github:*` — any tool from github MCP server
//! - `mcp:*:read*` — any MCP tool with "read" in name
//! - `*:*secret*` — block "secret" in any tool
//!
//! If no tool type prefix is provided, patterns match against all tool types.

use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, warn};

// ============================================================================
// Concurrency
// ============================================================================

/// Per-agent locks for policy file writes.
///
/// Prevents concurrent writes from overwriting each other.
/// Different agents can write concurrently without contention.
pub type PolicyLocks = Arc<DashMap<String, Arc<Mutex<()>>>>;

// ============================================================================
// Constants
// ============================================================================

/// Current policy API version.
pub const POLICY_API_VERSION: &str = "agnx/v1alpha1";

/// Policy kind identifier.
pub const POLICY_KIND: &str = "Policy";

// ============================================================================
// Public Types
// ============================================================================

/// Tool execution policy.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicy {
    /// API version for the policy format.
    #[serde(default = "default_api_version")]
    pub api_version: String,

    /// Kind identifier (always "Policy").
    #[serde(default = "default_kind")]
    pub kind: String,

    /// Policy mode for command filtering.
    #[serde(default)]
    pub mode: PolicyMode,

    /// Patterns to deny (air-gap, checked first in all modes).
    /// Format: `tool_type:pattern` (e.g., `bash:rm -rf *`, `mcp:*:delete*`)
    #[serde(default)]
    pub deny: Vec<String>,

    /// Patterns to allow.
    /// Format: `tool_type:pattern` (e.g., `bash:cargo *`, `mcp:github:*`)
    #[serde(default)]
    pub allow: Vec<String>,

    /// Notification configuration.
    #[serde(default)]
    pub notify: NotifyConfig,
}

fn default_api_version() -> String {
    POLICY_API_VERSION.to_string()
}

fn default_kind() -> String {
    POLICY_KIND.to_string()
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            api_version: POLICY_API_VERSION.to_string(),
            kind: POLICY_KIND.to_string(),
            mode: PolicyMode::default(),
            deny: Vec::new(),
            allow: Vec::new(),
            notify: NotifyConfig::default(),
        }
    }
}

/// Policy mode for command filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    /// Trust everything — only deny list blocks (default for backwards compatibility).
    #[default]
    Dangerous,
    /// Allow list runs without asking, everything else requires human approval.
    Ask,
    /// Only allow list runs, everything else is denied.
    Restrict,
}

/// Notification configuration for command execution.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NotifyConfig {
    /// Whether notifications are enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Patterns of commands to notify about.
    /// Format: `tool_type:pattern`
    #[serde(default)]
    pub patterns: Vec<String>,

    /// Delivery configurations (multiple destinations supported).
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

/// A single notification delivery target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Delivery {
    /// Log to tracing.
    Log,
    /// Send to webhook URL.
    Webhook {
        /// The webhook URL to POST to.
        url: String,
    },
}

/// Decision from policy check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Command is allowed.
    Allow,
    /// Command is denied by policy.
    Deny,
    /// Command requires human approval.
    Ask,
}

/// Tool type for pattern matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolType {
    /// Bash/shell commands.
    Bash,
    /// MCP server tools.
    Mcp,
    /// Built-in tools.
    Builtin,
}

impl ToolType {
    /// Get the string representation for pattern matching.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolType::Bash => "bash",
            ToolType::Mcp => "mcp",
            ToolType::Builtin => "builtin",
        }
    }
}

// ============================================================================
// Implementation
// ============================================================================

impl ToolPolicy {
    /// Load policy from agent directory.
    ///
    /// Loads `policy.yaml` (base) and merges with `policy.local.yaml` (overrides).
    pub async fn load(agent_dir: &Path) -> Self {
        let base = Self::load_file(&agent_dir.join("policy.yaml")).await;
        let local = Self::load_file(&agent_dir.join("policy.local.yaml")).await;

        match (base, local) {
            (Some(base), Some(local)) => base.merge(local),
            (Some(base), None) => base,
            (None, Some(local)) => local,
            (None, None) => Self::default(),
        }
    }

    /// Load a single policy file.
    async fn load_file(path: &Path) -> Option<Self> {
        let content = fs::read_to_string(path).await.ok()?;
        match serde_saphyr::from_str(&content) {
            Ok(policy) => Some(policy),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to parse policy file");
                None
            }
        }
    }

    /// Merge another policy into this one (other overrides self).
    ///
    /// Merge rules:
    /// - `mode`: Other overrides if not `Dangerous` (the default)
    /// - `deny/allow`: Union of both lists
    /// - `notify.enabled`: Other overrides
    /// - `notify.patterns`: Union of both lists
    /// - `notify.delivery`: Other overrides if not default
    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
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

    /// Save policy to local override file atomically.
    ///
    /// Writes to a temp file and renames to prevent corruption on crash.
    ///
    /// **Note:** For concurrent-safe modifications, use `add_pattern_and_save()`
    /// which handles locking and reload automatically.
    pub async fn save_local(&self, agent_dir: &Path) -> std::io::Result<()> {
        let path = agent_dir.join("policy.local.yaml");
        let tmp_path = agent_dir.join("policy.local.yaml.tmp");
        let content = serde_saphyr::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        fs::write(&tmp_path, &content).await?;
        fs::rename(&tmp_path, &path).await?;
        debug!(path = %path.display(), "Saved local policy");
        Ok(())
    }

    /// Atomically add a pattern to the allow list and save.
    ///
    /// This is the recommended way to modify policy files as it:
    /// 1. Acquires a per-agent lock to prevent concurrent writes
    /// 2. Reloads the latest policy from disk
    /// 3. Adds the new pattern
    /// 4. Saves atomically
    ///
    /// This prevents race conditions where concurrent calls would overwrite
    /// each other's changes.
    pub async fn add_pattern_and_save(
        base_policy: &ToolPolicy,
        agent_dir: &Path,
        agent_name: &str,
        tool_type: ToolType,
        pattern: &str,
        policy_locks: &PolicyLocks,
    ) -> std::io::Result<()> {
        // Get or create lock for this agent
        let lock = policy_locks
            .entry(agent_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();

        // Hold lock while reading, modifying, and writing
        let _guard = lock.lock().await;

        let mut policy = base_policy.clone();
        policy.reload_local(agent_dir).await;
        policy.add_allow_pattern(tool_type, pattern);
        policy.save_local(agent_dir).await
    }

    /// Reload the local policy file and merge with current state.
    ///
    /// Use this before modifying and saving to avoid overwriting concurrent changes.
    pub async fn reload_local(&mut self, agent_dir: &Path) {
        if let Some(local) = Self::load_file(&agent_dir.join("policy.local.yaml")).await {
            // Merge local patterns into self (union of allow/deny lists)
            for pattern in local.allow {
                if !self.allow.contains(&pattern) {
                    self.allow.push(pattern);
                }
            }
            for pattern in local.deny {
                if !self.deny.contains(&pattern) {
                    self.deny.push(pattern);
                }
            }
        }
    }

    /// Add a pattern to the allow list.
    pub fn add_allow_pattern(&mut self, tool_type: ToolType, pattern: &str) {
        let full_pattern = format!("{}:{}", tool_type.as_str(), pattern);
        if !self.allow.contains(&full_pattern) {
            self.allow.push(full_pattern);
        }
    }

    /// Check if a tool invocation is allowed by the policy.
    ///
    /// Deny list is always checked first (air-gap safety). If a command matches
    /// the deny list, it is denied regardless of mode or allow list.
    ///
    /// # Arguments
    /// * `tool_type` - The type of tool being invoked
    /// * `invocation` - The tool invocation string (command for bash, tool name for MCP)
    pub fn check(&self, tool_type: ToolType, invocation: &str) -> PolicyDecision {
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
                // If in allow list, allow without asking
                if matches_any_typed(&self.allow, tool_str, invocation) {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Ask
                }
            }
        }
    }

    /// Check if a tool invocation should trigger a notification.
    pub fn should_notify(&self, tool_type: ToolType, invocation: &str) -> bool {
        let tool_str = tool_type.as_str();
        self.notify.enabled
            && (self.notify.patterns.is_empty()
                || matches_any_typed(&self.notify.patterns, tool_str, invocation))
    }
}

/// Check if an invocation matches any pattern in the list.
///
/// Patterns use the format `tool_type:pattern`:
/// - `bash:cargo *` — matches bash commands starting with cargo
/// - `mcp:github:*` — matches any github MCP tool
/// - `*:*secret*` — matches any tool with "secret" in invocation
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
    // Split pattern into tool type and pattern parts
    let (pattern_tool_type, pattern_rest) = match pattern.split_once(':') {
        Some((t, p)) => (t, p),
        None => {
            // No colon - treat entire pattern as matching any tool type
            ("*", pattern)
        }
    };

    // Check tool type matches
    if pattern_tool_type != "*" && !matches_pattern(pattern_tool_type, tool_type) {
        return false;
    }

    // Check invocation matches
    matches_pattern(pattern_rest, invocation)
}

/// Check if a string matches a single pattern.
///
/// Simple glob matching: `*` matches any sequence of characters.
/// Pattern parts must appear in order in the string.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    // Split pattern on '*' and check if all parts appear in order
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        // No wildcards - exact substring match
        return text.contains(pattern);
    }

    let mut pos = 0;
    for part in &parts {
        if part.is_empty() {
            continue;
        }

        // Find this part starting from current position
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
    use tempfile::TempDir;

    #[test]
    fn default_policy_has_correct_api_version() {
        let policy = ToolPolicy::default();
        assert_eq!(policy.api_version, "agnx/v1alpha1");
        assert_eq!(policy.kind, "Policy");
        assert_eq!(policy.mode, PolicyMode::Dangerous);
    }

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
        // Deny list should block commands even in restrict mode with allow: *
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

        // Deny list should block commands even in dangerous mode
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

        // Deny list should block commands even in ask mode
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

        // Wildcard deny blocks all tool types
        assert_eq!(
            policy.check(ToolType::Bash, "echo password123"),
            PolicyDecision::Deny
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "vault:get_password"),
            PolicyDecision::Deny
        );

        // Wildcard allow permits all tool types
        assert_eq!(
            policy.check(ToolType::Bash, "cat file.txt"),
            PolicyDecision::Ask
        );
        assert_eq!(
            policy.check(ToolType::Mcp, "filesystem:read_file"),
            PolicyDecision::Allow
        );
        // "readme.txt" contains "read" so it should match
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

    #[tokio::test]
    async fn load_returns_default_when_no_files() {
        let tmp = TempDir::new().unwrap();
        let policy = ToolPolicy::load(tmp.path()).await;

        assert_eq!(policy.mode, PolicyMode::Dangerous);
        assert!(policy.allow.is_empty());
        assert!(policy.deny.is_empty());
    }

    #[tokio::test]
    async fn load_base_policy() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("policy.yaml"),
            r#"
apiVersion: agnx/v1alpha1
kind: Policy
mode: restrict
deny:
  - "bash:rm*"
  - "*:*sudo*"
"#,
        )
        .unwrap();

        let policy = ToolPolicy::load(tmp.path()).await;

        assert_eq!(policy.api_version, "agnx/v1alpha1");
        assert_eq!(policy.kind, "Policy");
        assert_eq!(policy.mode, PolicyMode::Restrict);
        assert_eq!(policy.deny.len(), 2);
    }

    #[tokio::test]
    async fn load_and_merge_local_policy() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("policy.yaml"),
            r#"
apiVersion: agnx/v1alpha1
kind: Policy
mode: restrict
deny:
  - "bash:rm*"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("policy.local.yaml"),
            r#"
apiVersion: agnx/v1alpha1
kind: Policy
mode: ask
allow:
  - "bash:echo*"
"#,
        )
        .unwrap();

        let policy = ToolPolicy::load(tmp.path()).await;

        assert_eq!(policy.mode, PolicyMode::Ask);
        assert_eq!(policy.deny, vec!["bash:rm*"]);
        assert_eq!(policy.allow, vec!["bash:echo*"]);
    }

    #[tokio::test]
    async fn save_local_creates_file() {
        let tmp = TempDir::new().unwrap();
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:echo*".to_string()],
            ..Default::default()
        };

        policy.save_local(tmp.path()).await.unwrap();

        let content = std::fs::read_to_string(tmp.path().join("policy.local.yaml")).unwrap();
        assert!(content.contains("ask"));
        assert!(content.contains("echo*"));
    }

    #[test]
    fn policy_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&PolicyMode::Dangerous).unwrap(),
            "\"dangerous\""
        );
        assert_eq!(serde_json::to_string(&PolicyMode::Ask).unwrap(), "\"ask\"");
        assert_eq!(
            serde_json::to_string(&PolicyMode::Restrict).unwrap(),
            "\"restrict\""
        );
    }

    #[test]
    fn delivery_serialization() {
        // Log delivery
        assert_eq!(
            serde_json::to_string(&Delivery::Log).unwrap(),
            r#"{"type":"log"}"#
        );
        // Webhook delivery
        assert_eq!(
            serde_json::to_string(&Delivery::Webhook {
                url: "https://example.com".to_string()
            })
            .unwrap(),
            r#"{"type":"webhook","url":"https://example.com"}"#
        );
    }

    #[test]
    fn tool_type_as_str() {
        assert_eq!(ToolType::Bash.as_str(), "bash");
        assert_eq!(ToolType::Mcp.as_str(), "mcp");
        assert_eq!(ToolType::Builtin.as_str(), "builtin");
    }
}
