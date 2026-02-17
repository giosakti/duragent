//! Client library and API types for Duragent.
//!
//! This crate provides shared types (API, LLM) and the HTTP client
//! used by both the `duragent` server crate and the `duragent-cli` REPL.

pub mod api;
pub mod client;
pub mod llm;
pub mod sse_parser;
