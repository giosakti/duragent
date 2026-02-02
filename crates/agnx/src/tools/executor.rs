//! Tool executor for running tools in agentic workflows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs;
use tokio::sync::RwLock;

use super::bash;
use super::cli;
use super::error::ToolError;
use super::notify::send_notification;
use super::schedule::{self, ToolExecutionContext};
use crate::agent::{NotifyConfig, PolicyDecision, ToolConfig, ToolPolicy, ToolType};
use crate::llm::{FunctionDefinition, ToolCall, ToolDefinition};
use crate::sandbox::{ExecResult, Sandbox};
use crate::scheduler::SchedulerHandle;

// ============================================================================
// Types
// ============================================================================

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Whether the tool succeeded.
    pub success: bool,
    /// Content for LLM consumption.
    pub content: String,
}

impl ToolResult {
    /// Build a ToolResult from sandbox execution output.
    pub fn from_exec(result: ExecResult) -> Self {
        let mut content = String::new();

        if !result.stdout.is_empty() {
            content.push_str(&result.stdout);
        }
        if !result.stderr.is_empty() {
            if !content.is_empty() {
                content.push_str("\n--- stderr ---\n");
            }
            content.push_str(&result.stderr);
        }
        if content.is_empty() {
            content = format!("Command completed with exit code {}", result.exit_code);
        }

        Self {
            success: result.exit_code == 0,
            content,
        }
    }
}

// ============================================================================
// Executor
// ============================================================================

/// Executor for running tools.
pub struct ToolExecutor {
    /// Tool configurations by name.
    tools: HashMap<String, ToolConfig>,
    /// Sandbox for executing commands.
    sandbox: Arc<dyn Sandbox>,
    /// Base directory for the agent (for resolving relative paths).
    agent_dir: PathBuf,
    /// Cached README content by tool name.
    readme_cache: RwLock<HashMap<String, String>>,
    /// Tool policy for command filtering.
    policy: ToolPolicy,
    /// Notification configuration.
    notify_config: NotifyConfig,
    /// Session ID for notifications (optional).
    session_id: Option<String>,
    /// Agent name for notifications.
    agent_name: String,
    /// Scheduler handle for schedule tools (optional).
    scheduler: Option<SchedulerHandle>,
    /// Execution context for schedule tools (gateway, chat_id, etc.).
    execution_context: Option<ToolExecutionContext>,
}

impl ToolExecutor {
    /// Create a new tool executor.
    pub fn new(
        tools: Vec<ToolConfig>,
        sandbox: Arc<dyn Sandbox>,
        agent_dir: PathBuf,
        policy: ToolPolicy,
        agent_name: String,
    ) -> Self {
        let tools_map = tools
            .into_iter()
            .map(|tc| {
                let name = match &tc {
                    ToolConfig::Builtin { name } => name.clone(),
                    ToolConfig::Cli { name, .. } => name.clone(),
                };
                (name, tc)
            })
            .collect();

        let notify_config = policy.notify.clone();

        Self {
            tools: tools_map,
            sandbox,
            agent_dir,
            readme_cache: RwLock::new(HashMap::new()),
            policy,
            notify_config,
            session_id: None,
            agent_name,
            scheduler: None,
            execution_context: None,
        }
    }

    /// Set the session ID for notifications.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the scheduler handle for schedule tools.
    pub fn with_scheduler(mut self, scheduler: SchedulerHandle) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Set the execution context for schedule tools.
    pub fn with_execution_context(mut self, ctx: ToolExecutionContext) -> Self {
        self.execution_context = Some(ctx);
        self
    }

