//! Scheduler for time-based task execution.
//!
//! Provides runtime scheduling that lets agents create time-based triggers to:
//! - Send messages (simple notifications/reminders)
//! - Execute tasks (run agent work with tools, summarize results)
//!
//! Both modes deliver results via any gateway (Telegram, Discord, etc.).

// Re-export scheduler domain types from duragent-types
pub use duragent_types::scheduler::*;

pub mod error;
pub mod schedule_cache;
mod schedule_eval;
pub mod service;

pub use schedule_eval::{RetryConfigEval, generate_schedule_id};

pub use error::{Result, SchedulerError};
pub use schedule_cache::{LoadResult, ScheduleCache};
pub use service::{SchedulerConfig, SchedulerHandle, SchedulerService};
