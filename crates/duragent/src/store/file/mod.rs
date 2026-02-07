//! File-based storage implementations.
//!
//! These implementations store data on the local filesystem using:
//! - YAML for structured documents (snapshots, schedules, agents)
//! - JSONL for append-only logs (events, run logs)
//!
//! All writes use atomic operations (temp file + rename) to prevent corruption.

mod agent;
mod policy;
mod run_log;
mod schedule;
mod session;

pub use agent::FileAgentCatalog;
pub use policy::FilePolicyStore;
pub use run_log::FileRunLogStore;
pub use schedule::FileScheduleStore;
pub use session::FileSessionStore;
