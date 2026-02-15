//! Tool execution for agentic capabilities.
//!
//! This module provides the infrastructure for executing tools in agentic workflows.
//! Tools can be built-in (like `bash`) or CLI-based (custom scripts).

mod builtins;
pub mod discovery;
mod error;
mod executor;
mod factory;
pub mod hooks;
mod notify;
mod tool;

pub use builtins::schedule;
pub use builtins::schedule::ToolExecutionContext;
pub use error::ToolError;
pub(crate) use executor::extract_action;
pub use executor::{ToolExecutor, ToolResult};
pub use factory::{
    KNOWN_BUILTIN_TOOLS, ReloadDeps, ToolDependencies, build_executor, create_tools,
};
pub use notify::send_notification;
pub use tool::{SharedTool, Tool};
