//! Tool execution for agentic capabilities.
//!
//! This module provides the infrastructure for executing tools in agentic workflows.
//! Tools can be built-in (like `bash`) or CLI-based (custom scripts).

mod bash;
mod cli;
mod error;
mod executor;
mod notify;

pub use error::ToolError;
pub use executor::{ToolExecutor, ToolResult};
pub use notify::send_notification;
