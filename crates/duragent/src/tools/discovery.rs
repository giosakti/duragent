//! Convention-based tool discovery.
//!
//! Scans directories for tool subdirectories following this convention:
//! ```text
//! tools/
//!   my-tool/
//!     run           # or run.sh, run.py — any executable named run or run.*
//!     README.md     # optional, loaded as tool description
//! ```
//!
//! Tool name = directory name (e.g., `tools/code-search/` → tool named `code-search`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::debug;

use crate::sandbox::Sandbox;

use super::builtins::cli::CliTool;
use super::tool::SharedTool;

// ============================================================================
// Public API
// ============================================================================

/// Scan multiple directories for tools, deduplicating by name (first dir wins).
pub fn discover_all_tools(dirs: &[PathBuf], sandbox: &Arc<dyn Sandbox>) -> Vec<SharedTool> {
    let mut seen = HashSet::new();
    let mut tools = Vec::new();

    for dir in dirs {
        for tool in discover_tools(dir, sandbox) {
            if seen.insert(tool.name().to_string()) {
                tools.push(tool);
            } else {
                debug!(
                    tool = %tool.name(),
                    dir = %dir.display(),
                    "Skipping duplicate discovered tool"
                );
            }
        }
    }

    tools
}

/// Scan a single directory for tool subdirectories.
pub fn discover_tools(dir: &Path, sandbox: &Arc<dyn Sandbox>) -> Vec<SharedTool> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut tools = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(tool) = discover_single_tool(&path, sandbox) else {
            continue;
        };
        tools.push(tool);
    }

    tools
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Discover a single tool from a subdirectory.
fn discover_single_tool(tool_dir: &Path, sandbox: &Arc<dyn Sandbox>) -> Option<SharedTool> {
    let executable = find_run_executable(tool_dir)?;

    let dir_name = tool_dir.file_name()?.to_string_lossy().to_string();
    let readme = load_readme(tool_dir);
    let abs_command = executable.to_string_lossy().to_string();

    debug!(
        tool = %dir_name,
        command = %abs_command,
        has_readme = readme.is_some(),
        "Discovered tool"
    );

    // CliTool expects a relative readme path; we already loaded the content,
    // so pass description=None and readme_path=None, then construct directly.
    // We set description from README content if present.
    let tool = CliTool::new(
        sandbox.clone(),
        tool_dir.to_path_buf(),
        dir_name,
        abs_command,
        readme.clone(),
        None,
    );

    Some(Arc::new(tool))
}

/// Find an executable named `run` or `run.*` in a directory.
fn find_run_executable(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Match "run" exactly or "run.*" (e.g., run.sh, run.py)
        if file_name == "run" || file_name.starts_with("run.") {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }

    None
}

/// Load README.md content from a tool directory.
fn load_readme(dir: &Path) -> Option<String> {
    let readme_path = dir.join("README.md");
    std::fs::read_to_string(&readme_path).ok()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::TrustSandbox;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn test_sandbox() -> Arc<dyn Sandbox> {
        Arc::new(TrustSandbox)
    }

    // ------------------------------------------------------------------------
    // discover_tools — single directory scanning
    // ------------------------------------------------------------------------

    #[test]
    fn discover_tools_finds_tool_with_run_script() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("my-tool");
        fs::create_dir(&tool_dir).unwrap();

        let run_path = tool_dir.join("run.sh");
        let mut f = fs::File::create(&run_path).unwrap();
        writeln!(f, "#!/bin/bash\necho hello").unwrap();

        let sandbox = test_sandbox();
        let tools = discover_tools(tmp.path(), &sandbox);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "my-tool");
    }

    #[test]
    fn discover_tools_finds_tool_with_bare_run() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("bare-tool");
        fs::create_dir(&tool_dir).unwrap();

        let run_path = tool_dir.join("run");
        let mut f = fs::File::create(&run_path).unwrap();
        writeln!(f, "#!/bin/bash\necho bare").unwrap();

        let sandbox = test_sandbox();
        let tools = discover_tools(tmp.path(), &sandbox);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "bare-tool");
    }

    #[test]
    fn discover_tools_loads_readme_as_description() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("documented-tool");
        fs::create_dir(&tool_dir).unwrap();

        let run_path = tool_dir.join("run.py");
        fs::File::create(&run_path).unwrap();

        let readme_path = tool_dir.join("README.md");
        let mut f = fs::File::create(&readme_path).unwrap();
        writeln!(f, "# My Tool\n\nDoes cool things.").unwrap();

        let sandbox = test_sandbox();
        let tools = discover_tools(tmp.path(), &sandbox);

        assert_eq!(tools.len(), 1);
        let def = tools[0].definition();
        assert!(def.function.description.contains("My Tool"));
    }

    #[test]
    fn discover_tools_skips_dir_without_run() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("no-run");
        fs::create_dir(&tool_dir).unwrap();

        // Only a README, no run script
        let readme_path = tool_dir.join("README.md");
        fs::File::create(&readme_path).unwrap();

        let sandbox = test_sandbox();
        let tools = discover_tools(tmp.path(), &sandbox);

        assert!(tools.is_empty());
    }

    #[test]
    fn discover_tools_returns_empty_for_missing_dir() {
        let sandbox = test_sandbox();
        let tools = discover_tools(Path::new("/nonexistent/tools/dir"), &sandbox);
        assert!(tools.is_empty());
    }

    #[test]
    fn discover_tools_skips_files_in_root_dir() {
        let tmp = TempDir::new().unwrap();
        // A file (not a directory) in the tools dir should be ignored
        fs::File::create(tmp.path().join("stray-file.txt")).unwrap();

        let sandbox = test_sandbox();
        let tools = discover_tools(tmp.path(), &sandbox);

        assert!(tools.is_empty());
    }

    // ------------------------------------------------------------------------
    // discover_all_tools — multi-directory with deduplication
    // ------------------------------------------------------------------------

    #[test]
    fn discover_all_tools_deduplicates_first_wins() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        // Create same tool name in both directories
        for dir in [dir1.path(), dir2.path()] {
            let tool_dir = dir.join("shared-tool");
            fs::create_dir(&tool_dir).unwrap();
            fs::File::create(tool_dir.join("run.sh")).unwrap();
        }

        let sandbox = test_sandbox();
        let tools = discover_all_tools(
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
            &sandbox,
        );

        // Only one instance of "shared-tool"
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "shared-tool");
    }

    #[test]
    fn discover_all_tools_merges_unique_tools() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let tool1 = dir1.path().join("tool-a");
        fs::create_dir(&tool1).unwrap();
        fs::File::create(tool1.join("run")).unwrap();

        let tool2 = dir2.path().join("tool-b");
        fs::create_dir(&tool2).unwrap();
        fs::File::create(tool2.join("run.sh")).unwrap();

        let sandbox = test_sandbox();
        let tools = discover_all_tools(
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
            &sandbox,
        );

        assert_eq!(tools.len(), 2);
        let names: HashSet<_> = tools.iter().map(|t| t.name().to_string()).collect();
        assert!(names.contains("tool-a"));
        assert!(names.contains("tool-b"));
    }
}
