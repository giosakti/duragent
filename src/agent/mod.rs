//! Agent loading and registry types for Agnx Agent Format (AAF).
//!
//! This module is responsible for parsing `agent.yaml`, loading referenced Markdown files,
//! and maintaining an in-memory registry (`AgentStore`) used by the HTTP API.

pub const API_VERSION_V1ALPHA1: &str = "agnx/v1alpha1";
pub const KIND_AGENT: &str = "Agent";

mod error;
mod provider;
mod spec;
mod store;

pub use store::{AgentStore, log_scan_warnings, resolve_agents_dir};
