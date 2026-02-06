//! File-based directives loading.
//!
//! Directives are `*.md` files loaded from two directories:
//! - Workspace-level: `{workspace}/directives/` (shared across all agents)
//! - Agent-level: `{agent_dir}/directives/` (per-agent)
//!
//! Both levels are always concatenated (workspace first, then agent).

use std::path::Path;

use chrono::Utc;
use tracing::warn;

use super::{DirectiveEntry, Scope};

/// Load directives from both workspace and agent directories.
///
/// Workspace directives (scope: Global) come first, then agent directives (scope: Agent).
pub fn load_all_directives(workspace_directives: &Path, agent_dir: &Path) -> Vec<DirectiveEntry> {
    let mut directives = load_directives_from_dir(workspace_directives, Scope::Global);
    directives.extend(load_directives_from_dir(
        &agent_dir.join("directives"),
        Scope::Agent,
    ));
    directives
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

/// Ensure the default memory directive file exists.
///
/// Creates `{workspace_directives}/memory.md` with default content if it doesn't exist.
/// No-op if the file already exists.
pub fn ensure_memory_directive(workspace_directives: &Path, default_content: &str) {
    let path = workspace_directives.join("memory.md");
    if path.exists() {
        return;
    }

    if let Err(e) = std::fs::create_dir_all(workspace_directives) {
        warn!(error = %e, "Failed to create directives directory");
        return;
    }

    if let Err(e) = std::fs::write(&path, default_content) {
        warn!(path = %path.display(), error = %e, "Failed to write default memory directive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let result = load_all_directives(&workspace_dir, &agent_dir);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].scope, Scope::Global);
        assert_eq!(result[0].source, "memory");
        assert_eq!(result[1].scope, Scope::Agent);
        assert_eq!(result[1].source, "custom");
    }

    #[test]
    fn ensure_creates_memory_directive() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("directives");

        ensure_memory_directive(&dir, "Default content");

        let path = dir.join("memory.md");
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Default content");
    }

    #[test]
    fn ensure_does_not_overwrite_existing() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("directives");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("memory.md"), "User-edited content").unwrap();

        ensure_memory_directive(&dir, "Default content");

        let content = std::fs::read_to_string(dir.join("memory.md")).unwrap();
        assert_eq!(content, "User-edited content");
    }
}
