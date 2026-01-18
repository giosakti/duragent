use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::error::{AgentLoadError, AgentLoadWarning};
use super::provider::Provider;
use super::{API_VERSION_V1ALPHA1, KIND_AGENT};

/// An agent specification loaded from an agent.yaml file.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentMetadata,
    pub model: ModelConfig,
    pub system_prompt: Option<String>,
    pub instructions: Option<String>,
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
    system_prompt: Option<String>,
    instructions: Option<String>,
}

impl AgentSpec {
    /// Load an agent and return non-fatal warnings (e.g., missing referenced markdown files).
    pub fn load_with_warnings(
        agent_dir: &Path,
    ) -> Result<(Self, Vec<AgentLoadWarning>), AgentLoadError> {
        let yaml_path = agent_dir.join("agent.yaml");
        let yaml_content = fs::read_to_string(&yaml_path)?;

        let raw: RawAgentSpec =
            serde_saphyr::from_str(&yaml_content).map_err(AgentLoadError::Yaml)?;

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

        let mut warnings = Vec::new();

        // Load system_prompt markdown if specified
        let system_prompt = if let Some(ref path) = raw.spec.system_prompt {
            let full_path = agent_dir.join(path);
            match fs::read_to_string(&full_path) {
                Ok(content) => Some(content),
                Err(e) => {
                    warnings.push(AgentLoadWarning::MissingFile {
                        agent: raw.metadata.name.clone(),
                        field: "system_prompt",
                        path: full_path,
                        error: e.to_string(),
                    });
                    None
                }
            }
        } else {
            None
        };

        // Load instructions markdown if specified
        let instructions = if let Some(ref path) = raw.spec.instructions {
            let full_path = agent_dir.join(path);
            match fs::read_to_string(&full_path) {
                Ok(content) => Some(content),
                Err(e) => {
                    warnings.push(AgentLoadWarning::MissingFile {
                        agent: raw.metadata.name.clone(),
                        field: "instructions",
                        path: full_path,
                        error: e.to_string(),
                    });
                    None
                }
            }
        } else {
            None
        };

        Ok((
            AgentSpec {
                api_version: raw.api_version,
                kind: raw.kind,
                metadata: raw.metadata,
                model: raw.spec.model,
                system_prompt,
                instructions,
            },
            warnings,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_yaml(dir: &Path, contents: &str) {
        fs::write(dir.join("agent.yaml"), contents).unwrap();
    }

    #[test]
    fn load_minimal_agent() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("test-agent");
        fs::create_dir(&agent_dir).unwrap();

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

        let (agent, warnings) = AgentSpec::load_with_warnings(&agent_dir).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(agent.api_version, API_VERSION_V1ALPHA1);
        assert_eq!(agent.kind, KIND_AGENT);
        assert_eq!(agent.metadata.name, "test-agent");
        assert_eq!(agent.model.provider, Provider::OpenRouter);
        assert_eq!(agent.model.name, "anthropic/claude-sonnet-4");
        assert!(agent.system_prompt.is_none());
        assert!(agent.instructions.is_none());
    }

    #[test]
    fn load_agent_with_system_prompt() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("test-agent");
        fs::create_dir(&agent_dir).unwrap();

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
        fs::write(
            agent_dir.join("SYSTEM_PROMPT.md"),
            "You are a helpful assistant.",
        )
        .unwrap();

        let (agent, _) = AgentSpec::load_with_warnings(&agent_dir).unwrap();
        assert_eq!(agent.metadata.name, "test-agent");
        assert_eq!(agent.metadata.description, Some("A test agent".to_string()));
        assert_eq!(agent.metadata.version, Some("1.0.0".to_string()));
        assert_eq!(
            agent.system_prompt,
            Some("You are a helpful assistant.".to_string())
        );
    }

    #[test]
    fn load_agent_missing_system_prompt_is_warning_not_error() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("test-agent");
        fs::create_dir(&agent_dir).unwrap();

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

        let (agent, warnings) = AgentSpec::load_with_warnings(&agent_dir).unwrap();
        assert!(agent.system_prompt.is_none());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn load_agent_invalid_api_version() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("test-agent");
        fs::create_dir(&agent_dir).unwrap();

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

        let err = AgentSpec::load_with_warnings(&agent_dir).unwrap_err();
        assert!(matches!(err, AgentLoadError::Validation(_)));
    }

    #[test]
    fn load_agent_with_all_model_options() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("test-agent");
        fs::create_dir(&agent_dir).unwrap();

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

        let (agent, _) = AgentSpec::load_with_warnings(&agent_dir).unwrap();
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
}
