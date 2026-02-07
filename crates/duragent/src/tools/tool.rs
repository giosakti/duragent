//! Tool trait for extensible tool execution.
//!
//! This module defines the `Tool` trait that allows tools to be implemented
//! as self-contained structs with their own dependencies, rather than relying
//! on match-statement dispatch in the executor.

use std::sync::Arc;

use async_trait::async_trait;

use super::error::ToolError;
use super::executor::ToolResult;
use crate::llm::ToolDefinition;

/// A tool that can be executed by the tool executor.
///
/// Each tool implementation holds its own dependencies (sandbox, scheduler, etc.)
/// and knows how to execute itself. This allows adding new tools without modifying
/// the executor.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The unique name of this tool.
    fn name(&self) -> &str;

    /// Generate the LLM tool definition for this tool.
    ///
    /// If the tool has a README, implementations should append it to the
    /// description so the LLM has full context on how to use the tool.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given JSON arguments.
    ///
    /// Returns a ToolResult on success, or a ToolError on failure.
    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError>;

    /// Extended documentation for this tool, if any.
    ///
    /// This is typically loaded from a README file for CLI tools.
    /// Built-in tools return None.
    fn readme(&self) -> Option<&str> {
        None
    }
}

/// Type alias for a shared tool reference.
pub type SharedTool = Arc<dyn Tool>;
