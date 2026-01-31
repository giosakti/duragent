use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;

use super::error::{AgentLoadError, AgentLoadWarning};
use super::spec::AgentSpec;

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

/// Non-fatal issues encountered while scanning the agents directory.
#[derive(Debug)]
pub enum AgentScanWarning {
    AgentsDirMissing {
        path: PathBuf,
    },
    AgentsDirReadError {
        path: PathBuf,
        error: String,
    },
    EntryReadError {
        path: PathBuf,
        error: String,
    },
    InvalidAgent {
        path: PathBuf,
        error: AgentLoadError,
    },
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

    /// Scan a directory for agent subdirectories and load all valid agents.
    pub async fn scan(agents_dir: &Path) -> AgentScanReport {
        let mut agents = HashMap::new();
        let mut warnings = Vec::new();

        // Check if directory exists using async metadata
        let dir_exists = fs::metadata(agents_dir).await.is_ok();
        if !dir_exists {
            warnings.push(AgentScanWarning::AgentsDirMissing {
                path: agents_dir.to_path_buf(),
            });
            return AgentScanReport {
                store: AgentStore {
                    agents: Arc::new(agents),
                },
                warnings,
            };
        }

        let mut read_dir = match fs::read_dir(agents_dir).await {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(AgentScanWarning::AgentsDirReadError {
                    path: agents_dir.to_path_buf(),
                    error: e.to_string(),
                });
                return AgentScanReport {
                    store: AgentStore {
                        agents: Arc::new(agents),
                    },
                    warnings,
                };
            }
        };

        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(e) => {
                    warnings.push(AgentScanWarning::EntryReadError {
                        path: agents_dir.to_path_buf(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();

            // Check if it's a directory using async metadata
            let is_dir = match fs::metadata(&path).await {
                Ok(m) => m.is_dir(),
                Err(_) => false,
            };
            if !is_dir {
                continue;
            }

            let yaml_path = path.join("agent.yaml");
            let yaml_exists = fs::metadata(&yaml_path).await.is_ok();
            if !yaml_exists {
                continue;
            }

            match AgentSpec::load_with_warnings(&path).await {
                Ok((agent, agent_warnings)) => {
                    let name = agent.metadata.name.clone();
                    agents.insert(name, agent);
                    for w in agent_warnings {
                        warnings.push(AgentScanWarning::AgentWarning(w));
                    }
                }
                Err(e) => {
                    warnings.push(AgentScanWarning::InvalidAgent {
                        path: path.to_path_buf(),
                        error: e,
                    });
                }
            }
        }

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

/// Resolve `agents_dir` to an absolute (or at least fully joined) path.
///
/// If `agents_dir` is relative, it is resolved relative to the config file directory.
pub fn resolve_agents_dir(config_path: &Path, agents_dir: &Path) -> PathBuf {
    crate::config::resolve_path(config_path, agents_dir)
}

/// Log non-fatal warnings produced by agent scanning.
pub fn log_scan_warnings(warnings: &[AgentScanWarning]) {
    use tracing::warn;

    for w in warnings {
        match w {
            AgentScanWarning::AgentsDirMissing { path } => {
                warn!(path = %path.display(), "Agents directory does not exist");
            }
            AgentScanWarning::AgentsDirReadError { path, error } => {
                warn!(path = %path.display(), error = %error, "Failed to read agents directory");
            }
            AgentScanWarning::EntryReadError { path, error } => {
                warn!(path = %path.display(), error = %error, "Failed to read directory entry");
            }
            AgentScanWarning::InvalidAgent { path, error } => {
                warn!(path = %path.display(), error = %error, "Skipping invalid agent");
            }
            AgentScanWarning::AgentWarning(AgentLoadWarning::MissingFile {
                agent,
                field,
                path,
                error,
            }) => {
                warn!(
                    agent = %agent,
                    field = %field,
                    path = %path.display(),
                    error = %error,
                    "Missing referenced agent file"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{API_VERSION_V1ALPHA1, KIND_AGENT};
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

        let report = AgentStore::scan(&agents_dir).await;
        let agent = report.store.get("test-agent");
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().metadata.name, "test-agent");
    }

    #[tokio::test]
    async fn store_get_returns_none_for_missing() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let report = AgentStore::scan(&agents_dir).await;
        assert!(report.store.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn store_is_empty_returns_true_when_empty() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
        assert_eq!(report.store.len(), 0);
        assert!(report.warnings.is_empty());
    }

    #[tokio::test]
    async fn agent_store_scan_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("nonexistent");

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
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
            r#"apiVersion: agnx/v99
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

        let report = AgentStore::scan(&agents_dir).await;
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

        let report = AgentStore::scan(&agents_dir).await;
        assert_eq!(report.store.len(), 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| matches!(w, AgentScanWarning::InvalidAgent { .. }))
        );
    }

    // ==========================================================================
    // resolve_agents_dir()
    // ==========================================================================

    #[test]
    fn resolve_agents_dir_absolute_path_unchanged() {
        let result = resolve_agents_dir(Path::new("/etc/config.yaml"), Path::new("/opt/agents"));
        assert_eq!(result, PathBuf::from("/opt/agents"));
    }

    #[test]
    fn resolve_agents_dir_relative_path_resolved() {
        let result = resolve_agents_dir(Path::new("/etc/agnx/config.yaml"), Path::new("./agents"));
        assert_eq!(result, PathBuf::from("/etc/agnx/agents"));
    }

    // ==========================================================================
    // AgentScanWarning - Debug/Display coverage
    // ==========================================================================

    #[test]
    fn scan_warning_debug_formats() {
        let warnings = vec![
            AgentScanWarning::AgentsDirMissing {
                path: PathBuf::from("/missing"),
            },
            AgentScanWarning::AgentsDirReadError {
                path: PathBuf::from("/unreadable"),
                error: "permission denied".to_string(),
            },
            AgentScanWarning::EntryReadError {
                path: PathBuf::from("/dir"),
                error: "io error".to_string(),
            },
        ];

        // Just verify Debug is implemented and doesn't panic
        for w in &warnings {
            let _ = format!("{:?}", w);
        }
    }
}
