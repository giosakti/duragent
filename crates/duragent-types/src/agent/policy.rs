//! Tool policy types for command filtering and approval.
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
//!
//! Evaluation logic (`check`, `merge`, `should_notify`, `add_allow_pattern`)
//! lives in `duragent::agent::policy_eval`.

use serde::{Deserialize, Serialize};

// ============================================================================
// Constants
// ============================================================================

/// Current policy API version.
pub const POLICY_API_VERSION: &str = "duragent/v1alpha1";

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    /// Bash/shell commands.
    Bash,
    /// MCP server tools.
    Mcp,
    /// Built-in tools.
    Builtin,
    /// CLI/discovered tools.
    Cli,
}

impl ToolType {
    /// Get the string representation for pattern matching.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolType::Bash => "bash",
            ToolType::Mcp => "mcp",
            ToolType::Builtin => "builtin",
            ToolType::Cli => "cli",
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_has_correct_api_version() {
        let policy = ToolPolicy::default();
        assert_eq!(policy.api_version, "duragent/v1alpha1");
        assert_eq!(policy.kind, "Policy");
        assert_eq!(policy.mode, PolicyMode::Dangerous);
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
        assert_eq!(
            serde_json::to_string(&Delivery::Log).unwrap(),
            r#"{"type":"log"}"#
        );
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
        assert_eq!(ToolType::Cli.as_str(), "cli");
    }
}
