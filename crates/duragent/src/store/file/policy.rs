//! File-based policy storage implementation.
//!
//! Loads and saves tool policies from YAML files with a 3-tier merge hierarchy:
//! 1. `<workspace>/policy.yaml` — workspace-level shared policy
//! 2. `<agents>/<name>/policy.yaml` — agent base policy
//! 3. `<agents>/<name>/policy.local.yaml` — user local overrides
//!
//! Merge semantics per field:
//! - **deny**: union across all tiers (security accumulates, cannot be removed)
//! - **mode**: most specific explicit tier wins; inherited if not set
//! - **allow**: union across all tiers (like deny)
//! - **notify**: union across all tiers

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::agent::{Delivery, NotifyConfig, PolicyMode, ToolPolicy};
use crate::store::error::{StorageError, StorageResult};
use crate::store::policy::PolicyStore;

// ============================================================================
// Constants
// ============================================================================

/// Policy file name (used at workspace and agent level).
const POLICY_FILE: &str = "policy.yaml";

/// Local policy override file name (agent level only).
const POLICY_LOCAL_FILE: &str = "policy.local.yaml";

// ============================================================================
// FilePolicyStore
// ============================================================================

/// File-based implementation of `PolicyStore`.
///
/// Policies are loaded with a 3-tier merge hierarchy:
/// 1. **Workspace** — shared deny list (`<workspace>/policy.yaml`)
/// 2. **Agent base** — mode + agent-specific rules (`<agents>/<name>/policy.yaml`)
/// 3. **Agent local** — user overrides (`<agents>/<name>/policy.local.yaml`)
///
/// Merge semantics:
/// - **deny**: union across all tiers (security accumulates)
/// - **mode**: most specific explicit tier wins; inherited if not set
/// - **allow**: union across all tiers (like deny)
/// - **notify**: union across all tiers
#[derive(Debug, Clone)]
pub struct FilePolicyStore {
    agents_dir: PathBuf,
    workspace_dir: Option<PathBuf>,
}

impl FilePolicyStore {
    /// Create a new file policy store.
    ///
    /// If `workspace_dir` is provided, workspace-level `policy.yaml` is loaded
    /// as the base before agent-level policies are merged on top.
    pub fn new(agents_dir: impl Into<PathBuf>, workspace_dir: Option<PathBuf>) -> Self {
        Self {
            agents_dir: agents_dir.into(),
            workspace_dir,
        }
    }
}

#[async_trait]
impl PolicyStore for FilePolicyStore {
    async fn load(&self, agent_name: &str) -> ToolPolicy {
        let workspace = match self.workspace_path() {
            Some(path) => Self::load_raw_file(&path).await,
            None => None,
        };
        let base = Self::load_raw_file(&self.base_path(agent_name)).await;
        let local = Self::load_raw_file(&self.local_path(agent_name)).await;

        resolve_tiers(workspace, base, local)
    }

    async fn save(&self, agent_name: &str, policy: &ToolPolicy) -> StorageResult<()> {
        let path = self.local_path(agent_name);

        // Collect patterns already present in workspace and base tiers
        // so we only persist patterns that are truly local additions.
        let mut inherited_deny: Vec<String> = Vec::new();
        let mut inherited_allow: Vec<String> = Vec::new();
        let mut inherited_notify = NotifyConfig::default();
        if let Some(ws_path) = self.workspace_path()
            && let Some(ws) = Self::load_raw_file(&ws_path).await
        {
            inherited_deny.extend(ws.deny);
            inherited_allow.extend(ws.allow.unwrap_or_default());
            if let Some(ref notify) = ws.notify {
                merge_notify(&mut inherited_notify, notify);
            }
        }
        if let Some(base) = Self::load_raw_file(&self.base_path(agent_name)).await {
            inherited_deny.extend(base.deny);
            inherited_allow.extend(base.allow.unwrap_or_default());
            if let Some(ref notify) = base.notify {
                merge_notify(&mut inherited_notify, notify);
            }
        }

        let local_deny: Vec<String> = policy
            .deny
            .iter()
            .filter(|d| !inherited_deny.contains(d))
            .cloned()
            .collect();

        let local_allow: Vec<String> = policy
            .allow
            .iter()
            .filter(|a| !inherited_allow.contains(a))
            .cloned()
            .collect();

        // Save as RawPolicy: omit mode so it's inherited from parent tiers on
        // next load. This prevents add_pattern_and_save from "freezing" the
        // current mode into the local file.
        let local_notify_enabled = policy.notify.enabled && !inherited_notify.enabled;
        let local_notify_patterns: Vec<String> = policy
            .notify
            .patterns
            .iter()
            .filter(|p| !inherited_notify.patterns.contains(p))
            .cloned()
            .collect();
        let local_notify_deliveries: Vec<Delivery> = policy
            .notify
            .deliveries
            .iter()
            .filter(|d| !inherited_notify.deliveries.contains(d))
            .cloned()
            .collect();
        let local_notify = if local_notify_enabled
            || !local_notify_patterns.is_empty()
            || !local_notify_deliveries.is_empty()
        {
            Some(NotifyConfig {
                enabled: local_notify_enabled,
                patterns: local_notify_patterns,
                deliveries: local_notify_deliveries,
            })
        } else {
            None
        };

        let raw = RawPolicy {
            mode: None,
            deny: local_deny,
            allow: if local_allow.is_empty() {
                None
            } else {
                Some(local_allow)
            },
            notify: local_notify,
        };

        let content = serde_saphyr::to_string(&raw)
            .map_err(|e| StorageError::serialization(e.to_string()))?;

        super::atomic_write_file(&path, content.as_bytes()).await?;

        tracing::debug!(path = %path.display(), "saved local policy");
        Ok(())
    }
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Raw policy as deserialized from a single YAML file.
///
/// Uses `Option` for fields where "not set" vs "explicitly set to default" matters.
/// - `mode: None` → inherit from parent tier
/// - `allow: None` → inherit from parent tier
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<PolicyMode>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    notify: Option<NotifyConfig>,
}

