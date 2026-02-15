//! Tool executor for running tools in agentic workflows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::debug;

use super::discovery::discover_all_tools;
use super::error::ToolError;
use super::factory::{ReloadDeps, create_tools};
use super::notify::send_notification;
use super::tool::Tool;
use crate::agent::{NotifyConfig, PolicyDecision, ToolPolicy, ToolType};
use crate::config::DEFAULT_TOOLS_DIR;
use crate::llm::{ToolCall, ToolDefinition};
use crate::sandbox::ExecResult;

// ============================================================================
// Types
// ============================================================================

/// Maximum bytes of tool output to keep before truncating.
/// This prevents OOM from tools that dump enormous output (e.g., `cat /dev/urandom`).
/// The downstream token-based truncation in the agentic loop handles LLM context fitting;
/// this limit prevents the large string from ever entering memory.
const MAX_OUTPUT_BYTES: usize = 256 * 1024; // 256 KB

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

        if content.len() > MAX_OUTPUT_BYTES {
            // Truncate at a char boundary
            let mut end = MAX_OUTPUT_BYTES;
            while end > 0 && !content.is_char_boundary(end) {
                end -= 1;
            }
            content.truncate(end);
            content.push_str("\n\n[output truncated at 256 KB]");
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
    /// Tool implementations by name.
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Tool policy for command filtering.
    policy: ToolPolicy,
    /// Notification configuration.
    notify_config: NotifyConfig,
    /// Session ID for notifications (optional).
    session_id: Option<String>,
    /// Agent name for notifications.
    agent_name: String,
    /// Dependencies for rebuilding tools mid-session via `reload_tools`.
    reload_deps: Option<ReloadDeps>,
}

impl ToolExecutor {
    /// Create a new tool executor with a policy and agent name.
    ///
    /// Use `register()` or `register_all()` to add tools after construction.
    pub fn new(policy: ToolPolicy, agent_name: String) -> Self {
        let notify_config = policy.notify.clone();

        Self {
            tools: HashMap::new(),
            policy,
            notify_config,
            session_id: None,
            agent_name,
            reload_deps: None,
        }
    }

