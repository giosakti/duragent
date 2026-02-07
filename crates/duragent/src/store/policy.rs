//! Policy storage trait.
//!
//! Defines the interface for persisting tool policies.

use async_trait::async_trait;

use crate::agent::ToolPolicy;

use super::error::StorageResult;

/// Storage interface for tool policy persistence.
///
/// Policies support a base + local override model where the base policy
/// ships with the agent and local overrides are user-configurable.
#[async_trait]
pub trait PolicyStore: Send + Sync {
    /// Load the tool policy for an agent.
    ///
    /// Merges base policy with local overrides.
    /// Returns a default policy if no policy is configured.
    async fn load(&self, agent_name: &str) -> ToolPolicy;

    /// Save local policy overrides for an agent.
    ///
    /// Only modifies local overrides, never the base policy.
    /// Must be atomic - either fully succeeds or has no effect.
    async fn save(&self, agent_name: &str, policy: &ToolPolicy) -> StorageResult<()>;
}
