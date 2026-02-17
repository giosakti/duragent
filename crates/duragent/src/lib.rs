//! Duragent - A minimal and fast self-hosted runtime for durable and portable AI agents.

// ============================================================================
// Always Available
// ============================================================================

pub use duragent_client::api;
pub use duragent_client::client;
pub use duragent_client::sse_parser;

pub mod auth;
pub mod build_info;
pub mod config;
pub mod launcher;
pub mod llm;

// ============================================================================
// Server-only (behind `server` feature)
// ============================================================================

#[cfg(feature = "server")]
pub mod agent;
#[cfg(feature = "server")]
pub mod background;
#[cfg(feature = "server")]
pub mod context;
#[cfg(feature = "server")]
pub mod gateway;
#[cfg(feature = "server")]
pub mod handlers;
#[cfg(feature = "server")]
pub mod memory;
#[cfg(feature = "server")]
pub mod process;
#[cfg(feature = "server")]
pub mod sandbox;
#[cfg(feature = "server")]
pub mod scheduler;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod session;
#[cfg(feature = "server")]
pub mod store;
#[cfg(feature = "server")]
pub mod sync;
#[cfg(feature = "server")]
pub mod tools;
