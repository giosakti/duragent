//! Tool factory for creating tools from configuration.
//!
//! This module provides the `create_tools` function that instantiates
//! Tool implementations from ToolConfig entries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::{AgentSpec, ToolConfig, ToolPolicy};
use crate::config::DEFAULT_TOOLS_DIR;
use crate::memory::Memory;
use crate::process::ProcessRegistryHandle;
use crate::sandbox::Sandbox;
use crate::scheduler::SchedulerHandle;

use super::builtins::bash::BashTool;
use super::builtins::cli::CliTool;
use super::builtins::manage_process::ProcessTool;
use super::builtins::memory::{RecallTool, ReflectTool, RememberTool, UpdateWorldTool};
use super::builtins::reload::ReloadToolsTool;
use super::builtins::schedule::{
    CancelScheduleTool, ListSchedulesTool, ScheduleTaskTool, ToolExecutionContext,
};
use super::builtins::spawn_process::SpawnProcessTool;
use super::builtins::web_fetch::WebFetchTool;
use super::builtins::web_search::WebSearchTool;
use super::discovery::discover_all_tools;
use super::executor::ToolExecutor;
use super::tool::SharedTool;

/// All recognized builtin tool names.
///
/// Used at agent load time to warn about typos or unknown tool names.
pub const KNOWN_BUILTIN_TOOLS: &[&str] = &[
    "bash",
    "schedule_task",
    "list_schedules",
    "cancel_schedule",
    "web_search",
    "web_fetch",
    "reload_tools",
    "spawn_process",
    "manage_process",
];

/// Dependencies needed for creating tools.
pub struct ToolDependencies {
    /// Sandbox for executing commands.
    pub sandbox: Arc<dyn Sandbox>,
    /// Base directory for the agent.
    pub agent_dir: PathBuf,
    /// Scheduler handle for schedule tools (optional).
    pub scheduler: Option<SchedulerHandle>,
    /// Execution context for schedule tools (optional).
    pub execution_context: Option<ToolExecutionContext>,
    /// Workspace-level tools directory for discovery (optional).
    pub workspace_tools_dir: Option<PathBuf>,
    /// Process registry handle for background process tools (optional).
    pub process_registry: Option<ProcessRegistryHandle>,
    /// Session ID for process tools (optional).
    pub session_id: Option<String>,
    /// Agent name for process tools (optional).
    pub agent_name: Option<String>,
}

/// Dependencies needed for rebuilding tools mid-session via `reload_tools`.
pub struct ReloadDeps {
    pub sandbox: Arc<dyn Sandbox>,
    pub agent_dir: PathBuf,
    pub workspace_tools_dir: Option<PathBuf>,
    pub agent_tool_configs: Vec<ToolConfig>,
}

/// Create tools from configuration.
///
/// Takes a list of ToolConfig entries and creates the corresponding Tool
/// implementations with their dependencies injected.
pub fn create_tools(configs: &[ToolConfig], deps: &ToolDependencies) -> Vec<SharedTool> {
    configs
        .iter()
        .filter_map(|config| create_tool(config, deps))
        .collect()
}

/// Create a single tool from configuration.
fn create_tool(config: &ToolConfig, deps: &ToolDependencies) -> Option<SharedTool> {
    match config {
        ToolConfig::Builtin { name } => create_builtin_tool(name, deps),
        ToolConfig::Cli {
            name,
            command,
            description,
            readme,
        } => {
            let tool = CliTool::new(
                deps.sandbox.clone(),
                deps.agent_dir.clone(),
                name.clone(),
                command.clone(),
                description.clone(),
                readme.as_deref(),
            );
            Some(Arc::new(tool))
        }
    }
}

