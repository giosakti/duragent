//! Agnx - A minimal and fast self-hosted runtime for durable and portable AI agents.

// ============================================================================
// Core Infrastructure
// ============================================================================

pub mod build_info;
pub mod config;
pub mod store;
pub mod sync;

// ============================================================================
// Server & HTTP
// ============================================================================

pub mod api;
pub mod handlers;
pub mod server;
pub mod sse_parser;

// ============================================================================
// Domain
// ============================================================================

pub mod agent;
pub mod context;
pub mod gateway;
pub mod llm;
pub mod memory;
pub mod sandbox;
pub mod scheduler;
pub mod session;
pub mod tools;

// ============================================================================
// Client & Utilities
// ============================================================================

pub mod background;
pub mod client;
pub mod launcher;