    /// Register a single tool.
    pub fn register(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    /// Register multiple tools.
    pub fn register_all(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        for tool in tools {
            self.tools.insert(tool.name().to_string(), tool);
        }
        self
    }

    /// Set the session ID for notifications.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the reload dependencies for mid-session tool rebuilds.
    pub fn with_reload_deps(mut self, deps: ReloadDeps) -> Self {
        self.reload_deps = Some(deps);
        self
    }

    /// Replace all tools, preserving memory tools and `reload_tools`.
    ///
    /// Used by the agentic loop after `reload_tools` to rebuild the executor
    /// with newly discovered tools while keeping session-bound tools intact.
    pub fn replace_tools(&mut self, new_tools: Vec<Arc<dyn Tool>>) {
        const PRESERVED_TOOLS: &[&str] =
            &["memory", "reload_tools", "background_process", "session"];

        // Extract preserved tools before clearing
        let preserved: Vec<Arc<dyn Tool>> = self
            .tools
            .values()
            .filter(|t| PRESERVED_TOOLS.contains(&t.name()))
            .cloned()
            .collect();

        self.tools.clear();

        // Re-insert preserved tools
        for tool in preserved {
            self.tools.insert(tool.name().to_string(), tool);
        }

        // Insert new tools (preserved tools win on collision)
        for tool in new_tools {
            self.tools.entry(tool.name().to_string()).or_insert(tool);
        }
    }

    /// Rebuild tool set using stored `reload_deps`.
    ///
    /// Called after `reload_tools` to re-discover tools from directories
    /// and merge with explicit tools (explicit wins on name collision).
    /// No-op if `reload_deps` was not set.
    pub fn rebuild(&mut self) {
        let Some(ref deps) = self.reload_deps else {
            return;
        };

        let tool_deps = super::factory::ToolDependencies {
            sandbox: deps.sandbox.clone(),
            agent_dir: deps.agent_dir.clone(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: deps.workspace_tools_dir.clone(),
            // Process and peek tools hold session-specific state.
            // They survive rebuild via PRESERVED_TOOLS in replace_tools().
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let explicit = create_tools(&deps.agent_tool_configs, &tool_deps);

        let mut discovery_dirs = vec![deps.agent_dir.join(DEFAULT_TOOLS_DIR)];
        if let Some(ref ws) = deps.workspace_tools_dir {
            discovery_dirs.push(ws.clone());
        }
        let discovered = discover_all_tools(&discovery_dirs, &deps.sandbox);

        // Merge: explicit wins on name collision
        let explicit_names: HashSet<String> =
            explicit.iter().map(|t| t.name().to_string()).collect();
        let mut merged = explicit;
        for tool in discovered {
            if !explicit_names.contains(tool.name()) {
                merged.push(tool);
            }
        }

        debug!(
            tool_count = merged.len(),
            "Rebuilt executor after reload_tools"
        );
        self.replace_tools(merged);
    }

    /// Execute a tool call and return the result.
    ///
    /// Checks policy before execution:
    /// - If denied by policy, returns `PolicyDenied` error
    /// - If approval required (ask mode), returns `ApprovalRequired` error
    /// - If allowed, executes and optionally sends notifications
    pub async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.function.name;
        let tool = self.find_tool(tool_name)?;

        // Determine tool type and invocation string for policy check
        let (tool_type, invocation) = self.get_tool_type_and_invocation(tool_name, tool_call);

        // Check policy
        match self.policy.check(tool_type, &invocation) {
            PolicyDecision::Deny => {
                return Err(ToolError::PolicyDenied(invocation));
            }
            PolicyDecision::Ask => {
                return Err(ToolError::ApprovalRequired {
                    call_id: tool_call.id.clone(),
                    command: invocation,
                    tool_type,
                });
            }
            PolicyDecision::Allow => {
                // Continue with execution
            }
        }

        self.execute_tool(tool, tool_call, tool_type, &invocation)
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
        let tool = self.find_tool(tool_name)?;

        // Determine tool type and invocation string (for notifications)
        let (tool_type, invocation) = self.get_tool_type_and_invocation(tool_name, tool_call);

        self.execute_tool(tool, tool_call, tool_type, &invocation)
            .await
    }

    /// Generate tool definitions for the LLM.
    ///
    /// If `filter` is provided, only tools whose names are in the filter
    /// will be included. This controls what the LLM sees, not what can execute.
    pub fn tool_definitions(&self, filter: Option<&HashSet<String>>) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|(name, _)| filter.is_none_or(|f| f.contains(*name)))
            .map(|(_, tool)| tool.definition())
            .collect()
    }

    /// Check if any tools are configured.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Look up a tool by name (case-insensitive fallback).
    fn find_tool(&self, name: &str) -> Result<&Arc<dyn Tool>, ToolError> {
        if let Some(tool) = self.tools.get(name) {
            return Ok(tool);
        }
        // Case-insensitive fallback: LLMs sometimes capitalize tool names
        let lower = name.to_lowercase();
        for (key, tool) in &self.tools {
            if key.to_lowercase() == lower {
                return Ok(tool);
            }
        }
        Err(ToolError::NotFound(name.to_string()))
    }

    /// Look up a tool Arc by name (case-insensitive fallback), returning Option.
    fn find_tool_arc(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.find_tool(name).ok()
    }

    /// Get tool type and invocation string for policy checks.
    ///
    /// For builtin tools, extracts the `action` field from JSON arguments
    /// and returns `tool_name:action` as the invocation string. This enables
    /// policy patterns like `builtin:schedule:create` or `builtin:web:*`.
    fn get_tool_type_and_invocation(
        &self,
        tool_name: &str,
        tool_call: &ToolCall,
    ) -> (ToolType, String) {
        let tool = self.find_tool_arc(tool_name);
        let tool_type = tool.map(|t| t.tool_type()).unwrap_or(ToolType::Builtin);

        let invocation = if tool_type == ToolType::Bash {
            extract_bash_command(&tool_call.function.arguments)
        } else if tool_type == ToolType::Builtin {
            // For builtin tools, append :action if present in arguments
            match extract_action(&tool_call.function.arguments) {
                Some(action) => format!("{}:{}", tool_name, action),
                None => tool_name.to_string(),
            }
        } else {
            tool_name.to_string()
        };

        (tool_type, invocation)
    }

