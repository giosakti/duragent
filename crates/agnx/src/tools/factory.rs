//! Tool factory for creating tools from configuration.
//!
//! This module provides the `create_tools` function that instantiates
//! Tool implementations from ToolConfig entries.

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::ToolConfig;
use crate::memory::Memory;
use crate::sandbox::Sandbox;
use crate::scheduler::SchedulerHandle;

use super::bash::BashTool;
use super::cli::CliTool;
use super::memory::{RecallTool, ReflectTool, RememberTool, UpdateWorldTool};
use super::schedule::{
    CancelScheduleTool, ListSchedulesTool, ScheduleTaskTool, ToolExecutionContext,
};
use super::tool::SharedTool;

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

/// Create memory tools for an agent.
///
/// Call this when an agent has memory configured. Returns all four memory tools:
/// - `recall` — Load memory context
/// - `remember` — Append to daily log
/// - `reflect` — Rewrite MEMORY.md
/// - `update_world` — Append to world knowledge
pub fn create_memory_tools(memory: Arc<Memory>) -> Vec<SharedTool> {
    vec![
        Arc::new(RecallTool::new(memory.clone())) as SharedTool,
        Arc::new(RememberTool::new(memory.clone())) as SharedTool,
        Arc::new(ReflectTool::new(memory.clone())) as SharedTool,
        Arc::new(UpdateWorldTool::new(memory)) as SharedTool,
    ]
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
