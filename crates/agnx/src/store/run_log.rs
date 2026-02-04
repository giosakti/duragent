//! Run log storage trait.
//!
//! Defines the interface for persisting schedule execution history.

use async_trait::async_trait;

use crate::scheduler::RunLogEntry;

use super::error::StorageResult;

/// Storage interface for schedule run logs.
///
/// Run logs are append-only records of schedule execution history.
#[async_trait]
pub trait RunLogStore: Send + Sync {
    /// Load recent entries from the schedule's run log.
    ///
    /// Returns the most recent `limit` entries in chronological order.
    async fn load_recent(&self, schedule_id: &str, limit: usize)
    -> StorageResult<Vec<RunLogEntry>>;

    /// Append an entry to the schedule's run log.
    ///
    /// May trigger automatic pruning if the log grows too large.
    async fn append(&self, schedule_id: &str, entry: &RunLogEntry) -> StorageResult<()>;

    /// Delete the run log for a schedule.
    ///
    /// Called when a schedule is deleted.
    async fn delete(&self, schedule_id: &str) -> StorageResult<()>;
}