    /// Internal: execute tool and send notifications.
    async fn execute_tool(
        &self,
        tool: &Arc<dyn Tool>,
        tool_call: &ToolCall,
        tool_type: ToolType,
        invocation: &str,
    ) -> Result<ToolResult, ToolError> {
        // Execute the tool
        debug!(
            tool = %tool_call.function.name,
            arguments = %tool_call.function.arguments,
            "Executing tool"
        );
        let result = tool.execute(&tool_call.function.arguments).await;

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
}

// ============================================================================
// Private Helpers
// ============================================================================

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

/// Extract the action field from tool arguments, if present.
pub(crate) fn extract_action(arguments: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ActionArgs {
        action: Option<String>,
    }

    serde_json::from_str::<ActionArgs>(arguments)
        .ok()
        .and_then(|args| args.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{PolicyMode, ToolConfig};
    use crate::sandbox::TrustSandbox;
    use crate::tools::{ToolDependencies, create_tools};
    use tempfile::TempDir;

    // ------------------------------------------------------------------------
    // Test Helpers
    // ------------------------------------------------------------------------

    fn test_executor(tools: Vec<ToolConfig>) -> ToolExecutor {
        let temp_dir = TempDir::new().unwrap();
        let sandbox = Arc::new(TrustSandbox);
        let deps = ToolDependencies {
            sandbox,
            agent_dir: temp_dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let tools = create_tools(&tools, &deps);
        ToolExecutor::new(ToolPolicy::default(), "test-agent".to_string()).register_all(tools)
    }

    fn test_executor_with_policy(tools: Vec<ToolConfig>, policy: ToolPolicy) -> ToolExecutor {
        let temp_dir = TempDir::new().unwrap();
        let sandbox = Arc::new(TrustSandbox);
        let deps = ToolDependencies {
            sandbox,
            agent_dir: temp_dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let tools = create_tools(&tools, &deps);
        ToolExecutor::new(policy, "test-agent".to_string()).register_all(tools)
    }

    fn test_executor_with_dir(tools: Vec<ToolConfig>, dir: &TempDir) -> ToolExecutor {
        let sandbox = Arc::new(TrustSandbox);
        let deps = ToolDependencies {
            sandbox,
            agent_dir: dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let tools = create_tools(&tools, &deps);
        ToolExecutor::new(ToolPolicy::default(), "test-agent".to_string()).register_all(tools)
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
        // Unknown builtins are now skipped by create_tools, so no tools are created
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "unknown-builtin".to_string(),
        }]);

        let defs = executor.tool_definitions(None);
        assert_eq!(defs.len(), 0);
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
            Err(ToolError::ApprovalRequired {
                call_id,
                command,
                tool_type,
            }) => {
                assert_eq!(call_id, "call_1");
                assert_eq!(command, "echo hello");
                assert_eq!(tool_type, ToolType::Bash);
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
        let deps = ToolDependencies {
            sandbox,
            agent_dir: temp_dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let tools = create_tools(
            &[ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            &deps,
        );
        let executor = ToolExecutor::new(policy, "test-agent".to_string()).register_all(tools);

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
        let deps = ToolDependencies {
            sandbox,
            agent_dir: temp_dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
            session_registry: None,
        };
        let tools = create_tools(
            &[ToolConfig::Builtin {
                name: "bash".to_string(),
            }],
            &deps,
        );
        let executor = ToolExecutor::new(policy, "test-agent".to_string()).register_all(tools);

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
        let executor = ToolExecutor::new(ToolPolicy::default(), "test-agent".to_string())
            .with_session_id("session-123".to_string());
        assert_eq!(executor.session_id, Some("session-123".to_string()));
    }

    // ------------------------------------------------------------------------
    // has_tools - Tool availability check
    // ------------------------------------------------------------------------

    #[test]
    fn has_tools_returns_false_when_empty() {
        let executor = ToolExecutor::new(ToolPolicy::default(), "test-agent".to_string());
        assert!(!executor.has_tools());
    }

    #[test]
    fn has_tools_returns_true_when_configured() {
        let executor = test_executor(vec![ToolConfig::Builtin {
            name: "bash".to_string(),
        }]);
        assert!(executor.has_tools());
    }
}
