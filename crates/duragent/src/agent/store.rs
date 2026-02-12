use std::collections::HashMap;
use std::sync::Arc;

use super::error::{AgentLoadError, AgentLoadWarning};
use super::spec::AgentSpec;
use crate::store::{AgentCatalog, ScanWarning};

// ============================================================================
// Public Types
// ============================================================================

/// Store for loaded agents, shared across request handlers.
#[derive(Debug, Clone)]
pub struct AgentStore {
    agents: Arc<HashMap<String, AgentSpec>>,
}

/// Result of scanning the agents directory.
#[derive(Debug)]
pub struct AgentScanReport {
    pub store: AgentStore,
    pub warnings: Vec<AgentScanWarning>,
}

/// Non-fatal issues encountered while loading agents.
#[derive(Debug)]
pub enum AgentScanWarning {
    /// The agents location doesn't exist.
    AgentsDirMissing { location: String },
    /// Catalog-level error (e.g., failed to load agents).
    CatalogError { error: String },
    /// An agent failed to load.
    InvalidAgent { name: String, error: AgentLoadError },
    /// Warning from loading an agent (e.g., missing resource).
    AgentWarning(AgentLoadWarning),
}

// ============================================================================
// AgentStore Implementation
// ============================================================================

impl AgentStore {
    /// Get an agent by name.
    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.get(name)
    }

    /// Get the number of loaded agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Iterate over all agents.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &AgentSpec)> {
        self.agents.iter()
    }

    /// Load agents from a catalog.
    ///
    /// Accepts any `AgentCatalog` implementation (file-based, in-memory, etc.).
    pub async fn from_catalog(catalog: &dyn AgentCatalog) -> AgentScanReport {
        let result = match catalog.load_all().await {
            Ok(r) => r,
            Err(e) => {
                // Storage error during scan (e.g., read_dir failed)
                return AgentScanReport {
                    store: AgentStore {
                        agents: Arc::new(HashMap::new()),
                    },
                    warnings: vec![AgentScanWarning::CatalogError {
                        error: e.to_string(),
                    }],
                };
            }
        };

        // Convert loaded agents to HashMap by name
        let agents: HashMap<String, AgentSpec> = result
            .agents
            .into_iter()
            .map(|a| (a.metadata.name.clone(), a))
            .collect();

        // Convert ScanWarning to AgentScanWarning
        let warnings = result
            .warnings
            .into_iter()
            .map(|w| match w {
                ScanWarning::AgentsDirMissing { path } => {
                    AgentScanWarning::AgentsDirMissing { location: path }
                }
                ScanWarning::InvalidAgent { name, error } => AgentScanWarning::InvalidAgent {
                    name,
                    error: AgentLoadError::Validation(error),
                },
                ScanWarning::MissingResource { agent, resource } => {
                    AgentScanWarning::AgentWarning(AgentLoadWarning::MissingFile {
                        agent,
                        field: "unknown",
                        location: resource,
                        error: "file not found".to_string(),
                    })
                }
            })
            .collect();

        AgentScanReport {
            store: AgentStore {
                agents: Arc::new(agents),
            },
            warnings,
        }
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Log non-fatal warnings produced by agent loading.
pub fn log_scan_warnings(warnings: &[AgentScanWarning]) {
    use tracing::warn;

    for w in warnings {
        match w {
            AgentScanWarning::AgentsDirMissing { location } => {
                warn!(location = %location, "Agents location does not exist");
            }
            AgentScanWarning::CatalogError { error } => {
                warn!(error = %error, "Failed to load agents from catalog");
            }
            AgentScanWarning::InvalidAgent { name, error } => {
                warn!(agent = %name, error = %error, "Skipping invalid agent");
            }
            AgentScanWarning::AgentWarning(AgentLoadWarning::MissingFile {
                agent,
                field,
                location,
                error,
            }) => {
                warn!(
                    agent = %agent,
                    field = %field,
                    location = %location,
                    error = %error,
                    "Missing referenced agent file"
                );
            }
            AgentScanWarning::AgentWarning(AgentLoadWarning::InvalidSkill {
                agent,
                skill_dir,
                error,
            }) => {
                warn!(
                    agent = %agent,
                    skill_dir = %skill_dir,
                    error = %error,
                    "Invalid skill in agent"
                );
            }
            AgentScanWarning::AgentWarning(AgentLoadWarning::UnknownBuiltinTool {
                agent,
                tool_name,
            }) => {
                warn!(
                    agent = %agent,
                    tool = %tool_name,
                    "Unknown builtin tool in agent config"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::{API_VERSION_V1ALPHA1, KIND_AGENT};
    use crate::store::file::FileAgentCatalog;
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

    async fn scan_agents(agents_dir: &Path) -> AgentScanReport {
        let catalog = FileAgentCatalog::new(agents_dir);
        AgentStore::from_catalog(&catalog).await
    }

    // ==========================================================================
    // AgentStore methods - Happy path
    // ==========================================================================

    #[tokio::test]
    async fn store_get_returns_existing_agent() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();
        create_minimal_agent(&agent_dir, "test-agent");

        let report = scan_agents(&agents_dir).await;
        let agent = report.store.get("test-agent");
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().metadata.name, "test-agent");
    }

    #[tokio::test]
    async fn store_get_returns_none_for_missing() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let report = scan_agents(&agents_dir).await;
        assert!(report.store.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn store_is_empty_returns_true_when_empty() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let report = scan_agents(&agents_dir).await;
        assert!(report.store.is_empty());
        assert_eq!(report.store.len(), 0);
    }

    #[tokio::test]
    async fn store_is_empty_returns_false_when_populated() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();
        create_minimal_agent(&agent_dir, "test-agent");

        let report = scan_agents(&agents_dir).await;
        assert!(!report.store.is_empty());
    }

    #[tokio::test]
    async fn store_iter_yields_all_agents() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        for name in ["alpha", "beta", "gamma"] {
            let agent_dir = agents_dir.join(name);
            std::fs::create_dir(&agent_dir).unwrap();
            create_minimal_agent(&agent_dir, name);
        }

        let report = scan_agents(&agents_dir).await;
        let names: Vec<_> = report.store.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    // ==========================================================================
    // scan() - Directory handling
    // ==========================================================================

    #[tokio::test]
    async fn agent_store_scan_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 0);
        assert!(report.warnings.is_empty());
    }

    #[tokio::test]
    async fn agent_store_scan_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("nonexistent");

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 0);
        assert!(matches!(
            report.warnings.first(),
            Some(AgentScanWarning::AgentsDirMissing { .. })
        ));
    }

    #[tokio::test]
    async fn agent_store_scan_multiple_agents() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let agent1_dir = agents_dir.join("agent-one");
        std::fs::create_dir(&agent1_dir).unwrap();
        create_minimal_agent(&agent1_dir, "agent-one");

        let agent2_dir = agents_dir.join("agent-two");
        std::fs::create_dir(&agent2_dir).unwrap();
        create_minimal_agent(&agent2_dir, "agent-two");

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 2);
        assert!(report.store.get("agent-one").is_some());
        assert!(report.store.get("agent-two").is_some());
    }

    #[tokio::test]
    async fn agent_store_scan_skips_files_in_agents_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        // Create a file (not directory) in agents dir
        std::fs::write(agents_dir.join("README.md"), "# Agents").unwrap();

        // Create a valid agent
        let agent_dir = agents_dir.join("valid-agent");
        std::fs::create_dir(&agent_dir).unwrap();
        create_minimal_agent(&agent_dir, "valid-agent");

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 1);
        assert!(report.warnings.is_empty());
    }

    #[tokio::test]
    async fn agent_store_scan_skips_dirs_without_agent_yaml() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        // Directory without agent.yaml
        let no_yaml_dir = agents_dir.join("not-an-agent");
        std::fs::create_dir(&no_yaml_dir).unwrap();
        std::fs::write(no_yaml_dir.join("README.md"), "not an agent").unwrap();

        // Valid agent
        let agent_dir = agents_dir.join("valid-agent");
        std::fs::create_dir(&agent_dir).unwrap();
        create_minimal_agent(&agent_dir, "valid-agent");

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 1);
        assert!(report.store.get("valid-agent").is_some());
    }

    #[tokio::test]
    async fn agent_store_skips_invalid_agents() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        // Valid agent.
        let valid_dir = agents_dir.join("valid-agent");
        std::fs::create_dir(&valid_dir).unwrap();
        create_minimal_agent(&valid_dir, "valid-agent");

        // Invalid agent.
        let invalid_dir = agents_dir.join("invalid-agent");
        std::fs::create_dir(&invalid_dir).unwrap();
        std::fs::write(
            invalid_dir.join("agent.yaml"),
            r#"apiVersion: duragent/v99
kind: Agent
metadata:
  name: invalid-agent
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
"#,
        )
        .unwrap();

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 1);
        assert!(report.store.get("valid-agent").is_some());
        assert!(report.store.get("invalid-agent").is_none());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| matches!(w, AgentScanWarning::InvalidAgent { .. }))
        );
    }

    #[tokio::test]
    async fn agent_store_skips_malformed_yaml() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let bad_yaml_dir = agents_dir.join("bad-yaml");
        std::fs::create_dir(&bad_yaml_dir).unwrap();
        std::fs::write(
            bad_yaml_dir.join("agent.yaml"),
            "this: is: not: valid: yaml::",
        )
        .unwrap();

        let report = scan_agents(&agents_dir).await;
        assert_eq!(report.store.len(), 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| matches!(w, AgentScanWarning::InvalidAgent { .. }))
        );
    }

    // ==========================================================================
    // AgentScanWarning - Debug/Display coverage
    // ==========================================================================

    #[test]
    fn scan_warning_debug_formats() {
        let warnings = vec![
            AgentScanWarning::AgentsDirMissing {
                location: "/missing".to_string(),
            },
            AgentScanWarning::CatalogError {
                error: "permission denied".to_string(),
            },
            AgentScanWarning::InvalidAgent {
                name: "bad-agent".to_string(),
                error: AgentLoadError::Validation("invalid spec".to_string()),
            },
        ];

        // Just verify Debug is implemented and doesn't panic
        for w in &warnings {
            let _ = format!("{:?}", w);
        }
    }
}