/// Resolve three tiers of raw policies into a single effective ToolPolicy.
fn resolve_tiers(
    workspace: Option<RawPolicy>,
    base: Option<RawPolicy>,
    local: Option<RawPolicy>,
) -> ToolPolicy {
    let tiers = [workspace, base, local];
    let mut policy = ToolPolicy::default();
    let mut seen_deny: HashSet<String> = HashSet::new();
    let mut seen_allow: HashSet<String> = HashSet::new();

    // Mode: most specific explicit wins
    for tier in tiers.iter().rev().flatten() {
        if let Some(ref mode) = tier.mode {
            policy.mode = mode.clone();
            break;
        }
    }

    // Deny: union across all tiers (security accumulates)
    for tier in tiers.iter().flatten() {
        for pattern in &tier.deny {
            if seen_deny.insert(pattern.clone()) {
                policy.deny.push(pattern.clone());
            }
        }
    }

    // Allow: union across all tiers (like deny)
    for tier in tiers.iter().flatten() {
        if let Some(ref allow) = tier.allow {
            for pattern in allow {
                if seen_allow.insert(pattern.clone()) {
                    policy.allow.push(pattern.clone());
                }
            }
        }
    }

    // Notify: union across all tiers
    for tier in tiers.iter().flatten() {
        if let Some(ref notify) = tier.notify {
            merge_notify(&mut policy.notify, notify);
        }
    }

    policy
}

fn merge_notify(target: &mut NotifyConfig, notify: &NotifyConfig) {
    target.enabled = target.enabled || notify.enabled;
    let mut seen_patterns: HashSet<String> = target.patterns.iter().cloned().collect();
    for pattern in &notify.patterns {
        if seen_patterns.insert(pattern.clone()) {
            target.patterns.push(pattern.clone());
        }
    }
    for delivery in &notify.deliveries {
        if !target.deliveries.contains(delivery) {
            target.deliveries.push(delivery.clone());
        }
    }
}

impl FilePolicyStore {
    fn workspace_path(&self) -> Option<PathBuf> {
        self.workspace_dir.as_ref().map(|dir| dir.join(POLICY_FILE))
    }

    fn base_path(&self, agent_name: &str) -> PathBuf {
        self.agents_dir.join(agent_name).join(POLICY_FILE)
    }

    fn local_path(&self, agent_name: &str) -> PathBuf {
        self.agents_dir.join(agent_name).join(POLICY_LOCAL_FILE)
    }

    async fn load_raw_file(path: &Path) -> Option<RawPolicy> {
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

        let store = FilePolicyStore::new(&agents_dir, None);
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
            agent_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: restrict
deny:
  - "bash:rm*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, None);
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
            agent_dir.join(POLICY_FILE),
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
            agent_dir.join(POLICY_LOCAL_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
allow:
  - "bash:echo*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, None);
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

        let store = FilePolicyStore::new(&agents_dir, None);

        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:echo*".to_string()],
            ..Default::default()
        };

        store.save("test-agent", &policy).await.unwrap();

        // Mode should NOT be saved (inherited from parent tiers)
        let content = std::fs::read_to_string(agent_dir.join(POLICY_LOCAL_FILE)).unwrap();
        assert!(!content.contains("ask"));
        assert!(content.contains("echo*"));
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        let store = FilePolicyStore::new(&agents_dir, None);

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

    // -----------------------------------------------------------------------
    // 3-tier workspace merge
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn three_way_merge_unions_deny_and_allow() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: shared deny + default allow
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
deny:
  - "bash:rm -rf*"
  - "*:*sudo*"
allow:
  - "bash:ls*"
"#,
        )
        .unwrap();

