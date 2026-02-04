//! File-based agent catalog implementation.
//!
//! Loads agent specifications from YAML files in agent directories.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use super::policy::FilePolicyStore;
use crate::agent::{AgentLoadWarning, AgentSpec, LoadedAgentFiles};
use crate::store::PolicyStore;
use crate::store::agent::{AgentCatalog, AgentScanResult, ScanWarning};
use crate::store::error::{StorageError, StorageResult};

/// File-based implementation of `AgentCatalog`.
///
/// Scans a directory for agent subdirectories, each containing an `agent.yaml` file.
#[derive(Debug, Clone)]
pub struct FileAgentCatalog {
    agents_dir: PathBuf,
}

impl FileAgentCatalog {
    /// Create a new file agent catalog.
    pub fn new(agents_dir: impl Into<PathBuf>) -> Self {
        Self {
            agents_dir: agents_dir.into(),
        }
    }
}

#[async_trait]
impl AgentCatalog for FileAgentCatalog {
    async fn load_all(&self) -> StorageResult<AgentScanResult> {
        let mut agents = Vec::new();
        let mut warnings = Vec::new();

        // Check if directory exists
        if fs::metadata(&self.agents_dir).await.is_err() {
            warnings.push(ScanWarning::AgentsDirMissing {
                path: self.agents_dir.display().to_string(),
            });
            return Ok(AgentScanResult { agents, warnings });
        }

        let policy_store = FilePolicyStore::new(&self.agents_dir);

        let mut entries = match fs::read_dir(&self.agents_dir).await {
            Ok(e) => e,
            Err(e) => return Err(StorageError::file_io(&self.agents_dir, e)),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| StorageError::file_io(&self.agents_dir, e))?
        {
            let path = entry.path();

            // Skip non-directories
            let is_dir = match fs::metadata(&path).await {
                Ok(m) => m.is_dir(),
                Err(_) => false,
            };
            if !is_dir {
                continue;
            }

            // Skip directories without agent.yaml
            let yaml_path = path.join("agent.yaml");
            if fs::metadata(&yaml_path).await.is_err() {
                continue;
            }

            // Get agent name for policy loading
            let agent_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            // Load policy for this agent
            let policy = policy_store.load(&agent_name).await;

            // Try to load the agent
            match load_agent_from_dir(&path, &agent_name, policy).await {
                Ok((agent, agent_warnings)) => {
                    agents.push(agent);

                    // Convert agent warnings to scan warnings
                    for w in agent_warnings {
                        match w {
                            AgentLoadWarning::MissingFile {
                                agent, location, ..
                            } => {
                                warnings.push(ScanWarning::MissingResource {
                                    agent,
                                    resource: location,
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    warnings.push(ScanWarning::InvalidAgent {
                        name: agent_name,
                        error: e.to_string(),
                    });
                }
            }
        }

        Ok(AgentScanResult { agents, warnings })
    }

    async fn load(&self, name: &str) -> StorageResult<AgentSpec> {
        let agent_dir = self.agents_dir.join(name);
        let yaml_path = agent_dir.join("agent.yaml");

        if fs::metadata(&yaml_path).await.is_err() {
            return Err(StorageError::not_found("agent", name));
        }

        // Load policy for this agent
        let policy_store = FilePolicyStore::new(&self.agents_dir);
        let policy = policy_store.load(name).await;

        let (agent, _warnings) = load_agent_from_dir(&agent_dir, name, policy)
            .await
            .map_err(|e| StorageError::file_deserialization(&agent_dir, e.to_string()))?;

        Ok(agent)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Load an agent from a directory, reading all referenced files.
///
/// Returns the loaded agent and any warnings about missing optional files.
async fn load_agent_from_dir(
    agent_dir: &Path,
    agent_name: &str,
    policy: crate::agent::ToolPolicy,
) -> Result<(AgentSpec, Vec<AgentLoadWarning>), crate::agent::AgentLoadError> {
    let yaml_path = agent_dir.join("agent.yaml");

    // Read agent.yaml
    let yaml_content = fs::read_to_string(&yaml_path).await?;

    // Parse file references from YAML
    let refs = AgentSpec::parse_file_refs(&yaml_content)?;

    // Load optional files, collecting warnings for missing ones
    let mut warnings = Vec::new();

    let soul = load_optional_file(agent_dir, &refs.soul, agent_name, "soul", &mut warnings).await;
    let system_prompt = load_optional_file(
        agent_dir,
        &refs.system_prompt,
        agent_name,
        "system_prompt",
        &mut warnings,
    )
    .await;
    let instructions = load_optional_file(
        agent_dir,
        &refs.instructions,
        agent_name,
        "instructions",
        &mut warnings,
    )
    .await;

    let files = LoadedAgentFiles {
        soul,
        system_prompt,
        instructions,
    };

    let agent = AgentSpec::from_yaml(&yaml_content, files, policy, agent_dir.to_path_buf())?;

    Ok((agent, warnings))
}

/// Load an optional file referenced in agent.yaml.
///
/// If the file path is None or the file doesn't exist, returns None.
/// Adds a warning to the list if the file is referenced but missing.
async fn load_optional_file(
    agent_dir: &Path,
    file_ref: &Option<String>,
    agent_name: &str,
    field: &'static str,
    warnings: &mut Vec<AgentLoadWarning>,
) -> Option<String> {
    let path_str = file_ref.as_ref()?;
    let path = agent_dir.join(path_str);

    match fs::read_to_string(&path).await {
        Ok(content) => Some(content),
        Err(e) => {
            warnings.push(AgentLoadWarning::MissingFile {
                agent: agent_name.to_string(),
                field,
                location: path_str.clone(),
                error: e.to_string(),
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{API_VERSION_V1ALPHA1, KIND_AGENT};
    use std::path::Path;
    use tempfile::TempDir;

    fn create_minimal_agent(dir: &Path, name: &str) {
        let yaml = format!(
            r#"apiVersion: {API_VERSION_V1ALPHA1}
kind: {KIND_AGENT}
metadata:
  name: {name}
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#
        );
        std::fs::write(dir.join("agent.yaml"), yaml).unwrap();
    }

    #[tokio::test]
    async fn load_all_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let catalog = FileAgentCatalog::new(&agents_dir);
        let result = catalog.load_all().await.unwrap();

        assert!(result.agents.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn load_all_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("nonexistent");

        let catalog = FileAgentCatalog::new(&agents_dir);
        let result = catalog.load_all().await.unwrap();

        assert!(result.agents.is_empty());
        assert!(matches!(
            result.warnings.first(),
            Some(ScanWarning::AgentsDirMissing { .. })
        ));
    }

    #[tokio::test]
    async fn load_all_multiple_agents() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        for name in ["agent-a", "agent-b", "agent-c"] {
            let agent_dir = agents_dir.join(name);
            std::fs::create_dir(&agent_dir).unwrap();
            create_minimal_agent(&agent_dir, name);
        }

        let catalog = FileAgentCatalog::new(&agents_dir);
        let result = catalog.load_all().await.unwrap();

        assert_eq!(result.agents.len(), 3);
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn load_all_skips_invalid() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        // Valid agent
        let valid_dir = agents_dir.join("valid");
        std::fs::create_dir(&valid_dir).unwrap();
        create_minimal_agent(&valid_dir, "valid");

        // Invalid agent
        let invalid_dir = agents_dir.join("invalid");
        std::fs::create_dir(&invalid_dir).unwrap();
        std::fs::write(invalid_dir.join("agent.yaml"), "not: valid: yaml::").unwrap();

        let catalog = FileAgentCatalog::new(&agents_dir);
        let result = catalog.load_all().await.unwrap();

        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].metadata.name, "valid");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| matches!(w, ScanWarning::InvalidAgent { .. }))
        );
    }

    #[tokio::test]
    async fn load_single_agent() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();
        create_minimal_agent(&agent_dir, "test-agent");

        let catalog = FileAgentCatalog::new(&agents_dir);
        let agent = catalog.load("test-agent").await.unwrap();

        assert_eq!(agent.metadata.name, "test-agent");
    }

    #[tokio::test]
    async fn load_nonexistent_agent() {
        use crate::store::StorageError;

        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let catalog = FileAgentCatalog::new(&agents_dir);
        let result = catalog.load("nonexistent").await;

        assert!(matches!(result, Err(StorageError::NotFound { .. })));
    }
}
