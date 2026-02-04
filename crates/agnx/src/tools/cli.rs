//! CLI tool execution (custom scripts).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use crate::llm::{FunctionDefinition, ToolDefinition};
use crate::sandbox::Sandbox;

use super::error::ToolError;
use super::executor::ToolResult;
use super::tool::Tool;

/// A CLI tool that executes a custom script.
pub struct CliTool {
    sandbox: Arc<dyn Sandbox>,
    agent_dir: PathBuf,
    name: String,
    command: String,
    description: Option<String>,
    readme_content: Option<String>,
}

impl CliTool {
    /// Create a new CLI tool.
    ///
    /// If `readme_path` is provided, the file is loaded eagerly at construction.
    pub fn new(
        sandbox: Arc<dyn Sandbox>,
        agent_dir: PathBuf,
        name: String,
        command: String,
        description: Option<String>,
        readme_path: Option<&str>,
    ) -> Self {
        // Load README content eagerly if path is provided
        let readme_content = readme_path.and_then(|path| {
            let full_path = agent_dir.join(path);
            match std::fs::read_to_string(&full_path) {
                Ok(content) => Some(content),
                Err(e) => {
                    warn!(
                        tool = %name,
                        path = %full_path.display(),
                        error = %e,
                        "Failed to load tool README"
                    );
                    None
                }
            }
        });

        Self {
            sandbox,
            agent_dir,
            name,
            command,
            description,
            readme_content,
        }
    }
}

#[async_trait]
impl Tool for CliTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        // Build description, appending README if available
        let description = match (&self.description, &self.readme_content) {
            (Some(desc), Some(readme)) => format!("{}\n\n{}", desc, readme),
            (Some(desc), None) => desc.clone(),
            (None, Some(readme)) => format!("CLI tool: {}\n\n{}", self.name, readme),
            (None, None) => format!("CLI tool: {}", self.name),
        };

        definition_with_description(&self.name, &description)
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute(&self.sandbox, &self.agent_dir, &self.command, arguments).await
    }

    fn readme(&self) -> Option<&str> {
        self.readme_content.as_deref()
    }
}

/// Execute a CLI tool.
pub async fn execute(
    sandbox: &Arc<dyn Sandbox>,
    agent_dir: &Path,
    command: &str,
    arguments: &str,
) -> Result<ToolResult, ToolError> {
    // Parse arguments JSON
    let args: CliArgs = serde_json::from_str(arguments).unwrap_or_else(|e| {
        warn!(
            error = %e,
            arguments,
            "Failed to parse CLI tool arguments, using defaults"
        );
        CliArgs::default()
    });

    // Build the full command
    let full_command = if args.args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.args)
    };

    // Execute via sandbox (uses default timeout)
    let result = sandbox
        .exec(
            "bash",
            &["-c".to_string(), full_command],
            Some(agent_dir),
            None,
        )
        .await?;

    Ok(ToolResult::from_exec(result))
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Generate a tool definition for a CLI tool with a required description.
fn definition_with_description(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Command line arguments to pass to the tool"
                    }
                }
            })),
        },
    }
}

// ============================================================================
// Private Types
// ============================================================================

