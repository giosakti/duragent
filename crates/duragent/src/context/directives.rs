//! Directives loading: file-based + runtime defaults.
//!
//! File-based directives are `*.md` files loaded from two directories:
//! - Workspace-level: `{workspace}/directives/` (shared across all agents)
//! - Agent-level: `{agent_dir}/directives/` (per-agent)
//!
//! Runtime defaults are generated from agent config (e.g. memory directive).
//! File-based directives override runtime defaults when the `source` name matches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tracing::warn;

use crate::agent::AgentSpec;
use crate::config::DEFAULT_DIRECTIVES_DIR;

use super::{DirectiveEntry, Scope};

/// Load directives from file system and merge with runtime defaults.
///
/// File-based directives override runtime defaults when the `source` name matches
/// (e.g. a `directives/memory.md` file overrides the built-in memory directive).
pub fn load_all_directives(
    workspace_directives: &Path,
    agent_dir: &Path,
    agent: &AgentSpec,
) -> Vec<DirectiveEntry> {
    let mut file_directives = load_directives_from_dir(workspace_directives, Scope::Global);
    file_directives.extend(load_directives_from_dir(
        &agent_dir.join(DEFAULT_DIRECTIVES_DIR),
        Scope::Agent,
    ));
    let defaults = default_directives(agent);
    merge_directives(file_directives, defaults)
}

/// Load directives using a blocking filesystem call in a dedicated thread.
pub async fn load_all_directives_async(
    workspace_directives: PathBuf,
    agent_dir: PathBuf,
    agent: Arc<AgentSpec>,
) -> Vec<DirectiveEntry> {
    match tokio::task::spawn_blocking(move || {
        load_all_directives(&workspace_directives, &agent_dir, &agent)
    })
    .await
    {
        Ok(directives) => directives,
        Err(e) => {
            warn!(error = %e, "Failed to load directives");
            Vec::new()
        }
    }
}

/// Generate runtime default directives based on agent configuration.
pub fn default_directives(agent: &AgentSpec) -> Vec<DirectiveEntry> {
    let mut directives = Vec::new();
    if agent.memory.is_some() {
        directives.push(DirectiveEntry {
            instruction: crate::memory::DEFAULT_MEMORY_DIRECTIVE.to_string(),
            scope: Scope::Agent,
            source: "memory".to_string(),
            added_at: Utc::now(),
        });
    }
    directives
}

/// Merge file-based directives with runtime defaults.
///
/// File-based directives win on `source` name conflict.
fn merge_directives(
    file_directives: Vec<DirectiveEntry>,
    defaults: Vec<DirectiveEntry>,
) -> Vec<DirectiveEntry> {
    let file_sources: HashSet<&str> = file_directives.iter().map(|d| d.source.as_str()).collect();
    let mut merged: Vec<DirectiveEntry> = defaults
        .into_iter()
        .filter(|d| !file_sources.contains(d.source.as_str()))
        .collect();
    merged.extend(file_directives);
    merged
}

