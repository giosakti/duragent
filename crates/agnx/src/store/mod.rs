//! Storage abstraction layer for Agnx.
//!
//! This module defines trait interfaces for all persistence operations,
//! with file-based implementations provided in the `file` submodule.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Application / Domain                         │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │ uses traits
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     store/ (traits)                             │
//! │  SessionStore, ScheduleStore, RunLogStore, PolicyStore, ...    │
//! └──────────────────────────┬──────────────────────────────────────┘
//!                            │ implementations
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     store/file/                                 │
//! │  FileSessionStore, FileScheduleStore, FileRunLogStore, ...     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Adding a New Backend
//!
//! To add a new storage backend (e.g., PostgreSQL):
//!
//! 1. Create a new submodule: `store/postgresql/`
//! 2. Implement the relevant traits for your backend
//! 3. Wire up via dependency injection in `server.rs`
//!
//! # Naming Conventions
//!
//! - `list` - enumerate all entities
//! - `load` - read a single entity, returns `Option` if not found
//! - `save` - create or update (upsert semantics, must be atomic)
//! - `delete` - remove an entity
//! - `append` - add to an append-only log

pub mod error;

mod agent;
mod policy;
mod run_log;
mod schedule;
mod session;

pub mod file;

// Re-export traits
pub use agent::{AgentCatalog, AgentScanResult, ScanWarning};
pub use error::{StorageError, StorageResult};
pub use policy::PolicyStore;
pub use run_log::RunLogStore;
pub use schedule::ScheduleStore;
pub use session::SessionStore;