        // Agent: mode + agent-specific allow (unions with workspace allow)
        std::fs::write(
            agent_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
allow:
  - "bash:cargo*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Ask);
        // Deny: union from workspace
        assert!(policy.deny.contains(&"bash:rm -rf*".to_string()));
        assert!(policy.deny.contains(&"*:*sudo*".to_string()));
        // Allow: union from workspace + agent
        assert!(policy.allow.contains(&"bash:ls*".to_string()));
        assert!(policy.allow.contains(&"bash:cargo*".to_string()));
        assert_eq!(policy.allow.len(), 2);
    }

    #[tokio::test]
    async fn allow_inherited_when_not_set() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: allow=[ls*]
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
allow:
  - "bash:ls*"
"#,
        )
        .unwrap();

        // Agent: sets mode only, no allow → inherits workspace allow
        std::fs::write(
            agent_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Ask);
        assert_eq!(policy.allow, vec!["bash:ls*"]);
    }

    #[tokio::test]
    async fn mode_explicit_dangerous_overrides_parent() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: mode=ask
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
"#,
        )
        .unwrap();

        // Agent: explicitly sets mode=dangerous (should override, not be ignored)
        std::fs::write(
            agent_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: dangerous
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Dangerous);
    }

    #[tokio::test]
    async fn mode_inherited_when_not_set() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: mode=ask
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
"#,
        )
        .unwrap();

        // Agent: no mode set → inherits workspace mode
        std::fs::write(
            agent_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
allow:
  - "bash:cargo*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Ask);
    }

    #[tokio::test]
    async fn save_does_not_freeze_mode() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: mode=ask
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: ask
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir.clone()));

        // Simulate add_pattern_and_save: load, add pattern, save
        let mut policy = store.load("test-agent").await;
        assert_eq!(policy.mode, PolicyMode::Ask);
        policy.allow.push("bash:echo*".to_string());
        store.save("test-agent", &policy).await.unwrap();

        // Now change workspace mode to restrict
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
mode: restrict
"#,
        )
        .unwrap();

        // Reload — mode should follow workspace (restrict), not be frozen as ask
        let policy = store.load("test-agent").await;
        assert_eq!(policy.mode, PolicyMode::Restrict);
        assert_eq!(policy.allow, vec!["bash:echo*"]);
    }

    #[tokio::test]
    async fn save_does_not_freeze_notify() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace: notify enabled
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
notify:
  enabled: true
  patterns:
    - "bash:rm*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir.clone()));

        // Save a local allow pattern (should not persist notify)
        let mut policy = store.load("test-agent").await;
        assert!(policy.notify.enabled);
        policy.allow.push("bash:echo*".to_string());
        store.save("test-agent", &policy).await.unwrap();

        let local_content = std::fs::read_to_string(agent_dir.join(POLICY_LOCAL_FILE)).unwrap();
        assert!(!local_content.contains("notify"));

        // Disable notify at workspace level
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
notify:
  enabled: false
"#,
        )
        .unwrap();

        let reloaded = store.load("test-agent").await;
        assert!(!reloaded.notify.enabled);
    }

    #[tokio::test]
    async fn workspace_only_no_agent_files() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        std::fs::create_dir(agents_dir.join("test-agent")).unwrap();

        // Only workspace policy, no agent files
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            r#"
apiVersion: duragent/v1alpha1
kind: Policy
deny:
  - "bash:rm -rf*"
"#,
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));
        let policy = store.load("test-agent").await;

        assert_eq!(policy.mode, PolicyMode::Dangerous);
        assert_eq!(policy.deny, vec!["bash:rm -rf*"]);
        assert!(policy.allow.is_empty());
    }

    #[tokio::test]
    async fn save_does_not_duplicate_inherited_deny() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let agents_dir = workspace_dir.join("agents");
        std::fs::create_dir(&agents_dir).unwrap();
        let agent_dir = agents_dir.join("test-agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // Workspace deny
        std::fs::write(
            workspace_dir.join(POLICY_FILE),
            "deny:\n  - \"bash:rm -rf*\"\n",
        )
        .unwrap();

        // Agent base deny
        std::fs::write(
            agent_dir.join(POLICY_FILE),
            "deny:\n  - \"bash:*sudo*\"\nallow:\n  - \"bash:ls*\"\n",
        )
        .unwrap();

        let store = FilePolicyStore::new(&agents_dir, Some(workspace_dir));

        // Simulate add_pattern_and_save: load merged, add pattern, save
        let mut policy = store.load("test-agent").await;
        assert_eq!(policy.deny.len(), 2); // workspace + base
        policy.allow.push("bash:echo*".to_string());
        store.save("test-agent", &policy).await.unwrap();

        // Reload — deny should still be exactly 2, not 4
        let reloaded = store.load("test-agent").await;
        assert_eq!(reloaded.deny.len(), 2);

        // Save again — still 2, no growth
        store.save("test-agent", &reloaded).await.unwrap();
        let reloaded2 = store.load("test-agent").await;
        assert_eq!(reloaded2.deny.len(), 2);
    }
}