    /// Execute a tool call and return the result.
    ///
    /// Checks policy before execution:
    /// - If denied by policy, returns `PolicyDenied` error
    /// - If approval required (ask mode), returns `ApprovalRequired` error
    /// - If allowed, executes and optionally sends notifications
    pub async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.function.name;
        let config = self
            .tools
            .get(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.clone()))?;

        // Determine tool type and invocation string for policy check
        let (tool_type, invocation) = match config {
            ToolConfig::Builtin { name } if name == "bash" => {
                // For bash, extract the command from arguments
                let command = extract_bash_command(&tool_call.function.arguments);
                (ToolType::Bash, command)
            }
            ToolConfig::Builtin { name } => (ToolType::Builtin, name.clone()),
            ToolConfig::Cli { name, .. } => (ToolType::Builtin, name.clone()),
        };

        // Check policy
        match self.policy.check(tool_type, &invocation) {
            PolicyDecision::Deny => {
                return Err(ToolError::PolicyDenied(invocation));
            }
            PolicyDecision::Ask => {
                return Err(ToolError::ApprovalRequired {
                    call_id: tool_call.id.clone(),
                    command: invocation,
                });
            }
            PolicyDecision::Allow => {
                // Continue with execution
            }
        }

        self.execute_tool(tool_call, config, tool_type, &invocation)
            .await
    }

    /// Execute a tool call bypassing policy checks.
    ///
    /// Use this only for calls that have already been approved through the
    /// approval flow. Skips policy.check() but still executes the tool and
    /// sends notifications.
    pub async fn execute_bypassing_policy(
        &self,
        tool_call: &ToolCall,
    ) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.function.name;
        let config = self
            .tools
            .get(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.clone()))?;

        // Determine tool type and invocation string (for notifications)
        let (tool_type, invocation) = match config {
            ToolConfig::Builtin { name } if name == "bash" => {
                let command = extract_bash_command(&tool_call.function.arguments);
                (ToolType::Bash, command)
            }
            ToolConfig::Builtin { name } => (ToolType::Builtin, name.clone()),
            ToolConfig::Cli { name, .. } => (ToolType::Builtin, name.clone()),
        };

        self.execute_tool(tool_call, config, tool_type, &invocation)
            .await
    }

    /// Internal: execute tool and send notifications.
    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        config: &ToolConfig,
        tool_type: ToolType,
        invocation: &str,
    ) -> Result<ToolResult, ToolError> {
        let result = match config {
            ToolConfig::Builtin { name } if name == "bash" => {
                bash::execute(
                    &self.sandbox,
                    &self.agent_dir,
                    &tool_call.function.arguments,
                )
                .await
            }
            ToolConfig::Builtin { name } if name == "schedule_task" => {
                self.execute_schedule_tool(name, &tool_call.function.arguments)
                    .await
            }
            ToolConfig::Builtin { name } if name == "list_schedules" => {
                self.execute_schedule_tool(name, &tool_call.function.arguments)
                    .await
            }
            ToolConfig::Builtin { name } if name == "cancel_schedule" => {
                self.execute_schedule_tool(name, &tool_call.function.arguments)
                    .await
            }
            ToolConfig::Builtin { name } => {
                Err(ToolError::NotFound(format!("unknown builtin: {}", name)))
            }
            ToolConfig::Cli { command, .. } => {
                cli::execute(
                    &self.sandbox,
                    &self.agent_dir,
                    command,
                    &tool_call.function.arguments,
                )
                .await
            }
        };

        // Send notification if configured
        if self.policy.should_notify(tool_type, invocation) {
            let session_id = self.session_id.as_deref().unwrap_or("unknown");
            let success = result.as_ref().map(|r| r.success).unwrap_or(false);
            send_notification(
                &self.notify_config,
                session_id,
                &self.agent_name,
                invocation,
                success,
            )
            .await;
        }

        result
    }

    /// Generate tool definitions for the LLM.
    ///
    /// If `filter` is provided, only tools whose names are in the filter
    /// will be included. This controls what the LLM sees, not what can execute.
    pub fn tool_definitions(&self, filter: Option<&HashSet<String>>) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|(name, _)| filter.is_none_or(|f| f.contains(*name)))
            .map(|(_, config)| match config {
                ToolConfig::Builtin { name } if name == "bash" => bash::definition(),
                ToolConfig::Builtin { name } if name == "schedule_task" => {
                    schedule::schedule_task_definition()
                }
                ToolConfig::Builtin { name } if name == "list_schedules" => {
                    schedule::list_schedules_definition()
                }
                ToolConfig::Builtin { name } if name == "cancel_schedule" => {
                    schedule::cancel_schedule_definition()
                }
                ToolConfig::Builtin { name } => unknown_builtin_definition(name),
                ToolConfig::Cli {
                    name, description, ..
                } => cli::definition(name, description.as_deref()),
            })
            .collect()
    }

    /// Execute a schedule-related tool.
    async fn execute_schedule_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<ToolResult, ToolError> {
        let scheduler = self
            .scheduler
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("Scheduler not available".to_string()))?;

        let ctx = self.execution_context.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed("Execution context not available".to_string())
        })?;

        match name {
            "schedule_task" => schedule::execute_schedule_task(scheduler, ctx, arguments).await,
            "list_schedules" => schedule::execute_list_schedules(scheduler, ctx).await,
            "cancel_schedule" => schedule::execute_cancel_schedule(scheduler, ctx, arguments).await,
            _ => Err(ToolError::NotFound(format!(
                "unknown schedule tool: {}",
                name
            ))),
        }
    }

    /// Load and cache the README for a tool.
    pub async fn load_readme(&self, tool_name: &str) -> Option<String> {
        // Check cache first
        {
            let cache = self.readme_cache.read().await;
            if let Some(content) = cache.get(tool_name) {
                return Some(content.clone());
            }
        }

        // Get the tool config
        let config = self.tools.get(tool_name)?;
        let readme_path = match config {
            ToolConfig::Cli {
                readme: Some(r), ..
            } => self.agent_dir.join(r),
            _ => return None,
        };

        // Read the README file
        let content = fs::read_to_string(&readme_path).await.ok()?;

        // Cache it
        {
            let mut cache = self.readme_cache.write().await;
            cache.insert(tool_name.to_string(), content.clone());
        }

        Some(content)
    }

    /// Check if any tools are configured.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Generate a fallback definition for an unknown builtin.
