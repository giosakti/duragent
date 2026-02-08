//! Duragent Format agent specification parsing.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::AgentLoadError;
use super::policy::ToolPolicy;
use super::{API_VERSION_V1ALPHA1, KIND_AGENT};
use crate::context::default_context_window;
use crate::llm::Provider;

/// Default maximum tool iterations for agentic loops.
pub const DEFAULT_MAX_TOOL_ITERATIONS: u32 = 10;

// ============================================================================
// Public Types
// ============================================================================

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
    ///
    /// Note: Many provider APIs do not expose a direct "max input tokens" parameter. This
    /// is intended for Duragent-side preprocessing (e.g., truncating history/context).
    pub max_input_tokens: Option<u32>,
    /// Maximum tokens the model may generate for the response (output tokens).
    ///
    /// We prefer `max_output_tokens` in the Duragent Format schema for clarity, but accept `max_tokens`
    /// as a backwards-compatible alias (common in OpenAI-style APIs).
    #[serde(default, alias = "max_tokens")]
    pub max_output_tokens: Option<u32>,
    pub base_url: Option<String>,
}

impl ModelConfig {
    /// Returns max_input_tokens if set, otherwise the default for the model.
    pub fn effective_max_input_tokens(&self) -> u32 {
        self.max_input_tokens
            .unwrap_or_else(|| default_context_window(&self.name))
    }
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
    /// Context window management configuration.
    #[serde(default)]
    pub context: ContextConfig,
}

fn default_max_tool_iterations() -> u32 {
    DEFAULT_MAX_TOOL_ITERATIONS
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
    20_000
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
}

// ============================================================================
// AgentSpec Implementation
// ============================================================================

impl AgentSpec {
    /// Create an agent from YAML content and pre-loaded files.
    ///
    /// This method performs no file I/O - all content should be loaded by the caller
    /// (e.g., `FileAgentCatalog`).
    pub fn from_yaml(
        yaml_content: &str,
        files: LoadedAgentFiles,
        policy: ToolPolicy,
        agent_dir: PathBuf,
    ) -> Result<Self, AgentLoadError> {
        let raw: RawAgentSpec =
            serde_saphyr::from_str(yaml_content).map_err(AgentLoadError::Yaml)?;

        // Validate apiVersion
        if raw.api_version != API_VERSION_V1ALPHA1 {
            return Err(AgentLoadError::Validation(format!(
                "unsupported apiVersion '{}', expected '{API_VERSION_V1ALPHA1}'",
                raw.api_version
            )));
        }

        // Validate kind
        if raw.kind != KIND_AGENT {
            return Err(AgentLoadError::Validation(format!(
                "unsupported kind '{}', expected '{KIND_AGENT}'",
                raw.kind
            )));
        }

        // Validate context config
        if raw.spec.session.context.max_tool_result_tokens == 0 {
            return Err(AgentLoadError::Validation(
                "session.context.max_tool_result_tokens must be > 0".to_string(),
            ));
        }

        Ok(AgentSpec {
            api_version: raw.api_version,
            kind: raw.kind,
            metadata: raw.metadata,
            model: raw.spec.model,
            soul: files.soul,
            system_prompt: files.system_prompt,
            instructions: files.instructions,
            session: raw.spec.session,
            access: raw.spec.access,
            memory: raw.spec.memory,
            tools: raw.spec.tools,
            policy,
            agent_dir,
        })
    }

    /// Get file paths referenced in the YAML content.
    ///
    /// Used by storage implementations to know which files to load.
    pub fn parse_file_refs(yaml_content: &str) -> Result<AgentFileRefs, AgentLoadError> {
        let raw: RawAgentSpec =
            serde_saphyr::from_str(yaml_content).map_err(AgentLoadError::Yaml)?;

        Ok(AgentFileRefs {
            name: raw.metadata.name,
            soul: raw.spec.soul,
            system_prompt: raw.spec.system_prompt,
            instructions: raw.spec.instructions,
        })
    }
}

// ============================================================================
// Implementation Details
// ============================================================================

