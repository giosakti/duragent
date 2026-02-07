//! Agent loading and registry types for Duragent Format.
//!
//! This module is responsible for parsing `agent.yaml`, loading referenced Markdown files,
//! and maintaining an in-memory registry (`AgentStore`) used by the HTTP API.

pub const API_VERSION_V1ALPHA1: &str = "duragent/v1alpha1";
pub const KIND_AGENT: &str = "Agent";

mod error;
mod policy;
mod spec;
mod store;

pub use error::{AgentLoadError, AgentLoadWarning};
pub use policy::{
    Delivery, NotifyConfig, PolicyDecision, PolicyLocks, PolicyMode, ToolPolicy, ToolType,
};
pub use spec::{
    AgentFileRefs, AgentMetadata, AgentSessionConfig, AgentSpec, DEFAULT_MAX_TOOL_ITERATIONS,
    LoadedAgentFiles, ModelConfig, OnDisconnect, ToolConfig,
};
pub use store::{AgentStore, log_scan_warnings};