/// Arguments for CLI tools.
#[derive(Default, serde::Deserialize)]
struct CliArgs {
    #[serde(default)]
    args: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: generate a tool definition for a CLI tool.
    fn definition(name: &str, description: Option<&str>) -> ToolDefinition {
        let default_desc = format!("CLI tool: {}", name);
        let desc = description
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_desc);
        definition_with_description(name, desc)
    }

    // ==========================================================================
    // definition() - Happy path: generates correct tool definitions
    // ==========================================================================

    #[test]
    fn test_definition_with_description() {
        let def = definition("git-helper", Some("Run git commands"));
        assert_eq!(def.function.name, "git-helper");
        assert_eq!(def.function.description, "Run git commands");
        assert_eq!(def.tool_type, "function");
    }

    #[test]
    fn test_definition_without_description() {
        let def = definition("my-tool", None);
        assert_eq!(def.function.name, "my-tool");
        assert_eq!(def.function.description, "CLI tool: my-tool");
    }

    #[test]
    fn test_definition_has_args_parameter() {
        let def = definition("test-tool", None);
        let params = def.function.parameters.expect("should have parameters");

        assert_eq!(params["type"], "object");
        assert!(params["properties"]["args"].is_object());
        assert_eq!(params["properties"]["args"]["type"], "string");
    }

    // ==========================================================================
    // CliArgs - Argument parsing
    // ==========================================================================

    #[test]
    fn cli_args_parses_args_field() {
        let args: CliArgs = serde_json::from_str(r#"{"args": "-v --force"}"#).unwrap();
        assert_eq!(args.args, "-v --force");
    }

    #[test]
    fn cli_args_defaults_to_empty_string() {
        let args: CliArgs = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(args.args, "");
    }

    #[test]
    fn cli_args_handles_empty_args() {
        let args: CliArgs = serde_json::from_str(r#"{"args": ""}"#).unwrap();
        assert_eq!(args.args, "");
    }

    // ==========================================================================
    // CliTool - Tool trait implementation with README injection
    // ==========================================================================

    mod cli_tool_tests {
        use super::*;
        use crate::sandbox::TrustSandbox;
        use crate::tools::tool::Tool;
        use std::io::Write;
        use tempfile::TempDir;

        #[test]
        fn definition_without_readme() {
            let temp_dir = TempDir::new().unwrap();
            let sandbox: Arc<dyn Sandbox> = Arc::new(TrustSandbox);

            let tool = CliTool::new(
                sandbox,
                temp_dir.path().to_path_buf(),
                "my-tool".to_string(),
                "./my-tool.sh".to_string(),
                Some("My custom tool".to_string()),
                None,
            );

            let def = tool.definition();
            assert_eq!(def.function.name, "my-tool");
            assert_eq!(def.function.description, "My custom tool");
            assert!(tool.readme().is_none());
        }

        #[test]
        fn definition_with_readme_appends_to_description() {
            let temp_dir = TempDir::new().unwrap();
            let readme_path = temp_dir.path().join("README.md");
            let mut file = std::fs::File::create(&readme_path).unwrap();
            writeln!(file, "# Tool Documentation\n\nUsage: ./my-tool [args]").unwrap();

            let sandbox: Arc<dyn Sandbox> = Arc::new(TrustSandbox);

            let tool = CliTool::new(
                sandbox,
                temp_dir.path().to_path_buf(),
                "my-tool".to_string(),
                "./my-tool.sh".to_string(),
                Some("My custom tool".to_string()),
                Some("README.md"),
            );

            let def = tool.definition();
            assert_eq!(def.function.name, "my-tool");
            assert!(def.function.description.starts_with("My custom tool\n\n"));
            assert!(def.function.description.contains("Tool Documentation"));
            assert!(tool.readme().is_some());
        }

        #[test]
        fn definition_with_readme_but_no_description() {
            let temp_dir = TempDir::new().unwrap();
            let readme_path = temp_dir.path().join("README.md");
            let mut file = std::fs::File::create(&readme_path).unwrap();
            writeln!(file, "Extended docs here").unwrap();

            let sandbox: Arc<dyn Sandbox> = Arc::new(TrustSandbox);

            let tool = CliTool::new(
                sandbox,
                temp_dir.path().to_path_buf(),
                "my-tool".to_string(),
                "./my-tool.sh".to_string(),
                None,
                Some("README.md"),
            );

            let def = tool.definition();
            assert!(
                def.function
                    .description
                    .starts_with("CLI tool: my-tool\n\n")
            );
            assert!(def.function.description.contains("Extended docs here"));
        }

        #[test]
        fn definition_with_missing_readme_proceeds_without() {
            let temp_dir = TempDir::new().unwrap();
            let sandbox: Arc<dyn Sandbox> = Arc::new(TrustSandbox);

            let tool = CliTool::new(
                sandbox,
                temp_dir.path().to_path_buf(),
                "my-tool".to_string(),
                "./my-tool.sh".to_string(),
                Some("My custom tool".to_string()),
                Some("nonexistent.md"),
            );

            let def = tool.definition();
            // Should fall back to just the description
            assert_eq!(def.function.description, "My custom tool");
            assert!(tool.readme().is_none());
        }

        #[test]
        fn name_returns_tool_name() {
            let temp_dir = TempDir::new().unwrap();
            let sandbox: Arc<dyn Sandbox> = Arc::new(TrustSandbox);

            let tool = CliTool::new(
                sandbox,
                temp_dir.path().to_path_buf(),
                "deploy".to_string(),
                "./deploy.sh".to_string(),
                None,
                None,
            );

            assert_eq!(tool.name(), "deploy");
        }
    }

    // ==========================================================================
    // execute() - Integration tests with mock sandbox
    // ==========================================================================

    mod execute_tests {
        use super::*;
        use crate::sandbox::{ExecResult, SandboxError};
        use async_trait::async_trait;
        use std::path::PathBuf;
        use std::time::Duration;

        /// Mock sandbox that returns configurable results and verifies expected commands.
        struct MockSandbox {
            result: ExecResult,
            expected_command: Option<String>,
        }

        impl MockSandbox {
            fn new(result: ExecResult) -> Self {
                Self {
                    result,
                    expected_command: None,
                }
            }

            fn with_expected_command(mut self, cmd: &str) -> Self {
                self.expected_command = Some(cmd.to_string());
                self
            }
        }

        #[async_trait]
        impl Sandbox for MockSandbox {
            async fn exec(
                &self,
                cmd: &str,
                args: &[String],
                _cwd: Option<&Path>,
                _timeout: Option<Duration>,
            ) -> Result<ExecResult, SandboxError> {
                // Verify bash -c is used
                assert_eq!(cmd, "bash");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0], "-c");

                // Verify expected command if set
                if let Some(ref expected) = self.expected_command {
                    assert_eq!(&args[1], expected);
                }

                Ok(self.result.clone())
            }

            fn mode(&self) -> &'static str {
                "mock"
            }
        }

        #[tokio::test]
        async fn execute_without_args() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(
                MockSandbox::new(ExecResult {
                    exit_code: 0,
                    stdout: "success".to_string(),
                    stderr: "".to_string(),
                })
                .with_expected_command("./my-script"),
            );

            let result = execute(&sandbox, &PathBuf::from("/agent"), "./my-script", r#"{}"#)
                .await
                .unwrap();

            assert!(result.success);
            assert!(result.content.contains("success"));
        }

        #[tokio::test]
        async fn execute_with_args() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(
                MockSandbox::new(ExecResult {
                    exit_code: 0,
                    stdout: "output".to_string(),
                    stderr: "".to_string(),
                })
                .with_expected_command("./build --release --target linux"),
            );

            let result = execute(
                &sandbox,
                &PathBuf::from("/agent"),
                "./build",
                r#"{"args": "--release --target linux"}"#,
            )
            .await
            .unwrap();

            assert!(result.success);
        }

        #[tokio::test]
        async fn execute_with_invalid_json_uses_defaults() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(
                MockSandbox::new(ExecResult {
                    exit_code: 0,
                    stdout: "ran".to_string(),
                    stderr: "".to_string(),
                })
                .with_expected_command("./tool"),
            );

            // Invalid JSON should use empty args (default)
            let result = execute(
                &sandbox,
                &PathBuf::from("/agent"),
                "./tool",
                "not valid json",
            )
            .await
            .unwrap();

            assert!(result.success);
        }

        #[tokio::test]
        async fn execute_returns_failure_on_nonzero_exit() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(MockSandbox::new(ExecResult {
                exit_code: 1,
                stdout: "".to_string(),
                stderr: "error occurred".to_string(),
            }));

            let result = execute(
                &sandbox,
                &PathBuf::from("/agent"),
                "./failing-script",
                r#"{}"#,
            )
            .await
            .unwrap();

            assert!(!result.success);
            assert!(result.content.contains("error occurred"));
        }

        #[tokio::test]
        async fn execute_combines_stdout_and_stderr() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(MockSandbox::new(ExecResult {
                exit_code: 0,
                stdout: "stdout output".to_string(),
                stderr: "stderr output".to_string(),
            }));

            let result = execute(&sandbox, &PathBuf::from("/agent"), "./script", r#"{}"#)
                .await
                .unwrap();

            assert!(result.content.contains("stdout output"));
            assert!(result.content.contains("stderr output"));
        }

        /// Mock sandbox that returns an error.
        struct ErrorSandbox;

        #[async_trait]
        impl Sandbox for ErrorSandbox {
            async fn exec(
                &self,
                _cmd: &str,
                _args: &[String],
                _cwd: Option<&Path>,
                _timeout: Option<Duration>,
            ) -> Result<ExecResult, SandboxError> {
                Err(SandboxError::ExecutionFailed("sandbox failed".to_string()))
            }

            fn mode(&self) -> &'static str {
                "error"
            }
        }

        #[tokio::test]
        async fn execute_propagates_sandbox_error() {
            let sandbox: Arc<dyn Sandbox> = Arc::new(ErrorSandbox);

            let result = execute(&sandbox, &PathBuf::from("/agent"), "./script", r#"{}"#).await;

            assert!(result.is_err());
        }
    }
}
