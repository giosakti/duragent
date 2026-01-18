/// Error type for agent loading operations.
#[derive(Debug)]
pub enum AgentLoadError {
    Io(std::io::Error),
    Yaml(serde_saphyr::Error),
    Validation(String),
}

impl std::fmt::Display for AgentLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentLoadError::Io(e) => write!(f, "IO error: {e}"),
            AgentLoadError::Yaml(e) => write!(f, "YAML parse error: {e}"),
            AgentLoadError::Validation(e) => write!(f, "Validation error: {e}"),
        }
    }
}

impl std::error::Error for AgentLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AgentLoadError::Io(e) => Some(e),
            AgentLoadError::Yaml(e) => Some(e),
            AgentLoadError::Validation(_) => None,
        }
    }
}

impl From<std::io::Error> for AgentLoadError {
    fn from(e: std::io::Error) -> Self {
        AgentLoadError::Io(e)
    }
}

/// Non-fatal issues encountered while loading an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLoadWarning {
    MissingFile {
        agent: String,
        field: &'static str,
        path: std::path::PathBuf,
        error: String,
    },
}