/// Raw YAML structure for parsing agent.yaml files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAgentSpec {
    api_version: String,
    kind: String,
    metadata: AgentMetadata,
    spec: RawAgentSpecBody,
}

#[derive(Debug, Deserialize)]
struct RawAgentSpecBody {
    model: ModelConfig,
    soul: Option<String>,
    system_prompt: Option<String>,
    instructions: Option<String>,
    #[serde(default)]
    session: AgentSessionConfig,
    #[serde(default)]
    access: Option<AccessConfig>,
    #[serde(default)]
    memory: Option<AgentMemoryConfig>,
    #[serde(default)]
    tools: Vec<ToolConfig>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::store::AgentCatalog;
    use crate::store::file::FileAgentCatalog;

    fn write_yaml(dir: &Path, contents: &str) {
        std::fs::write(dir.join("agent.yaml"), contents).unwrap();
    }

    /// Helper to load an agent using the FileAgentCatalog.
    async fn load_agent(
        agents_dir: &Path,
        agent_name: &str,
    ) -> crate::store::StorageResult<AgentSpec> {
        let catalog = FileAgentCatalog::new(agents_dir);
        catalog.load(agent_name).await
    }

    /// Helper to scan agents directory and return warnings.
    async fn scan_agents(agents_dir: &Path) -> crate::store::AgentScanResult {
        let catalog = FileAgentCatalog::new(agents_dir);
        catalog.load_all().await.unwrap()
    }

    #[tokio::test]
    async fn load_minimal_agent() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: test-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        let result = scan_agents(&agents_dir).await;
        assert!(result.warnings.is_empty());
        assert_eq!(result.agents.len(), 1);

        let agent = &result.agents[0];
        assert_eq!(agent.api_version, API_VERSION_V1ALPHA1);
        assert_eq!(agent.kind, KIND_AGENT);
        assert_eq!(agent.metadata.name, "test-agent");
        assert_eq!(agent.model.provider, Provider::OpenRouter);
        assert_eq!(agent.model.name, "anthropic/claude-sonnet-4");
        assert!(agent.system_prompt.is_none());
        assert!(agent.instructions.is_none());
    }

    #[tokio::test]
    async fn load_agent_with_system_prompt() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: test-agent
  description: A test agent
  version: 1.0.0
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./SYSTEM_PROMPT.md
"#,
        );
        std::fs::write(
            agent_dir.join("SYSTEM_PROMPT.md"),
            "You are a helpful assistant.",
        )
        .unwrap();

        let agent = load_agent(&agents_dir, "test-agent").await.unwrap();
        assert_eq!(agent.metadata.name, "test-agent");
        assert_eq!(agent.metadata.description, Some("A test agent".to_string()));
        assert_eq!(agent.metadata.version, Some("1.0.0".to_string()));
        assert_eq!(
            agent.system_prompt,
            Some("You are a helpful assistant.".to_string())
        );
    }

    #[tokio::test]
    async fn load_agent_missing_system_prompt_is_warning_not_error() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: test-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./MISSING.md
"#,
        );

        let result = scan_agents(&agents_dir).await;
        assert_eq!(result.agents.len(), 1);
        assert!(result.agents[0].system_prompt.is_none());
        assert_eq!(result.warnings.len(), 1);
    }

    #[tokio::test]
    async fn load_agent_invalid_api_version() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v2
kind: Agent
metadata:
  name: test-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        // Invalid agents are skipped with warnings, not errors
        let result = scan_agents(&agents_dir).await;
        assert!(result.agents.is_empty());
        assert_eq!(result.warnings.len(), 1);
    }

    #[tokio::test]
    async fn load_agent_with_all_model_options() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: test-agent
  labels:
    domain: productivity
    tier: premium
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
    temperature: 0.7
    max_output_tokens: 4096
    base_url: https://custom.example.com
