//! Scheduler for time-based task execution.
//!
//! Provides runtime scheduling that lets agents create time-based triggers to:
//! - Send messages (simple notifications/reminders)
//! - Execute tasks (run agent work with tools, summarize results)
//!
//! Both modes deliver results via any gateway (Telegram, Discord, etc.).
//!
//! # Storage
//!
//! Schedules are persisted to `.agnx/schedules/{id}.yaml` and run logs
//! to `.agnx/schedules/runs/{id}.jsonl`.
//!
//! # Usage
//!
//! ```ignore
//! // Start the scheduler service
//! let config = SchedulerConfig { ... };
//! let service = SchedulerService::new(config);
//! let handle = service.start().await;
//!
//! // Create a schedule (typically via tool call)
//! let schedule = Schedule { ... };
//! handle.create_schedule(schedule).await?;
//!
//! // List schedules for an agent
//! let schedules = handle.list_schedules("my-agent").await;
//!
//! // Cancel a schedule
//! handle.cancel_schedule("sched_123", "my-agent").await?;
//! ```

pub mod error;
pub mod run_log;
pub mod schedule;
pub mod service;
pub mod store;

pub use error::{Result, SchedulerError};
pub use run_log::RunLog;
pub use schedule::{
    RetryConfig, RunLogEntry, RunStatus, Schedule, ScheduleDestination, ScheduleId,
    SchedulePayload, ScheduleState, ScheduleStatus, ScheduleTiming,
};
pub use service::{SchedulerConfig, SchedulerHandle, SchedulerService};
pub use store::{LoadResult, ScheduleStore};
