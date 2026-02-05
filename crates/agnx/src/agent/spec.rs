//! AAF agent specification parsing.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::AgentLoadError;
use super::policy::ToolPolicy;
use super::{API_VERSION_V1ALPHA1, KIND_AGENT};
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
    /// Memory configuration.
    pub memory: Option<AgentMemoryConfig>,
    /// Tool configurations for agentic capabilities.
    pub tools: Vec<ToolConfig>,
    /// Tool execution policy.
    pub policy: ToolPolicy,
    /// Directory containing the agent's configuration files.
    pub agent_dir: PathBuf,
}

/// Agent metadata from the AAF spec.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentMetadata {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Model configuration from the AAF spec.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelConfig {
    pub provider: Provider,
    pub name: String,
    pub temperature: Option<f32>,
    /// Optional hint for input truncation before calling the provider.
    ///
    /// Note: Many provider APIs do not expose a direct "max input tokens" parameter. This
    /// is intended for Agnx-side preprocessing (e.g., truncating history/context).
    pub max_input_tokens: Option<u32>,
    /// Maximum tokens the model may generate for the response (output tokens).
    ///
    /// We prefer `max_output_tokens` in the AAF schema for clarity, but accept `max_tokens`
    /// as a backwards-compatible alias (common in OpenAI-style APIs).
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
}

fn default_max_tool_iterations() -> u32 {
    DEFAULT_MAX_TOOL_ITERATIONS
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

/// Tool configuration from the AAF spec.
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

        Ok(AgentSpec {
            api_version: raw.api_version,
            kind: raw.kind,
            metadata: raw.metadata,
            model: raw.spec.model,
            soul: files.soul,
            system_prompt: files.system_prompt,
            instructions: files.instructions,
            session: raw.spec.session,
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
            r#"apiVersion: agnx/v1alpha1
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
            r#"apiVersion: agnx/v1alpha1
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
            r#"apiVersion: agnx/v1alpha1
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
            r#"apiVersion: agnx/v2
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
            r#"apiVersion: agnx/v1alpha1
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
            r#"apiVersion: agnx/v1alpha1
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
            r#"apiVersion: agnx/v1alpha1
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
}
