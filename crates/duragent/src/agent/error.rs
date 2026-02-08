//! Agent error types.

use thiserror::Error;

/// Error type for agent loading operations.
#[derive(Debug, Error)]
pub enum AgentLoadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_saphyr::Error),

    #[error("validation error: {0}")]
    Validation(String),
}

/// Non-fatal issues encountered while loading an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLoadWarning {
    /// A referenced file could not be loaded.
    MissingFile {
        agent: String,
        field: &'static str,
        location: String,
        error: String,
    },
    /// A skill directory contained an invalid SKILL.md.
    InvalidSkill {
        agent: String,
        skill_dir: String,
        error: String,
    },
}
