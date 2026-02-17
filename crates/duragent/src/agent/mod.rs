//! Agent loading and registry types for Duragent Format.
//!
//! This module is responsible for parsing `agent.yaml`, loading referenced Markdown files,
//! and maintaining an in-memory registry (`AgentStore`) used by the HTTP API.

pub const API_VERSION_V1ALPHA1: &str = "duragent/v1alpha1";
pub const KIND_AGENT: &str = "Agent";

// Re-export all domain types from duragent-types
pub use duragent_types::agent::*;

// Local modules (server-only logic that can't move to duragent-types)
mod access_eval;
mod error;
mod parsing;
mod policy_eval;
mod policy_ext;
pub mod skill;
mod spec_eval;
mod store;

pub use access_eval::{check_access, resolve_sender_disposition};
pub use error::{AgentLoadError, AgentLoadWarning};
pub use parsing::{parse_agent_file_refs, parse_agent_yaml, validate_builtin_tools};
pub use policy_eval::ToolPolicyEval;
pub use policy_ext::{PolicyLocks, add_policy_pattern_and_save};
pub use skill::SkillParseError;
pub use spec_eval::{HooksConfigEval, ModelConfigEval};
pub use store::{AgentStore, log_scan_warnings};