/// Load all `*.md` directive files from a directory.
///
/// Files are sorted by filename. Empty/whitespace-only files are skipped.
/// Returns an empty vec if the directory doesn't exist.
pub fn load_directives_from_dir(dir: &Path, scope: Scope) -> Vec<DirectiveEntry> {
    if !dir.exists() {
        return Vec::new();
    }

    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .collect(),
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "Failed to read directives directory");
            return Vec::new();
        }
    };

    entries.sort_by_key(|e| e.file_name());

    let now = Utc::now();
    entries
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let source = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            match std::fs::read_to_string(&path) {
                Ok(content) if !content.trim().is_empty() => Some(DirectiveEntry {
                    instruction: content,
                    scope,
                    source,
                    added_at: now,
                }),
                Ok(_) => None, // skip empty/whitespace-only
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to read directive file");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentMemoryConfig;
    use tempfile::TempDir;

    #[test]
    fn nonexistent_dir_returns_empty() {
        let dir = Path::new("/tmp/nonexistent-directives-test-dir");
        let result = load_directives_from_dir(dir, Scope::Global);
        assert!(result.is_empty());
    }

    #[test]
    fn empty_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let result = load_directives_from_dir(tmp.path(), Scope::Global);
        assert!(result.is_empty());
    }

    #[test]
    fn reads_sorted_md_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("b-second.md"), "Second directive").unwrap();
        std::fs::write(tmp.path().join("a-first.md"), "First directive").unwrap();

        let result = load_directives_from_dir(tmp.path(), Scope::Global);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].source, "a-first");
        assert_eq!(result[0].instruction, "First directive");
        assert_eq!(result[1].source, "b-second");
        assert_eq!(result[1].instruction, "Second directive");
    }

    #[test]
    fn skips_non_md_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("valid.md"), "Keep me").unwrap();
        std::fs::write(tmp.path().join("skip.txt"), "Ignore me").unwrap();
        std::fs::write(tmp.path().join("skip.yaml"), "Also ignore").unwrap();

        let result = load_directives_from_dir(tmp.path(), Scope::Global);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, "valid");
    }

    #[test]
    fn skips_empty_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("empty.md"), "").unwrap();
        std::fs::write(tmp.path().join("whitespace.md"), "   \n  \n  ").unwrap();
        std::fs::write(tmp.path().join("content.md"), "Real content").unwrap();

        let result = load_directives_from_dir(tmp.path(), Scope::Global);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, "content");
    }

    #[test]
    fn sets_correct_scope() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("test.md"), "Content").unwrap();

        let global = load_directives_from_dir(tmp.path(), Scope::Global);
        assert_eq!(global[0].scope, Scope::Global);

        let agent = load_directives_from_dir(tmp.path(), Scope::Agent);
        assert_eq!(agent[0].scope, Scope::Agent);
    }

    #[test]
    fn merges_workspace_and_agent_directives() {
        let tmp = TempDir::new().unwrap();

        // Workspace directives
        let workspace_dir = tmp.path().join("directives");
        std::fs::create_dir(&workspace_dir).unwrap();
        std::fs::write(workspace_dir.join("memory.md"), "Memory instructions").unwrap();

        // Agent directives
        let agent_dir = tmp.path().join("agent");
        let agent_directives = agent_dir.join("directives");
        std::fs::create_dir_all(&agent_directives).unwrap();
        std::fs::write(agent_directives.join("custom.md"), "Agent custom").unwrap();

        let agent = stub_agent(None);
        let result = load_all_directives(&workspace_dir, &agent_dir, &agent);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].scope, Scope::Global);
        assert_eq!(result[0].source, "memory");
        assert_eq!(result[1].scope, Scope::Agent);
        assert_eq!(result[1].source, "custom");
    }

    #[test]
    fn default_directives_returns_memory_when_enabled() {
        let agent = stub_agent(Some(AgentMemoryConfig {
            backend: "filesystem".to_string(),
        }));
        let directives = default_directives(&agent);

        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].source, "memory");
        assert!(
            directives[0]
                .instruction
                .contains("persistent memory system")
        );
    }

    #[test]
    fn default_directives_empty_when_no_memory() {
        let agent = stub_agent(None);
        let directives = default_directives(&agent);
        assert!(directives.is_empty());
    }

    #[test]
    fn merge_file_overrides_runtime_default() {
        let file = vec![DirectiveEntry {
            instruction: "Custom memory instructions".to_string(),
            scope: Scope::Agent,
            source: "memory".to_string(),
            added_at: Utc::now(),
        }];
        let defaults = vec![DirectiveEntry {
            instruction: "Default memory instructions".to_string(),
            scope: Scope::Agent,
            source: "memory".to_string(),
            added_at: Utc::now(),
        }];

        let merged = merge_directives(file, defaults);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].instruction, "Custom memory instructions");
    }

    #[test]
    fn merge_keeps_non_overlapping() {
        let file = vec![DirectiveEntry {
            instruction: "Custom directive".to_string(),
            scope: Scope::Agent,
            source: "custom".to_string(),
            added_at: Utc::now(),
        }];
        let defaults = vec![DirectiveEntry {
            instruction: "Memory directive".to_string(),
            scope: Scope::Agent,
            source: "memory".to_string(),
            added_at: Utc::now(),
        }];

        let merged = merge_directives(file, defaults);

        assert_eq!(merged.len(), 2);
        let sources: Vec<&str> = merged.iter().map(|d| d.source.as_str()).collect();
        assert!(sources.contains(&"memory"));
        assert!(sources.contains(&"custom"));
    }

    /// Minimal AgentSpec for directive tests.
    fn stub_agent(memory: Option<AgentMemoryConfig>) -> crate::agent::AgentSpec {
        use crate::agent::{
            AgentMetadata, AgentSessionConfig, HooksConfig, ModelConfig, ToolPolicy,
        };
        use crate::llm::Provider;
        use std::collections::HashMap;
        use std::path::PathBuf;

        crate::agent::AgentSpec {
            api_version: "duragent/v1alpha1".to_string(),
            kind: "Agent".to_string(),
            metadata: AgentMetadata {
                name: "test-agent".to_string(),
                description: None,
                version: None,
                labels: HashMap::new(),
            },
            model: ModelConfig {
                provider: Provider::Other("test".to_string()),
                name: "test-model".to_string(),
                base_url: None,
                temperature: None,
                max_input_tokens: None,
                max_output_tokens: None,
            },
            soul: None,
            system_prompt: None,
            instructions: None,
            skills: Vec::new(),
            session: AgentSessionConfig::default(),
            memory,
            tools: Vec::new(),
            policy: ToolPolicy::default(),
            hooks: HooksConfig::default(),
            access: None,
            agent_dir: PathBuf::from("/tmp/test-agent"),
        }
    }
}
