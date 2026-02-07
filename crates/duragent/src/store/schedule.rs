//! Schedule storage trait.
//!
//! Defines the interface for persisting schedule definitions.

use async_trait::async_trait;

use crate::scheduler::Schedule;

use super::error::StorageResult;

/// Storage interface for schedule persistence.
#[async_trait]
pub trait ScheduleStore: Send + Sync {
    /// List all schedules.
    ///
    /// Used for loading schedules on startup.
    async fn list(&self) -> StorageResult<Vec<Schedule>>;

    /// Load a schedule by ID.
    ///
    /// Returns `Ok(None)` if the schedule doesn't exist.
    async fn load(&self, id: &str) -> StorageResult<Option<Schedule>>;

    /// Create or update a schedule (upsert semantics).
    ///
    /// Must be atomic - either fully succeeds or has no effect.
    async fn save(&self, schedule: &Schedule) -> StorageResult<()>;

    /// Delete a schedule.
    ///
    /// No-op if the schedule doesn't exist.
    async fn delete(&self, id: &str) -> StorageResult<()>;
}