fn unknown_builtin_definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: name.to_string(),
            description: format!("Built-in tool: {}", name),
            parameters: None,
        },
    }
}

/// Extract the command string from bash tool arguments.
fn extract_bash_command(arguments: &str) -> String {
    #[derive(serde::Deserialize)]
    struct BashArgs {
        command: String,
    }

    serde_json::from_str::<BashArgs>(arguments)
        .map(|args| args.command)
        .unwrap_or_else(|_| arguments.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::PolicyMode;
    use crate::sandbox::TrustSandbox;
    use tempfile::TempDir;

    // ------------------------------------------------------------------------
    // Test Helpers
    // ------------------------------------------------------------------------

    fn test_executor(tools: Vec<ToolConfig>) -> ToolExecutor {
        let sandbox = Arc::new(TrustSandbox);
        ToolExecutor::new(
            tools,
            sandbox,
            std::path::PathBuf::from("/tmp"),
            ToolPolicy::default(),
            "test-agent".to_string(),
        )
    }

    fn test_executor_with_policy(tools: Vec<ToolConfig>, policy: ToolPolicy) -> ToolExecutor {
        let sandbox = Arc::new(TrustSandbox);
        ToolExecutor::new(
            tools,
            sandbox,
            std::path::PathBuf::from("/tmp"),
            policy,
            "test-agent".to_string(),
        )
    }

    fn test_executor_with_dir(tools: Vec<ToolConfig>, dir: &TempDir) -> ToolExecutor {
        let sandbox = Arc::new(TrustSandbox);
        ToolExecutor::new(
            tools,
            sandbox,
            dir.path().to_path_buf(),
            ToolPolicy::default(),
            "test-agent".to_string(),
        )
    }

    fn bash_tool_call(command: &str) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: "bash".to_string(),
                arguments: format!(r#"{{"command": "{}"}}"#, command),
            },
        }
    }

    // ------------------------------------------------------------------------
    // ToolResult::from_exec - Result building
    // ------------------------------------------------------------------------

    #[test]
    fn tool_result_from_exec_success_with_stdout() {
        let exec_result = ExecResult {
            exit_code: 0,
            stdout: "hello world\n".to_string(),
            stderr: String::new(),
        };

        let result = ToolResult::from_exec(exec_result);

        assert!(result.success);
        assert_eq!(result.content, "hello world\n");
    }

    #[test]
    fn tool_result_from_exec_failure_with_stderr() {
        let exec_result = ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "error: file not found\n".to_string(),
        };

        let result = ToolResult::from_exec(exec_result);

        assert!(!result.success);
        assert_eq!(result.content, "error: file not found\n");
    }

    #[test]
    fn tool_result_from_exec_mixed_stdout_stderr() {
        let exec_result = ExecResult {
            exit_code: 0,
            stdout: "output\n".to_string(),
            stderr: "warning: something\n".to_string(),
        };

        let result = ToolResult::from_exec(exec_result);

        assert!(result.success);
        assert!(result.content.contains("output"));
        assert!(result.content.contains("--- stderr ---"));
        assert!(result.content.contains("warning: something"));
    }

    #[test]
    fn tool_result_from_exec_empty_output_shows_exit_code() {
        let exec_result = ExecResult {
            exit_code: 42,
            stdout: String::new(),
            stderr: String::new(),
        };

        let result = ToolResult::from_exec(exec_result);

        assert!(!result.success);
        assert!(result.content.contains("exit code 42"));
    }

    // ------------------------------------------------------------------------
    // extract_bash_command - Argument parsing
    // ------------------------------------------------------------------------

    #[test]
    fn extract_bash_command_valid_json() {
        let args = r#"{"command": "ls -la"}"#;
        assert_eq!(extract_bash_command(args), "ls -la");
    }

    #[test]
    fn extract_bash_command_invalid_json_returns_raw() {
        let args = "not valid json";
        assert_eq!(extract_bash_command(args), "not valid json");
    }

    #[test]
    fn extract_bash_command_missing_field_returns_raw() {
        let args = r#"{"other": "value"}"#;
        assert_eq!(extract_bash_command(args), r#"{"other": "value"}"#);
    }

    // ------------------------------------------------------------------------
    // unknown_builtin_definition - Fallback definitions
    // ------------------------------------------------------------------------

    #[test]
    fn unknown_builtin_definition_creates_valid_definition() {
        let def = unknown_builtin_definition("custom-tool");

        assert_eq!(def.tool_type, "function");
        assert_eq!(def.function.name, "custom-tool");
        assert!(def.function.description.contains("custom-tool"));
        assert!(def.function.parameters.is_none());
    }

    // ------------------------------------------------------------------------
    // tool_definitions - Definition generation
    // ------------------------------------------------------------------------

    #[test]
    fn tool_definitions_for_builtin_bash() {
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "bash".to_string(),
        }]);

        let defs = executor.tool_definitions(None);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "bash");
        assert!(defs[0].function.parameters.is_some());
    }

    #[test]
    fn tool_definitions_for_cli() {
        let executor = test_executor(vec![ToolConfig::Cli {
            name: "git-helper".to_string(),
            command: "./tools/git-helper.sh".to_string(),
            readme: None,
            description: Some("Run git commands".to_string()),
        }]);

        let defs = executor.tool_definitions(None);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "git-helper");
        assert_eq!(defs[0].function.description, "Run git commands");
    }

    #[test]
    fn tool_definitions_for_multiple_tools() {
        let executor = test_executor(vec![
            ToolConfig::Builtin {
                name: "bash".to_string(),
            },
            ToolConfig::Cli {
                name: "deploy".to_string(),
                command: "./deploy.sh".to_string(),
                readme: None,
                description: None,
            },
        ]);

        let defs = executor.tool_definitions(None);
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn tool_definitions_for_unknown_builtin() {
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "unknown-builtin".to_string(),
        }]);

        let defs = executor.tool_definitions(None);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "unknown-builtin");
        assert!(defs[0].function.description.contains("unknown-builtin"));
    }

    // ------------------------------------------------------------------------
    // execute - Tool execution with policy
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn execute_bash_command() {
        let temp_dir = TempDir::new().unwrap();
        let executor = test_executor_with_dir(
            vec![ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            &temp_dir,
        );

        let tool_call = bash_tool_call("echo hello");
        let result = executor.execute(&tool_call).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn execute_returns_not_found_for_unknown_tool() {
        let executor = test_executor(vec![]);

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: "nonexistent".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = executor.execute(&tool_call).await;

        assert!(matches!(result, Err(ToolError::NotFound(_))));
    }

    #[tokio::test]
    async fn execute_denies_command_when_policy_denies() {
        let policy = ToolPolicy {
            mode: PolicyMode::Dangerous,
            deny: vec!["bash:rm *".to_string()],
            ..Default::default()
        };
        let executor = test_executor_with_policy(
            vec![ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            policy,
        );

        let tool_call = bash_tool_call("rm -rf /tmp/test");
        let result = executor.execute(&tool_call).await;

        assert!(matches!(result, Err(ToolError::PolicyDenied(_))));
    }

    #[tokio::test]
    async fn execute_requires_approval_in_ask_mode() {
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec![], // Nothing pre-allowed
            ..Default::default()
        };
        let executor = test_executor_with_policy(
            vec![ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            policy,
        );

        let tool_call = bash_tool_call("echo hello");
        let result = executor.execute(&tool_call).await;

        match result {
            Err(ToolError::ApprovalRequired { call_id, command }) => {
                assert_eq!(call_id, "call_1");
                assert_eq!(command, "echo hello");
            }
            _ => panic!("Expected ApprovalRequired error"),
        }
    }

    #[tokio::test]
    async fn execute_allows_pre_approved_command_in_ask_mode() {
        let policy = ToolPolicy {
            mode: PolicyMode::Ask,
            allow: vec!["bash:echo *".to_string()],
            ..Default::default()
        };
        let temp_dir = TempDir::new().unwrap();
        let sandbox = Arc::new(TrustSandbox);
        let executor = ToolExecutor::new(
            vec![ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            sandbox,
            temp_dir.path().to_path_buf(),
            policy,
            "test-agent".to_string(),
        );

        let tool_call = bash_tool_call("echo hello");
        let result = executor.execute(&tool_call).await;

        assert!(result.is_ok());
        assert!(result.unwrap().content.contains("hello"));
    }

    // ------------------------------------------------------------------------
    // execute_bypassing_policy - Approved execution
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn execute_bypassing_policy_ignores_deny_list() {
        let policy = ToolPolicy {
            mode: PolicyMode::Dangerous,
            deny: vec!["bash:*".to_string()], // Deny everything
            ..Default::default()
        };
        let temp_dir = TempDir::new().unwrap();
        let sandbox = Arc::new(TrustSandbox);
        let executor = ToolExecutor::new(
            vec![ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            sandbox,
            temp_dir.path().to_path_buf(),
            policy,
            "test-agent".to_string(),
        );

        let tool_call = bash_tool_call("echo bypassed");
        let result = executor.execute_bypassing_policy(&tool_call).await;

        // Should succeed despite deny policy
        assert!(result.is_ok());
        assert!(result.unwrap().content.contains("bypassed"));
    }

    // ------------------------------------------------------------------------
    // with_session_id - Builder pattern
    // ------------------------------------------------------------------------

    #[test]
    fn with_session_id_sets_session() {
        let executor = test_executor(vec![]).with_session_id("session-123".to_string());
        assert_eq!(executor.session_id, Some("session-123".to_string()));
    }

    // ------------------------------------------------------------------------
    // has_tools - Tool availability check
    // ------------------------------------------------------------------------

    #[test]
    fn has_tools_returns_false_when_empty() {
        let executor = test_executor(vec![]);
        assert!(!executor.has_tools());
    }

    #[test]
    fn has_tools_returns_true_when_configured() {
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "bash".to_string(),
        }]);
        assert!(executor.has_tools());
    }

    // ------------------------------------------------------------------------
    // load_readme - README caching
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn load_readme_returns_none_for_builtin() {
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "bash".to_string(),
        }]);

        let readme = executor.load_readme("bash").await;
        assert!(readme.is_none());
    }

    #[tokio::test]
    async fn load_readme_returns_none_for_unknown_tool() {
        let executor = test_executor(vec![]);

        let readme = executor.load_readme("nonexistent").await;
        assert!(readme.is_none());
    }

    #[tokio::test]
    async fn load_readme_reads_and_caches_file() {
        let temp_dir = TempDir::new().unwrap();
        let readme_path = temp_dir.path().join("tools").join("git-helper.md");
        std::fs::create_dir_all(readme_path.parent().unwrap()).unwrap();
        std::fs::write(&readme_path, "# Git Helper\n\nUsage instructions.").unwrap();

        let executor = test_executor_with_dir(
            vec![ToolConfig::Cli {
                name: "git-helper".to_string(),
                command: "./tools/git-helper.sh".to_string(),
                readme: Some("tools/git-helper.md".to_string()),
                description: None,
            }],
            &temp_dir,
        );

        // First load
        let readme = executor.load_readme("git-helper").await;
        assert!(readme.is_some());
        assert!(readme.as_ref().unwrap().contains("Git Helper"));

        // Second load should use cache
        let readme2 = executor.load_readme("git-helper").await;
        assert_eq!(readme, readme2);
    }

    #[tokio::test]
    async fn load_readme_returns_none_for_missing_file() {
        let temp_dir = TempDir::new().unwrap();

        let executor = test_executor_with_dir(
            vec![ToolConfig::Cli {
                name: "git-helper".to_string(),
                command: "./tools/git-helper.sh".to_string(),
                readme: Some("nonexistent.md".to_string()),
                description: None,
            }],
            &temp_dir,
        );

        let readme = executor.load_readme("git-helper").await;
        assert!(readme.is_none());
    }
}
