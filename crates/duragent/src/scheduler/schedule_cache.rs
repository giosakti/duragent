//! In-memory cache for schedules with persistence.
//!
//! Wraps a `ScheduleStore` trait implementation with in-memory caching
//! and runtime state management.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info};

use super::error::{Result, SchedulerError};
use super::schedule::{Schedule, ScheduleId, ScheduleState, ScheduleStatus};
use crate::store::ScheduleStore as ScheduleStoreTrait;

/// In-memory cache for schedules with persistence.
///
/// Wraps a `ScheduleStore` trait implementation and adds:
/// - In-memory cache for fast lookups
/// - Runtime state management (not persisted)
#[derive(Clone)]
pub struct ScheduleCache {
    inner: Arc<RwLock<ScheduleCacheInner>>,
    /// Underlying persistence store.
    persistence: Arc<dyn ScheduleStoreTrait>,
}

struct ScheduleCacheInner {
    /// Cached schedules by ID.
    schedules: HashMap<ScheduleId, Schedule>,
    /// Runtime state by schedule ID.
    states: HashMap<ScheduleId, ScheduleState>,
}

impl ScheduleCache {
    /// Create a new cache with the given persistence backend.
    pub fn new(persistence: Arc<dyn ScheduleStoreTrait>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ScheduleCacheInner {
                schedules: HashMap::new(),
                states: HashMap::new(),
            })),
            persistence,
        }
    }

    /// Load all schedules from disk.
    ///
    /// Call this on startup to restore persisted schedules.
    pub async fn load(&self) -> Result<LoadResult> {
        let mut loaded = 0;
        let errors = Vec::new();

        let schedules = self
            .persistence
            .list()
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        let mut inner = self.inner.write().await;
        for schedule in schedules {
            let id = schedule.id.clone();
            inner.schedules.insert(id.clone(), schedule);
            inner.states.insert(id, ScheduleState::default());
            loaded += 1;
        }

        if loaded > 0 {
            info!(loaded, "Loaded schedules");
        }

        Ok(LoadResult { loaded, errors })
    }

    /// Create and persist a new schedule.
    pub async fn create(&self, schedule: Schedule) -> Result<()> {
        let id = schedule.id.clone();

        // Persist to disk first
        self.persistence
            .save(&schedule)
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        // Then add to cache
        let mut inner = self.inner.write().await;
        inner.schedules.insert(id.clone(), schedule);
        inner.states.insert(id.clone(), ScheduleState::default());

        debug!(schedule_id = %id, "Created schedule");
        Ok(())
    }

    /// Get a schedule by ID.
    pub async fn get(&self, id: &str) -> Option<Schedule> {
        let inner = self.inner.read().await;
        inner.schedules.get(id).cloned()
    }

    /// Get runtime state for a schedule.
    pub async fn get_state(&self, id: &str) -> Option<ScheduleState> {
        let inner = self.inner.read().await;
        inner.states.get(id).cloned()
    }

    /// Update runtime state for a schedule.
    pub async fn update_state(&self, id: &str, state: ScheduleState) {
        let mut inner = self.inner.write().await;
        inner.states.insert(id.to_string(), state);
    }

    /// Atomically update runtime state for a schedule.
    ///
    /// This prevents read-modify-write races by holding the lock during the entire operation.
    /// The closure receives a mutable reference to the current state (or default if none exists).
    pub async fn update_state_atomically<F>(&self, id: &str, f: F)
    where
        F: FnOnce(&mut ScheduleState),
    {
        let mut inner = self.inner.write().await;
        let state = inner
            .states
            .entry(id.to_string())
            .or_insert_with(ScheduleState::default);
        f(state);
    }

    /// List schedules for an agent.
    pub async fn list_by_agent(&self, agent: &str) -> Vec<Schedule> {
        let inner = self.inner.read().await;
        inner
            .schedules
            .values()
            .filter(|s| s.agent == agent && s.status == ScheduleStatus::Active)
            .cloned()
            .collect()
    }

    /// List all active schedules.
    pub async fn list_active(&self) -> Vec<Schedule> {
        let inner = self.inner.read().await;
        inner
            .schedules
            .values()
            .filter(|s| s.status == ScheduleStatus::Active)
            .cloned()
            .collect()
    }

    /// Update schedule status and persist.
    pub async fn update_status(&self, id: &str, status: ScheduleStatus) -> Result<()> {
        let mut inner = self.inner.write().await;

        let schedule = inner
            .schedules
            .get_mut(id)
            .ok_or_else(|| SchedulerError::NotFound(id.to_string()))?;

        schedule.status = status;
        let schedule_clone = schedule.clone();

        // Release lock before I/O
        drop(inner);

        self.persistence
            .save(&schedule_clone)
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        debug!(schedule_id = %id, status = ?status, "Updated schedule status");
        Ok(())
    }

    /// Delete a schedule from disk and cache.
    pub async fn delete(&self, id: &str) -> Result<()> {
        // Remove from cache
        {
            let mut inner = self.inner.write().await;
            inner.schedules.remove(id);
            inner.states.remove(id);
        }

        // Remove from disk
        self.persistence
            .delete(id)
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        debug!(schedule_id = %id, "Deleted schedule");
        Ok(())
    }
}

