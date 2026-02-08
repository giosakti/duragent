//! File-based policy storage implementation.
//!
//! Loads and saves tool policies from YAML files.
//! - `policy.yaml` - base policy (shipped with agent)
//! - `policy.local.yaml` - local overrides (user additions)

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::agent::ToolPolicy;
use crate::store::error::{StorageError, StorageResult};
use crate::store::policy::PolicyStore;

/// File-based implementation of `PolicyStore`.
///
/// Policies are loaded from agent directories with support for
/// base + local override merging.
#[derive(Debug, Clone)]
pub struct FilePolicyStore {
    agents_dir: PathBuf,
}

impl FilePolicyStore {
    /// Create a new file policy store.
    pub fn new(agents_dir: impl Into<PathBuf>) -> Self {
        Self {
            agents_dir: agents_dir.into(),
        }
    }

    /// Get the base policy file path.
    fn base_path(&self, agent_name: &str) -> PathBuf {
        self.agents_dir.join(agent_name).join("policy.yaml")
    }

    /// Get the local policy file path.
    fn local_path(&self, agent_name: &str) -> PathBuf {
        self.agents_dir.join(agent_name).join("policy.local.yaml")
    }

    /// Load a single policy file.
    async fn load_file(path: &Path) -> Option<ToolPolicy> {
        let content = fs::read_to_string(path).await.ok()?;
        match serde_saphyr::from_str(&content) {
            Ok(policy) => Some(policy),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse policy file");
                None
            }
        }
    }
}

#[async_trait]
impl PolicyStore for FilePolicyStore {
    async fn load(&self, agent_name: &str) -> ToolPolicy {
        let base = Self::load_file(&self.base_path(agent_name)).await;
        let local = Self::load_file(&self.local_path(agent_name)).await;

        match (base, local) {
            (Some(base), Some(local)) => base.merge(local),
            (Some(base), None) => base,
            (None, Some(local)) => local,
            (None, None) => ToolPolicy::default(),
        }
    }

    async fn save(&self, agent_name: &str, policy: &ToolPolicy) -> StorageResult<()> {
        let agent_dir = self.agents_dir.join(agent_name);
        let path = self.local_path(agent_name);
        let tmp_path = agent_dir.join("policy.local.yaml.tmp");

        let content = serde_saphyr::to_string(policy)
            .map_err(|e| StorageError::serialization(e.to_string()))?;

        super::atomic_write_file(&tmp_path, &path, content.as_bytes()).await?;

        tracing::debug!(path = %path.display(), "saved local policy");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::PolicyMode;
    use tempfile::TempDir;

    #[tokio::test]
    async fn load_default_when_no_files() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        std::fs::create_dir(agents_dir.join("test-agent")).unwrap();

        let store = FilePolicyStore::new(&agents_dir);
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Dangerous);
        assert!(policy.allow.is_empty());
        assert!(policy.deny.is_empty());
    }

    #[tokio::test]
    async fn load_base_policy() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        std::fs::write(
            agent_dir.join("policy.yaml"),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: restrict
deny:
  - "bash:rm*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir);
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Restrict);
        assert_eq!(policy.deny, vec!["bash:rm*"]);
    }

    #[tokio::test]
    async fn load_and_merge_local() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        std::fs::write(
            agent_dir.join("policy.yaml"),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: restrict
deny:
  - "bash:rm*"
"#,
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("policy.local.yaml"),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
allow:
  - "bash:echo*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir);
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Ask);
        assert_eq!(policy.deny, vec!["bash:rm*"]);
        assert_eq!(policy.allow, vec!["bash:echo*"]);
    }

    #[tokio::test]
    async fn save_creates_local_file() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        let store = FilePolicyStore::new(&agents_dir);

        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:echo*".to_string()],
            ..Default::default()
        };

        store.save("test-agent", &policy).await.unwrap();

        let content = std::fs::read_to_string(agent_dir.join("policy.local.yaml")).unwrap();
        assert!(content.contains("ask"));
        assert!(content.contains("echo*"));
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        let store = FilePolicyStore::new(&agents_dir);

        let policy1 = ToolPolicy {
            allow: vec!["bash:ls*".to_string()],
            ..Default::default()
        };
        store.save("test-agent", &policy1).await.unwrap();

        let policy2 = ToolPolicy {
            allow: vec!["bash:cat*".to_string()],
            ..Default::default()
        };
        store.save("test-agent", &policy2).await.unwrap();

        let loaded = store.load("test-agent").await;
        assert_eq!(loaded.allow, vec!["bash:cat*"]);
    }
}
