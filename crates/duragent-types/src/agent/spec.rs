//! Agent specification types.
//!
//! Evaluation methods (`effective_max_input_tokens`, `with_defaults`,
//! `default_context_window`) live in `duragent::agent::spec_eval`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::access::AccessConfig;
use super::policy::ToolPolicy;
use crate::provider::Provider;
use crate::session::CompactionMode;

/// Default maximum tool iterations for agentic loops.
pub const DEFAULT_MAX_TOOL_ITERATIONS: u32 = 10;

/// Default timeout for a single LLM call (connect + stream), in seconds.
pub const DEFAULT_LLM_TIMEOUT_SECONDS: u64 = 300;

/// Parsed skill metadata from a SKILL.md file.
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    /// Skill name (lowercase, hyphens only, ≤64 chars).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Path to the skill directory containing SKILL.md.
    pub skill_path: PathBuf,
    /// Tool names this skill uses (from `allowed-tools` frontmatter).
    pub allowed_tools: Vec<String>,
    /// Arbitrary key-value metadata from frontmatter.
    pub metadata: HashMap<String, String>,
}

/// An agent specification loaded from an agent.yaml file.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentMetadata,
    pub model: ModelConfig,
    /// Agent personality and character (who the agent IS).
    pub soul: Option<String>,
    /// Core system prompt (what the agent DOES).
    pub system_prompt: Option<String>,
    /// Additional runtime instructions.
    pub instructions: Option<String>,
    /// Loaded skill metadata from skills directory.
    pub skills: Vec<SkillMetadata>,
    /// Session behavior configuration.
    pub session: AgentSessionConfig,
    /// Access control configuration.
    pub access: Option<AccessConfig>,
    /// Memory configuration.
    pub memory: Option<AgentMemoryConfig>,
    /// Tool configurations for agentic capabilities.
    pub tools: Vec<ToolConfig>,
    /// Tool execution policy.
    pub policy: ToolPolicy,
    /// Tool lifecycle hooks (guards and steering).
    pub hooks: HooksConfig,
    /// Directory containing the agent's configuration files.
    pub agent_dir: PathBuf,
}

/// Agent metadata from the Duragent Format spec.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentMetadata {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Model configuration from the Duragent Format spec.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelConfig {
    pub provider: Provider,
    pub name: String,
    pub temperature: Option<f32>,
    /// Optional hint for input truncation before calling the provider.
    pub max_input_tokens: Option<u32>,
    /// Maximum tokens the model may generate for the response (output tokens).
    #[serde(default, alias = "max_tokens")]
    pub max_output_tokens: Option<u32>,
    pub base_url: Option<String>,
}

/// Session behavior configuration for an agent.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AgentSessionConfig {
    /// Behavior when client disconnects from a session.
    #[serde(default)]
    pub on_disconnect: OnDisconnect,
    /// Maximum number of tool iterations before stopping.
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: u32,
    /// Timeout for a single LLM call (connect + full stream consumption), in seconds.
    #[serde(default = "default_llm_timeout_seconds")]
    pub llm_timeout_seconds: u64,
    /// Context window management configuration.
    #[serde(default)]
    pub context: ContextConfig,
    /// Per-agent TTL override (hours). Overrides global `sessions.ttl_hours`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_hours: Option<u64>,
    /// Per-agent compaction mode override. Overrides global `sessions.compaction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionMode>,
}

fn default_max_tool_iterations() -> u32 {
    DEFAULT_MAX_TOOL_ITERATIONS
}

fn default_llm_timeout_seconds() -> u64 {
    DEFAULT_LLM_TIMEOUT_SECONDS
}

/// Behavior when client disconnects from a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDisconnect {
    /// Pause the session and wait for reconnect (default).
    #[default]
    Pause,
    /// Continue executing in the background.
    Continue,
}