/// Result of loading schedules from disk.
#[derive(Debug, Default)]
pub struct LoadResult {
    /// Number of schedules loaded.
    pub loaded: usize,
    /// Errors encountered (path, error message).
    pub errors: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::schedule::{ScheduleDestination, SchedulePayload, ScheduleTiming};
    use crate::store::file::FileScheduleStore;
    use chrono::Utc;
    use tempfile::TempDir;

    fn test_schedule(id: &str, agent: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            agent: agent.to_string(),
            created_by_session: "session_123".to_string(),
            destination: ScheduleDestination {
                gateway: "telegram".to_string(),
                chat_id: "12345".to_string(),
            },
            timing: ScheduleTiming::At {
                at: Utc::now() + chrono::Duration::hours(1),
            },
            payload: SchedulePayload::Message {
                message: "Test reminder".to_string(),
            },
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry: None,
        }
    }

    fn create_cache(temp_dir: &TempDir) -> ScheduleCache {
        let persistence = Arc::new(FileScheduleStore::new(temp_dir.path().join("schedules")));
        ScheduleCache::new(persistence)
    }

    #[tokio::test]
    async fn create_and_get_schedule() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        let schedule = test_schedule("sched_1", "my-agent");
        store.create(schedule.clone()).await.unwrap();

        let retrieved = store.get("sched_1").await.unwrap();
        assert_eq!(retrieved.id, "sched_1");
        assert_eq!(retrieved.agent, "my-agent");
    }

    #[tokio::test]
    async fn get_returns_none_for_missing() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        let result = store.get("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_by_agent_filters_correctly() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        store
            .create(test_schedule("sched_1", "agent-a"))
            .await
            .unwrap();
        store
            .create(test_schedule("sched_2", "agent-a"))
            .await
            .unwrap();
        store
            .create(test_schedule("sched_3", "agent-b"))
            .await
            .unwrap();

        let agent_a = store.list_by_agent("agent-a").await;
        assert_eq!(agent_a.len(), 2);

        let agent_b = store.list_by_agent("agent-b").await;
        assert_eq!(agent_b.len(), 1);
    }

    #[tokio::test]
    async fn update_status_persists() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        store
            .create(test_schedule("sched_1", "my-agent"))
            .await
            .unwrap();
        store
            .update_status("sched_1", ScheduleStatus::Completed)
            .await
            .unwrap();

        // Reload from disk with new store instance
        let new_store = create_cache(&temp_dir);
        new_store.load().await.unwrap();

        let retrieved = new_store.get("sched_1").await.unwrap();
        assert_eq!(retrieved.status, ScheduleStatus::Completed);
    }

    #[tokio::test]
    async fn delete_removes_from_disk_and_cache() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        store
            .create(test_schedule("sched_1", "my-agent"))
            .await
            .unwrap();
        assert!(store.get("sched_1").await.is_some());

        store.delete("sched_1").await.unwrap();
        assert!(store.get("sched_1").await.is_none());

        // Verify file is deleted
        let path = temp_dir.path().join("schedules").join("sched_1.yaml");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn load_recovers_schedules_from_disk() {
        let temp_dir = TempDir::new().unwrap();

        // Create schedules with first store
        {
            let store = create_cache(&temp_dir);
            store
                .create(test_schedule("sched_1", "agent"))
                .await
                .unwrap();
            store
                .create(test_schedule("sched_2", "agent"))
                .await
                .unwrap();
        }

        // Load with new store
        let store = create_cache(&temp_dir);
        let result = store.load().await.unwrap();

        assert_eq!(result.loaded, 2);
        assert!(store.get("sched_1").await.is_some());
        assert!(store.get("sched_2").await.is_some());
    }

    #[tokio::test]
    async fn list_active_excludes_completed() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        store
            .create(test_schedule("sched_1", "agent"))
            .await
            .unwrap();
        store
            .create(test_schedule("sched_2", "agent"))
            .await
            .unwrap();
        store
            .update_status("sched_2", ScheduleStatus::Completed)
            .await
            .unwrap();

        let active = store.list_active().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "sched_1");
    }

    #[tokio::test]
    async fn state_management() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_cache(&temp_dir);

        store
            .create(test_schedule("sched_1", "agent"))
            .await
            .unwrap();

        // Initial state is default
        let state = store.get_state("sched_1").await.unwrap();
        assert!(state.next_run_at.is_none());

        // Update state
        let new_state = ScheduleState {
            next_run_at: Some(Utc::now()),
            ..Default::default()
        };
        store.update_state("sched_1", new_state).await;

        let state = store.get_state("sched_1").await.unwrap();
        assert!(state.next_run_at.is_some());
    }
}