/// Create a builtin tool by name.
fn create_builtin_tool(name: &str, deps: &ToolDependencies) -> Option<SharedTool> {
    match name {
        "bash" => {
            let tool = BashTool::new(deps.sandbox.clone(), deps.agent_dir.clone());
            Some(Arc::new(tool))
        }
        "schedule_task" => {
            let (scheduler, ctx) = get_schedule_deps(deps)?;
            let tool = ScheduleTaskTool::new(scheduler, ctx);
            Some(Arc::new(tool))
        }
        "list_schedules" => {
            let (scheduler, ctx) = get_schedule_deps(deps)?;
            let tool = ListSchedulesTool::new(scheduler, ctx);
            Some(Arc::new(tool))
        }
        "cancel_schedule" => {
            let (scheduler, ctx) = get_schedule_deps(deps)?;
            let tool = CancelScheduleTool::new(scheduler, ctx);
            Some(Arc::new(tool))
        }
        "web_search" => {
            let tool = WebSearchTool::new()?;
            Some(Arc::new(tool))
        }
        "web_fetch" => Some(Arc::new(WebFetchTool::new())),
        "reload_tools" => {
            let mut dirs = vec![deps.agent_dir.join(DEFAULT_TOOLS_DIR)];
            if let Some(ref ws) = deps.workspace_tools_dir {
                dirs.push(ws.clone());
            }
            Some(Arc::new(ReloadToolsTool::new(dirs, deps.sandbox.clone())))
        }
        "spawn_process" => {
            let (registry, session_id, agent_name) = get_process_deps(deps)?;
            let ctx = deps.execution_context.as_ref();
            let tool = SpawnProcessTool::new(
                registry,
                session_id,
                agent_name,
                ctx.and_then(|c| c.gateway.clone()),
                ctx.and_then(|c| c.chat_id.clone()),
            );
            Some(Arc::new(tool))
        }
        "manage_process" => {
            let (registry, session_id, _) = get_process_deps(deps)?;
            let tool = ProcessTool::new(registry, session_id);
            Some(Arc::new(tool))
        }
        _ => {
            // Unknown builtin - return None to skip
            // The executor will handle this as a missing tool if called
            tracing::warn!(name = %name, "Unknown builtin tool");
            None
        }
    }
}

/// Get schedule dependencies, returning None if not available.
fn get_schedule_deps(deps: &ToolDependencies) -> Option<(SchedulerHandle, ToolExecutionContext)> {
    let scheduler = deps.scheduler.clone()?;
    let ctx = deps.execution_context.clone()?;
    Some((scheduler, ctx))
}

/// Get process dependencies, returning None if not available.
fn get_process_deps(deps: &ToolDependencies) -> Option<(ProcessRegistryHandle, String, String)> {
    let registry = deps.process_registry.clone()?;
    let session_id = deps.session_id.clone()?;
    let agent_name = deps.agent_name.clone()?;
    Some((registry, session_id, agent_name))
}

/// Create memory tools for an agent.
///
/// Call this when an agent has memory configured. Returns all four memory tools:
/// - `recall` — Load memory context
/// - `remember` — Append to daily log
/// - `reflect` — Rewrite MEMORY.md
/// - `update_world` — Append to world knowledge
fn create_memory_tools(memory: Arc<Memory>) -> Vec<SharedTool> {
    vec![
        Arc::new(RecallTool::new(memory.clone())) as SharedTool,
        Arc::new(RememberTool::new(memory.clone())) as SharedTool,
        Arc::new(ReflectTool::new(memory.clone())) as SharedTool,
        Arc::new(UpdateWorldTool::new(memory)) as SharedTool,
    ]
}

/// Build a fully configured tool executor for an agent.
///
/// Creates tools from agent config, registers memory tools if configured,
/// and sets the session ID. The caller provides `ToolDependencies` for the
/// parts that vary across call sites (scheduler, execution_context).
pub fn build_executor(
    agent: &AgentSpec,
    agent_name: &str,
    session_id: &str,
    policy: ToolPolicy,
    deps: ToolDependencies,
    world_memory_path: &Path,
) -> ToolExecutor {
    let explicit_tools = create_tools(&agent.tools, &deps);

    // Discover tools from agent and workspace directories
    let mut discovery_dirs = vec![deps.agent_dir.join(DEFAULT_TOOLS_DIR)];
    if let Some(ref ws) = deps.workspace_tools_dir {
        discovery_dirs.push(ws.clone());
    }
    let discovered = discover_all_tools(&discovery_dirs, &deps.sandbox);
    let merged = merge_tools(explicit_tools, discovered);

    let mut executor = ToolExecutor::new(policy, agent_name.to_string())
        .register_all(merged)
        .with_session_id(session_id.to_string());

    if agent.memory.is_some() {
        let memory = Arc::new(Memory::new(
            world_memory_path.to_path_buf(),
            agent.agent_dir.join("memory"),
        ));
        executor = executor.register_all(create_memory_tools(memory));
    }

    executor
}

