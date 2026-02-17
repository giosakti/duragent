//! Duragent Format agent specification parsing.
//!
//! Contains only parsing/loading logic that depends on duragent internals.
//! All domain types live in `duragent-types`.

use std::path::PathBuf;

use serde::Deserialize;

use super::error::{AgentLoadError, AgentLoadWarning};
use super::{API_VERSION_V1ALPHA1, KIND_AGENT};
use crate::agent::{
    AccessConfig, AgentFileRefs, AgentMemoryConfig, AgentMetadata, AgentSessionConfig, AgentSpec,
    HooksConfig, HooksConfigEval, LoadedAgentFiles, ModelConfig, SkillMetadata, ToolConfig,
    ToolPolicy,
};
/// Create an agent from YAML content and pre-loaded files.
///
/// This function performs no file I/O - all content should be loaded by the caller
/// (e.g., `FileAgentCatalog`).
pub fn parse_agent_yaml(
    yaml_content: &str,
    files: LoadedAgentFiles,
    skills: Vec<SkillMetadata>,
    policy: ToolPolicy,
    agent_dir: PathBuf,
) -> Result<AgentSpec, AgentLoadError> {
    let raw: RawAgentSpec = serde_saphyr::from_str(yaml_content).map_err(AgentLoadError::Yaml)?;

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

    // Merge agent-configured hooks with defaults from enabled tools.
    let tool_names: Vec<&str> = raw
        .spec
        .tools
        .iter()
        .filter_map(|t| match t {
            ToolConfig::Builtin { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let defaults = crate::tools::hooks::default_hooks(&tool_names);
    let hooks = raw.spec.hooks.with_defaults(defaults);

    Ok(AgentSpec {
        api_version: raw.api_version,
        kind: raw.kind,
        metadata: raw.metadata,
        model: raw.spec.model,
        soul: files.soul,
        system_prompt: files.system_prompt,
        instructions: files.instructions,
        skills,
        session: raw.spec.session,
        access: raw.spec.access,
        memory: raw.spec.memory,
        tools: raw.spec.tools,
        policy,
        hooks,
        agent_dir,
    })
}

/// Get file paths referenced in the YAML content.
///
/// Used by storage implementations to know which files to load.
pub fn parse_agent_file_refs(yaml_content: &str) -> Result<AgentFileRefs, AgentLoadError> {
    let raw: RawAgentSpec = serde_saphyr::from_str(yaml_content).map_err(AgentLoadError::Yaml)?;

    Ok(AgentFileRefs {
        name: raw.metadata.name,
        soul: raw.spec.soul,
        system_prompt: raw.spec.system_prompt,
        instructions: raw.spec.instructions,
        skills_dir: raw.spec.skills_dir,
    })
}

/// Check all `ToolConfig::Builtin` entries against `KNOWN_BUILTIN_TOOLS`.
///
/// Returns warnings for any unrecognized builtin tool names (e.g. typos).
pub fn validate_builtin_tools(agent_name: &str, tools: &[ToolConfig]) -> Vec<AgentLoadWarning> {
    tools
        .iter()
        .filter_map(|t| match t {
            ToolConfig::Builtin { name }
                if !crate::tools::KNOWN_BUILTIN_TOOLS.contains(&name.as_str()) =>
            {
                Some(AgentLoadWarning::UnknownBuiltinTool {
                    agent: agent_name.to_string(),
                    tool_name: name.clone(),
                })
            }
            _ => None,
        })
        .collect()
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
    skills_dir: Option<String>,
    #[serde(default)]
    session: AgentSessionConfig,
    #[serde(default)]
    access: Option<AccessConfig>,
    #[serde(default)]
    memory: Option<AgentMemoryConfig>,
    #[serde(default)]
    tools: Vec<ToolConfig>,
    #[serde(default)]
    hooks: HooksConfig,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::agent::{
        ActivationMode, ContextBufferMode, ContextConfig, DmPolicy, GroupPolicy, OnDisconnect,
        OverflowStrategy, QueueMode, SenderDisposition, ToolResultTruncation,
    };
    use crate::llm::Provider;
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
        let catalog = FileAgentCatalog::new(agents_dir, None);
        catalog.load(agent_name).await
    }

    /// Helper to scan agents directory and return warnings.
    async fn scan_agents(agents_dir: &Path) -> crate::store::AgentScanResult {
        let catalog = FileAgentCatalog::new(agents_dir, None);
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

    #[test]
    fn context_config_defaults() {
        let config = ContextConfig::default();
        assert_eq!(config.max_history_tokens, 40_000);
        assert_eq!(config.max_tool_result_tokens, 8_000);
        assert_eq!(config.tool_result_truncation, ToolResultTruncation::Head);
        assert_eq!(config.tool_result_keep_first, 2);
        assert_eq!(config.tool_result_keep_last, 5);
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

    #[test]
    fn validate_builtin_tools_accepts_known_tools() {
        let tools = vec![
            ToolConfig::Builtin {
                name: "bash".to_string(),
            },
            ToolConfig::Builtin {
                name: "schedule".to_string(),
            },
        ];
        let warnings = validate_builtin_tools("test-agent", &tools);
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_builtin_tools_warns_on_unknown() {
        let tools = vec![
            ToolConfig::Builtin {
                name: "bash".to_string(),
            },
            ToolConfig::Builtin {
                name: "bassh".to_string(),
            },
        ];
        let warnings = validate_builtin_tools("test-agent", &tools);
        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            AgentLoadWarning::UnknownBuiltinTool { agent, tool_name }
                if agent == "test-agent" && tool_name == "bassh"
        ));
    }

    #[test]
    fn validate_builtin_tools_ignores_cli_tools() {
        let tools = vec![ToolConfig::Cli {
            name: "anything".to_string(),
            command: "./tool.sh".to_string(),
            readme: None,
            description: None,
        }];
        let warnings = validate_builtin_tools("test-agent", &tools);
        assert!(warnings.is_empty());
    }

    #[tokio::test]
    async fn agent_with_memory_tool_gets_default_hooks() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("hooks-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: hooks-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  tools:
    - type: builtin
      name: memory
    - type: builtin
      name: background_process
"#,
        );

        let agent = load_agent(&agents_dir, "hooks-agent").await.unwrap();

        // memory default: skip_duplicate on memory:recall
        assert!(
            agent
                .hooks
                .before_tool
                .iter()
                .any(|h| h.tool_match == "memory:recall")
        );

        // background_process default: depends_on for send_keys
        assert!(
            agent
                .hooks
                .before_tool
                .iter()
                .any(|h| h.tool_match == "background_process:send_keys")
        );

        // background_process default: after_tool for capture and spawn
        assert!(
            agent
                .hooks
                .after_tool
                .iter()
                .any(|h| h.tool_match == "background_process:capture")
        );
        assert!(
            agent
                .hooks
                .after_tool
                .iter()
                .any(|h| h.tool_match == "background_process:spawn")
        );
    }

    #[tokio::test]
    async fn agent_explicit_hooks_override_defaults() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("override-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: override-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  tools:
    - type: builtin
      name: memory
  hooks:
    before_tool:
      - match: "memory:recall"
        type: skip_duplicate
        match_args: [days, query]
"#,
        );

        let agent = load_agent(&agents_dir, "override-agent").await.unwrap();

        // Only one before_tool hook for memory:recall (agent's override, not default)
        let recall_hooks: Vec<_> = agent
            .hooks
            .before_tool
            .iter()
            .filter(|h| h.tool_match == "memory:recall")
            .collect();
        assert_eq!(recall_hooks.len(), 1);
        assert_eq!(recall_hooks[0].match_args, vec!["days", "query"]);
    }

    #[tokio::test]
    async fn agent_without_tools_gets_no_default_hooks() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("no-tools-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        write_yaml(
            &agent_dir,
            r#"apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: no-tools-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        );

        let agent = load_agent(&agents_dir, "no-tools-agent").await.unwrap();
        assert!(agent.hooks.before_tool.is_empty());
        assert!(agent.hooks.after_tool.is_empty());
    }
}
