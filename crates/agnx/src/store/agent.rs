//! Agent catalog storage trait.
//!
//! Defines the interface for loading agent specifications.

use async_trait::async_trait;

use crate::agent::AgentSpec;

use super::error::StorageResult;

/// Warning encountered while scanning agents.
#[derive(Debug, Clone)]
pub enum ScanWarning {
    /// The agents directory doesn't exist.
    AgentsDirMissing { path: String },
    /// An agent failed to load.
    InvalidAgent { name: String, error: String },
    /// A referenced resource is missing (non-fatal).
    MissingResource { agent: String, resource: String },
}

impl std::fmt::Display for ScanWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentsDirMissing { path } => write!(f, "agents directory missing: {}", path),
            Self::InvalidAgent { name, error } => write!(f, "invalid agent '{}': {}", name, error),
            Self::MissingResource { agent, resource } => {
                write!(f, "agent '{}' missing resource: {}", agent, resource)
            }
        }
    }
}

/// Result of scanning for agents.
pub struct AgentScanResult {
    /// Successfully loaded agents.
    pub agents: Vec<AgentSpec>,
    /// Warnings encountered during scan.
    pub warnings: Vec<ScanWarning>,
}

/// Storage interface for agent catalog operations.
///
/// The catalog is read-only at runtime; agents are loaded during startup or reload.
#[async_trait]
pub trait AgentCatalog: Send + Sync {
    /// Scan and load all agent specifications.
    ///
    /// Returns loaded agents and any warnings encountered.
    /// Invalid agents are skipped with warnings rather than failing the scan.
    async fn load_all(&self) -> StorageResult<AgentScanResult>;

    /// Load a single agent by name.
    async fn load(&self, name: &str) -> StorageResult<AgentSpec>;
}