/// Context window management configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextConfig {
    /// Maximum tokens for conversation history at render time.
    /// 0 means no cap (use full available budget).
    #[serde(default = "default_max_history_tokens")]
    pub max_history_tokens: u32,
    /// Maximum tokens per individual tool result before truncation.
    #[serde(default = "default_max_tool_result_tokens")]
    pub max_tool_result_tokens: u32,
    /// Truncation strategy for tool results that exceed max_tool_result_tokens.
    #[serde(default)]
    pub tool_result_truncation: ToolResultTruncation,
    /// Number of earliest tool results to keep unmasked in the agentic loop.
    #[serde(default = "default_tool_result_keep_first")]
    pub tool_result_keep_first: u32,
    /// Number of latest tool results to keep unmasked in the agentic loop.
    #[serde(default = "default_tool_result_keep_last")]
    pub tool_result_keep_last: u32,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_history_tokens: default_max_history_tokens(),
            max_tool_result_tokens: default_max_tool_result_tokens(),
            tool_result_truncation: ToolResultTruncation::default(),
            tool_result_keep_first: default_tool_result_keep_first(),
            tool_result_keep_last: default_tool_result_keep_last(),
        }
    }
}

fn default_max_history_tokens() -> u32 {
    40_000
}

fn default_max_tool_result_tokens() -> u32 {
    8_000
}

fn default_tool_result_keep_first() -> u32 {
    2
}

fn default_tool_result_keep_last() -> u32 {
    5
}

/// Truncation strategy for tool results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultTruncation {
    /// Keep the beginning of the output (default).
    #[default]
    Head,
    /// Keep the end of the output.
    Tail,
    /// Keep both the beginning and end.
    Both,
}

/// Memory configuration for an agent.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentMemoryConfig {
    /// Memory backend implementation.
    /// Currently only "filesystem" is supported.
    #[serde(default = "default_memory_backend")]
    pub backend: String,
}

fn default_memory_backend() -> String {
    "filesystem".to_string()
}

/// Tool configuration from the Duragent Format spec.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolConfig {
    /// Built-in tool (e.g., bash).
    Builtin { name: String },
    /// CLI tool executed via script.
    Cli {
        name: String,
        command: String,
        #[serde(default)]
        readme: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
}

/// Configurable hooks for tool lifecycle events.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub before_tool: Vec<BeforeToolHook>,
    #[serde(default)]
    pub after_tool: Vec<AfterToolHook>,
}

/// A guard that runs before a tool executes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BeforeToolHook {
    /// Tool pattern to match (e.g. "background_process:send_keys", "bash", "web:*").
    #[serde(rename = "match")]
    pub tool_match: String,
    /// The type of guard to apply.
    #[serde(rename = "type")]
    pub hook_type: BeforeToolType,
    /// For `depends_on`: the prior tool:action that must exist.
    #[serde(default)]
    pub prior: Option<String>,
    /// For `depends_on`: argument field that must match between calls.
    #[serde(default)]
    pub match_arg: Option<String>,
    /// For `skip_duplicate`: argument fields that define call identity.
    #[serde(default)]
    pub match_args: Vec<String>,
}

/// The type of before-tool guard.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BeforeToolType {
    DependsOn,
    SkipDuplicate,
}

/// A steering message injected after a tool executes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AfterToolHook {
    /// Tool pattern to match (e.g. "background_process:capture").
    #[serde(rename = "match")]
    pub tool_match: String,
    /// Message to inject after successful execution.
    pub message: String,
    /// Suppress the message if any key-value pair matches the tool arguments.
    #[serde(default)]
    pub unless: HashMap<String, Value>,
}

/// Loaded file contents for optional agent files.
///
/// Used by `AgentSpec::from_yaml()` to construct an agent from pre-loaded content.
#[derive(Debug, Default)]
pub struct LoadedAgentFiles {
    pub soul: Option<String>,
    pub system_prompt: Option<String>,
    pub instructions: Option<String>,
}

/// File references parsed from agent.yaml.
///
/// Used by storage implementations to know which files to load.
#[derive(Debug)]
pub struct AgentFileRefs {
    pub name: String,
    pub soul: Option<String>,
    pub system_prompt: Option<String>,
    pub instructions: Option<String>,
    pub skills_dir: Option<String>,
}
