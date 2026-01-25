use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::error::{AgentLoadError, AgentLoadWarning};
use super::spec::AgentSpec;

/// Store for loaded agents, shared across request handlers.
#[derive(Debug, Clone)]
pub struct AgentStore {
    agents: Arc<HashMap<String, AgentSpec>>,
}

impl AgentStore {
    /// Scan a directory for agent subdirectories and load all valid agents.
    pub fn scan(agents_dir: &Path) -> AgentScanReport {
        let mut agents = HashMap::new();
        let mut warnings = Vec::new();

        if !agents_dir.exists() {
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

        let entries = match fs::read_dir(agents_dir) {
            Ok(e) => e,
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

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let yaml_path = path.join("agent.yaml");
            if !yaml_path.exists() {
                continue;
            }

            match AgentSpec::load_with_warnings(&path) {
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

    /// Get an agent by name.
    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.get(name)
    }
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
    InvalidAgent {
        path: PathBuf,
        error: AgentLoadError,
    },
    AgentWarning(AgentLoadWarning),
}

#[derive(Debug)]
pub struct AgentScanReport {
    pub store: AgentStore,
    pub warnings: Vec<AgentScanWarning>,
}

/// Resolve `agents_dir` to an absolute (or at least fully joined) path.
///
/// If `agents_dir` is relative, it is resolved relative to the config file directory.
pub fn resolve_agents_dir(config_path: &Path, agents_dir: &Path) -> PathBuf {
    if agents_dir.is_absolute() {
        return agents_dir.to_path_buf();
    }

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    config_dir.join(agents_dir)
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
        fs::write(dir.join("agent.yaml"), yaml).unwrap();
    }

    #[test]
    fn agent_store_scan_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).unwrap();

        let report = AgentStore::scan(&agents_dir);
        assert_eq!(report.store.len(), 0);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn agent_store_scan_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("nonexistent");

        let report = AgentStore::scan(&agents_dir);
        assert_eq!(report.store.len(), 0);
        assert!(matches!(
            report.warnings.first(),
            Some(AgentScanWarning::AgentsDirMissing { .. })
        ));
    }

    #[test]
    fn agent_store_scan_multiple_agents() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).unwrap();

        let agent1_dir = agents_dir.join("agent-one");
        fs::create_dir(&agent1_dir).unwrap();
        create_minimal_agent(&agent1_dir, "agent-one");

        let agent2_dir = agents_dir.join("agent-two");
        fs::create_dir(&agent2_dir).unwrap();
        create_minimal_agent(&agent2_dir, "agent-two");

        let report = AgentStore::scan(&agents_dir);
        assert_eq!(report.store.len(), 2);
        assert!(report.store.get("agent-one").is_some());
        assert!(report.store.get("agent-two").is_some());
    }

    #[test]
    fn agent_store_skips_invalid_agents() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir(&agents_dir).unwrap();

        // Valid agent.
        let valid_dir = agents_dir.join("valid-agent");
        fs::create_dir(&valid_dir).unwrap();
        create_minimal_agent(&valid_dir, "valid-agent");

        // Invalid agent.
        let invalid_dir = agents_dir.join("invalid-agent");
        fs::create_dir(&invalid_dir).unwrap();
        fs::write(
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

        let report = AgentStore::scan(&agents_dir);
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
}