"#,
        );

        let agent = load_agent(&agents_dir, "test-agent").await.unwrap();
        assert_eq!(agent.model.provider, Provider::OpenRouter);
        assert_eq!(agent.model.temperature, Some(0.7));
        assert_eq!(agent.model.max_output_tokens, Some(4096));
        assert_eq!(
            agent.model.base_url,
            Some("https://custom.example.com".to_string())
        );
        assert_eq!(
            agent.metadata.labels.get("domain"),
            Some(&"productivity".to_string())
        );
        assert_eq!(
            agent.metadata.labels.get("tier"),
            Some(&"premium".to_string())
        );
    }

    #[tokio::test]
    async fn load_agent_with_session_config() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("background-worker");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: background-worker
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  session:
    on_disconnect: continue
"#,
        );

        let result = scan_agents(&agents_dir).await;
        assert!(result.warnings.is_empty());
        assert_eq!(
            result.agents[0].session.on_disconnect,
            OnDisconnect::Continue
        );
    }

    #[tokio::test]
    async fn load_agent_session_defaults_to_pause() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: test-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        let agent = load_agent(&agents_dir, "test-agent").await.unwrap();
        assert_eq!(agent.session.on_disconnect, OnDisconnect::Pause);
    }

    #[test]
    fn on_disconnect_serialization() {
        assert_eq!(
            serde_json::to_string(&OnDisconnect::Pause).unwrap(),
            "\"pause\""
        );
        assert_eq!(
            serde_json::to_string(&OnDisconnect::Continue).unwrap(),
            "\"continue\""
        );
    }

    #[tokio::test]
    async fn load_agent_with_access_config() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("guarded-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: guarded-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    dm:
      policy: allowlist
      allowlist: ["12345"]
    groups:
      policy: allowlist
      allowlist: ["-100123456"]
      sender_default: silent
      sender_overrides:
        "67890": passive
        "99999": block
"#,
        );

        let agent = load_agent(&agents_dir, "guarded-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.dm.policy, DmPolicy::Allowlist);
        assert_eq!(access.dm.allowlist, vec!["12345"]);
        assert_eq!(access.groups.policy, GroupPolicy::Allowlist);
        assert_eq!(access.groups.allowlist, vec!["-100123456"]);
        assert_eq!(access.groups.sender_default, SenderDisposition::Silent);
        assert_eq!(
            access.groups.sender_overrides.get("67890"),
            Some(&SenderDisposition::Passive)
        );
        assert_eq!(
            access.groups.sender_overrides.get("99999"),
            Some(&SenderDisposition::Block)
        );
    }

    #[tokio::test]
    async fn load_agent_with_activation_mode_and_context_buffer() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("mention-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: mention-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      policy: open
      activation: mention
      context_buffer:
        max_messages: 50
        max_age_hours: 12
"#,
        );

        let agent = load_agent(&agents_dir, "mention-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.activation, ActivationMode::Mention);
        assert_eq!(access.groups.context_buffer.max_messages, 50);
        assert_eq!(access.groups.context_buffer.max_age_hours, 12);
    }

    #[tokio::test]
    async fn load_agent_with_activation_always() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("always-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: always-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      activation: always
"#,
        );

        let agent = load_agent(&agents_dir, "always-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.activation, ActivationMode::Always);
        // Defaults
        assert_eq!(access.groups.context_buffer.max_messages, 100);
        assert_eq!(access.groups.context_buffer.max_age_hours, 24);
    }

    #[tokio::test]
    async fn load_agent_activation_defaults_to_mention() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("default-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: default-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      policy: open
"#,
        );

        let agent = load_agent(&agents_dir, "default-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.activation, ActivationMode::Mention);
    }

    #[test]
    fn activation_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&ActivationMode::Mention).unwrap(),
            "\"mention\""
        );
        assert_eq!(
            serde_json::to_string(&ActivationMode::Always).unwrap(),
            "\"always\""
        );
    }

    #[tokio::test]
    async fn load_agent_with_context_buffer_mode_silent() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("silent-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: silent-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      activation: mention
      context_buffer:
        mode: silent
        max_messages: 50
"#,
        );

        let agent = load_agent(&agents_dir, "silent-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.context_buffer.mode, ContextBufferMode::Silent);
        assert_eq!(access.groups.context_buffer.max_messages, 50);
    }

    #[tokio::test]
    async fn load_agent_with_context_buffer_mode_passive() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("passive-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: passive-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      activation: mention
      context_buffer:
        mode: passive
"#,
        );

        let agent = load_agent(&agents_dir, "passive-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(
            access.groups.context_buffer.mode,
            ContextBufferMode::Passive
        );
    }

    #[tokio::test]
    async fn load_agent_context_buffer_mode_defaults_to_silent() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("default-mode-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: default-mode-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      activation: mention
      context_buffer:
        max_messages: 75
"#,
        );

        let agent = load_agent(&agents_dir, "default-mode-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.context_buffer.mode, ContextBufferMode::Silent);
        assert_eq!(access.groups.context_buffer.max_messages, 75);
    }

    #[test]
    fn context_buffer_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&ContextBufferMode::Silent).unwrap(),
            "\"silent\""
        );
        assert_eq!(
            serde_json::to_string(&ContextBufferMode::Passive).unwrap(),
            "\"passive\""
        );
    }

    #[tokio::test]
    async fn load_agent_without_access_config() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("open-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: open-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        let agent = load_agent(&agents_dir, "open-agent").await.unwrap();
        assert!(agent.access.is_none());
    }

    // ------------------------------------------------------------------------
    // Queue config
    // ------------------------------------------------------------------------

    #[test]
    fn queue_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&QueueMode::Batch).unwrap(),
            "\"batch\""
        );
        assert_eq!(
            serde_json::to_string(&QueueMode::Sequential).unwrap(),
            "\"sequential\""
        );
        assert_eq!(serde_json::to_string(&QueueMode::Drop).unwrap(), "\"drop\"");
    }

    #[test]
    fn overflow_strategy_serialization() {
        assert_eq!(
            serde_json::to_string(&OverflowStrategy::DropOld).unwrap(),
            "\"drop_old\""
        );
        assert_eq!(
            serde_json::to_string(&OverflowStrategy::DropNew).unwrap(),
            "\"drop_new\""
        );
        assert_eq!(
            serde_json::to_string(&OverflowStrategy::Reject).unwrap(),
            "\"reject\""
        );
    }

    #[test]
    fn queue_config_defaults() {
        let config = QueueConfig::default();
        assert_eq!(config.mode, QueueMode::Batch);
        assert_eq!(config.max_pending, 10);
        assert_eq!(config.overflow, OverflowStrategy::DropOld);
        assert!(config.reject_message.is_none());
        assert!(config.debounce.enabled);
        assert_eq!(config.debounce.window_ms, 1500);
    }

    #[tokio::test]
    async fn load_agent_with_queue_config() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("queued-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: queued-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      policy: open
      queue:
        mode: sequential
        max_pending: 5
        overflow: reject
        reject_message: "I'm busy, please wait."
        debounce:
          enabled: false
          window_ms: 2000
"#,
        );

        let agent = load_agent(&agents_dir, "queued-agent").await.unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.queue.mode, QueueMode::Sequential);
        assert_eq!(access.groups.queue.max_pending, 5);
        assert_eq!(access.groups.queue.overflow, OverflowStrategy::Reject);
        assert_eq!(
            access.groups.queue.reject_message,
            Some("I'm busy, please wait.".to_string())
        );
        assert!(!access.groups.queue.debounce.enabled);
        assert_eq!(access.groups.queue.debounce.window_ms, 2000);
    }

    #[tokio::test]
    async fn load_agent_queue_config_defaults() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("default-queue-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: default-queue-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  access:
    groups:
      policy: open
"#,
        );

        let agent = load_agent(&agents_dir, "default-queue-agent")
            .await
            .unwrap();
        let access = agent.access.unwrap();
        assert_eq!(access.groups.queue.mode, QueueMode::Batch);
        assert_eq!(access.groups.queue.max_pending, 10);
        assert_eq!(access.groups.queue.overflow, OverflowStrategy::DropOld);
        assert!(access.groups.queue.reject_message.is_none());
        assert!(access.groups.queue.debounce.enabled);
        assert_eq!(access.groups.queue.debounce.window_ms, 1500);
    }

    // ------------------------------------------------------------------------
    // Context config
    // ------------------------------------------------------------------------

    #[test]
    fn context_config_defaults() {
        let config = ContextConfig::default();
        assert_eq!(config.max_history_tokens, 20_000);
        assert_eq!(config.max_tool_result_tokens, 8_000);
        assert_eq!(config.tool_result_truncation, ToolResultTruncation::Head);
        assert_eq!(config.tool_result_keep_first, 2);
        assert_eq!(config.tool_result_keep_last, 5);
    }

    #[test]
    fn context_config_default_deserialization() {
        // Deserializing an empty session config should produce defaults for context
        let yaml = r#"
on_disconnect: pause
max_tool_iterations: 10
"#;
        let config: AgentSessionConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.context.max_history_tokens, 20_000);
        assert_eq!(config.context.max_tool_result_tokens, 8_000);
    }

    #[test]
    fn context_config_explicit_values() {
        let yaml = r#"
on_disconnect: pause
context:
  max_history_tokens: 50000
  max_tool_result_tokens: 16000
  tool_result_truncation: tail
  tool_result_keep_first: 3
  tool_result_keep_last: 10
"#;
        let config: AgentSessionConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.context.max_history_tokens, 50_000);
        assert_eq!(config.context.max_tool_result_tokens, 16_000);
        assert_eq!(
            config.context.tool_result_truncation,
            ToolResultTruncation::Tail
        );
        assert_eq!(config.context.tool_result_keep_first, 3);
        assert_eq!(config.context.tool_result_keep_last, 10);
    }

    #[test]
    fn tool_result_truncation_serialization() {
        assert_eq!(
            serde_json::to_string(&ToolResultTruncation::Head).unwrap(),
            "\"head\""
        );
        assert_eq!(
            serde_json::to_string(&ToolResultTruncation::Tail).unwrap(),
            "\"tail\""
        );
        assert_eq!(
            serde_json::to_string(&ToolResultTruncation::Both).unwrap(),
            "\"both\""
        );
    }

    #[tokio::test]
    async fn load_agent_with_context_config() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("context-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: context-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  session:
    on_disconnect: pause
    context:
      max_history_tokens: 30000
      max_tool_result_tokens: 4000
      tool_result_truncation: both
      tool_result_keep_first: 1
      tool_result_keep_last: 3
"#,
        );

        let agent = load_agent(&agents_dir, "context-agent").await.unwrap();
        assert_eq!(agent.session.context.max_history_tokens, 30_000);
        assert_eq!(agent.session.context.max_tool_result_tokens, 4_000);
        assert_eq!(
            agent.session.context.tool_result_truncation,
            ToolResultTruncation::Both
        );
        assert_eq!(agent.session.context.tool_result_keep_first, 1);
        assert_eq!(agent.session.context.tool_result_keep_last, 3);
    }

    #[tokio::test]
    async fn load_agent_context_config_defaults_when_omitted() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("no-context-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: no-context-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        let agent = load_agent(&agents_dir, "no-context-agent").await.unwrap();
        assert_eq!(agent.session.context.max_history_tokens, 20_000);
        assert_eq!(agent.session.context.max_tool_result_tokens, 8_000);
        assert_eq!(
            agent.session.context.tool_result_truncation,
            ToolResultTruncation::Head
        );
    }

    // ------------------------------------------------------------------------
    // ModelConfig
    // ------------------------------------------------------------------------

    #[test]
    fn effective_max_input_tokens_with_explicit_value() {
        let config = ModelConfig {
            provider: Provider::OpenRouter,
            name: "anthropic/claude-sonnet-4".to_string(),
            temperature: None,
            max_input_tokens: Some(100_000),
            max_output_tokens: None,
            base_url: None,
        };
        assert_eq!(config.effective_max_input_tokens(), 100_000);
    }

    #[test]
    fn effective_max_input_tokens_defaults_from_model_name() {
        let config = ModelConfig {
            provider: Provider::OpenRouter,
            name: "anthropic/claude-sonnet-4".to_string(),
            temperature: None,
            max_input_tokens: None,
            max_output_tokens: None,
            base_url: None,
        };
        assert_eq!(config.effective_max_input_tokens(), 200_000);
    }
}