/// Merge explicit tools with discovered tools. Explicit tools win on name collision.
fn merge_tools(explicit: Vec<SharedTool>, discovered: Vec<SharedTool>) -> Vec<SharedTool> {
    let explicit_names: std::collections::HashSet<String> =
        explicit.iter().map(|t| t.name().to_string()).collect();
    let mut merged = explicit;
    for tool in discovered {
        if !explicit_names.contains(tool.name()) {
            merged.push(tool);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::TrustSandbox;
    use tempfile::TempDir;

    fn test_deps() -> (TempDir, ToolDependencies) {
        let temp_dir = TempDir::new().unwrap();
        let deps = ToolDependencies {
            sandbox: Arc::new(TrustSandbox),
            agent_dir: temp_dir.path().to_path_buf(),
            scheduler: None,
            execution_context: None,
            workspace_tools_dir: None,
            process_registry: None,
            session_id: None,
            agent_name: None,
        };
        (temp_dir, deps)
    }

    #[test]
    fn create_tools_creates_bash_tool() {
        let (_temp, deps) = test_deps();
        let configs = vec![ToolConfig::Builtin {
            name: "bash".to_string(),
        }];

        let tools = create_tools(&configs, &deps);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "bash");
    }

    #[test]
    fn create_tools_creates_cli_tool() {
        let (_temp, deps) = test_deps();
        let configs = vec![ToolConfig::Cli {
            name: "my-tool".to_string(),
            command: "./my-tool.sh".to_string(),
            readme: None,
            description: Some("My custom tool".to_string()),
        }];

        let tools = create_tools(&configs, &deps);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "my-tool");
    }

    #[test]
    fn create_tools_skips_schedule_tools_without_deps() {
        let (_temp, deps) = test_deps();
        let configs = vec![
            ToolConfig::Builtin {
                name: "bash".to_string(),
            },
            ToolConfig::Builtin {
                name: "schedule_task".to_string(),
            },
        ];

        let tools = create_tools(&configs, &deps);

        // Only bash should be created, schedule_task skipped due to missing deps
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "bash");
    }

    #[test]
    fn create_tools_skips_unknown_builtin() {
        let (_temp, deps) = test_deps();
        let configs = vec![ToolConfig::Builtin {
            name: "unknown-builtin".to_string(),
        }];

        let tools = create_tools(&configs, &deps);

        assert!(tools.is_empty());
    }

    #[test]
    fn create_tools_handles_multiple_tools() {
        let (_temp, deps) = test_deps();
        let configs = vec![
            ToolConfig::Builtin {
                name: "bash".to_string(),
            },
            ToolConfig::Cli {
                name: "deploy".to_string(),
                command: "./deploy.sh".to_string(),
                readme: None,
                description: None,
            },
        ];

        let tools = create_tools(&configs, &deps);

        assert_eq!(tools.len(), 2);
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"deploy"));
    }

    #[test]
    fn create_tools_creates_web_fetch_tool() {
        let (_temp, deps) = test_deps();
        let configs = vec![ToolConfig::Builtin {
            name: "web_fetch".to_string(),
        }];

        let tools = create_tools(&configs, &deps);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "web_fetch");
    }

    #[test]
    fn known_builtin_tools_matches_factory() {
        // Every tool in KNOWN_BUILTIN_TOOLS should be handled by create_builtin_tool.
        // "bash" is the only one that doesn't need scheduler deps.
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"bash"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"schedule_task"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"list_schedules"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"cancel_schedule"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"web_search"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"web_fetch"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"reload_tools"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"spawn_process"));
        assert!(KNOWN_BUILTIN_TOOLS.contains(&"manage_process"));
        assert_eq!(KNOWN_BUILTIN_TOOLS.len(), 9);
    }

    #[test]
    fn create_memory_tools_returns_all_four_tools() {
        let temp_dir = TempDir::new().unwrap();
        let world_dir = temp_dir.path().join("world");
        let agent_memory_dir = temp_dir.path().join("agent-memory");
        let memory = Arc::new(Memory::new(world_dir, agent_memory_dir));

        let tools = create_memory_tools(memory);

        assert_eq!(tools.len(), 4);
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"recall"));
        assert!(names.contains(&"remember"));
        assert!(names.contains(&"reflect"));
        assert!(names.contains(&"update_world"));
    }
}
