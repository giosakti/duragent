//! Agent loading and registry types for Duragent Format.
//!
//! This module is responsible for parsing `agent.yaml`, loading referenced Markdown files,
//! and maintaining an in-memory registry (`AgentStore`) used by the HTTP API.

pub const API_VERSION_V1ALPHA1: &str = "duragent/v1alpha1";
pub const KIND_AGENT: &str = "Agent";

pub mod access;
mod error;
mod policy;
pub mod skill;
mod spec;
mod store;

pub use access::{
    AccessConfig, ActivationMode, ContextBufferConfig, ContextBufferMode, DebounceConfig,
    DmAccessConfig, DmPolicy, GroupAccessConfig, GroupPolicy, OverflowStrategy, QueueConfig,
    QueueMode, SenderDisposition,
};
pub use error::{AgentLoadError, AgentLoadWarning};
pub use policy::{
    Delivery, NotifyConfig, PolicyDecision, PolicyLocks, PolicyMode, ToolPolicy, ToolType,
};
pub use skill::{SkillMetadata, SkillParseError};
pub use spec::{
    AgentFileRefs, AgentMetadata, AgentSessionConfig, AgentSpec, ContextConfig,
    DEFAULT_MAX_TOOL_ITERATIONS, LoadedAgentFiles, ModelConfig, OnDisconnect, ToolConfig,
    ToolResultTruncation,
};
pub use store::{AgentStore, log_scan_warnings};
