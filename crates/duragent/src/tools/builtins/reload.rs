//! Reload tools.
//!
//! The `reload_tools` tool scans tool directories and reports what tools were
//! discovered. The actual executor rebuild happens in the agentic loop after
//! this tool returns — this tool signals intent and provides the scan report.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::sandbox::Sandbox;

use crate::tools::discovery::discover_all_tools;
use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

// ============================================================================
// ReloadToolsTool
// ============================================================================

/// Built-in tool that scans tool directories and reports discovered tools.
pub struct ReloadToolsTool {
    discovery_dirs: Vec<PathBuf>,
    sandbox: Arc<dyn Sandbox>,
}

impl ReloadToolsTool {
    pub fn new(discovery_dirs: Vec<PathBuf>, sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            discovery_dirs,
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for ReloadToolsTool {
    fn name(&self) -> &str {
        "reload_tools"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "reload_tools".to_string(),
                description: "Reload tools by scanning tool directories. Call this after writing \
                    a new tool script to disk so it becomes available for use. Returns a summary \
                    of all discovered tools."
                    .to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {}
                })),
            },
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<ToolResult, ToolError> {
        let tools = discover_all_tools(&self.discovery_dirs, &self.sandbox);

        let tool_list: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.definition().function.description,
                })
            })
            .collect();

        let summary = serde_json::json!({
            "discovered_tools": tool_list,
            "count": tools.len(),
            "directories_scanned": self.discovery_dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>(),
        });

        Ok(ToolResult {
            success: true,
            content: serde_json::to_string_pretty(&summary).unwrap_or_default(),
        })
    }
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

    #[tokio::test]
    async fn reload_tools_returns_discovered_tools() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("my-tool");
        fs::create_dir(&tool_dir).unwrap();
        let mut f = fs::File::create(tool_dir.join("run.sh")).unwrap();
        writeln!(f, "#!/bin/bash\necho hello").unwrap();

        let tool = ReloadToolsTool::new(vec![tmp.path().to_path_buf()], test_sandbox());
        let result = tool.execute("{}").await.unwrap();

        assert!(result.success);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["discovered_tools"][0]["name"], "my-tool");
    }

    #[tokio::test]
    async fn reload_tools_empty_directories() {
        let tmp = TempDir::new().unwrap();

        let tool = ReloadToolsTool::new(vec![tmp.path().to_path_buf()], test_sandbox());
        let result = tool.execute("{}").await.unwrap();

        assert!(result.success);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["count"], 0);
    }

    #[test]
    fn reload_tools_name_and_definition() {
        let tool = ReloadToolsTool::new(vec![], test_sandbox());

        assert_eq!(tool.name(), "reload_tools");
        let def = tool.definition();
        assert_eq!(def.function.name, "reload_tools");
        assert!(def.function.description.contains("Reload tools"));
    }
}
