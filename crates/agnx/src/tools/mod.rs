//! Tool execution for agentic capabilities.
//!
//! This module provides the infrastructure for executing tools in agentic workflows.
//! Tools can be built-in (like `bash`) or CLI-based (custom scripts).

mod bash;
mod cli;
mod error;
mod executor;
mod factory;
pub mod memory;
mod notify;
pub mod schedule;
mod tool;

pub use error::ToolError;
pub use executor::{ToolExecutor, ToolResult};
pub use factory::{ToolDependencies, create_memory_tools, create_tools};
pub use notify::send_notification;
pub use schedule::ToolExecutionContext;
pub use tool::{SharedTool, Tool};
