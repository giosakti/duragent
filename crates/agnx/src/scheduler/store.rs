//! Schedule persistence to YAML files.
//!
//! Stores schedules in `.agnx/schedules/{id}.yaml` with an in-memory cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::error::{Result, SchedulerError};
use super::schedule::{Schedule, ScheduleId, ScheduleState, ScheduleStatus};

/// Store for schedule persistence.
///
/// Maintains an in-memory cache backed by YAML files on disk.
#[derive(Clone)]
pub struct ScheduleStore {
    inner: Arc<RwLock<ScheduleStoreInner>>,
    /// Base path for schedule storage (e.g., `.agnx/schedules`).
    schedules_path: PathBuf,
}

struct ScheduleStoreInner {
    /// Cached schedules by ID.
    schedules: HashMap<ScheduleId, Schedule>,
    /// Runtime state by schedule ID.
    states: HashMap<ScheduleId, ScheduleState>,
}

impl ScheduleStore {
    /// Create a new store at the given path.
    pub fn new(schedules_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ScheduleStoreInner {
                schedules: HashMap::new(),
                states: HashMap::new(),
            })),
            schedules_path,
        }
    }

    /// Load all schedules from disk.
    ///
    /// Call this on startup to restore persisted schedules.
    pub async fn load(&self) -> Result<LoadResult> {
        // Ensure directory exists
        if !self.schedules_path.exists() {
            fs::create_dir_all(&self.schedules_path)
                .await
                .map_err(|e| SchedulerError::Storage(e.to_string()))?;
            return Ok(LoadResult::default());
        }

        let mut loaded = 0;
        let mut errors = Vec::new();

        let mut entries = fs::read_dir(&self.schedules_path)
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?
        {
            let path = entry.path();

            // Skip non-YAML files and the runs directory
            if path.is_dir() || path.extension().is_none_or(|ext| ext != "yaml") {
                continue;
            }

            match self.load_schedule_file(&path).await {
                Ok(schedule) => {
                    let id = schedule.id.clone();
                    let mut inner = self.inner.write().await;
                    inner.schedules.insert(id.clone(), schedule);
                    inner.states.insert(id, ScheduleState::default());
                    loaded += 1;
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to load schedule");
                    errors.push((path.display().to_string(), e.to_string()));
                }
            }
        }

        if loaded > 0 {
            info!(loaded = loaded, errors = errors.len(), "Loaded schedules");
        }

        Ok(LoadResult { loaded, errors })
    }

    /// Load a single schedule file.
    async fn load_schedule_file(&self, path: &Path) -> Result<Schedule> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| SchedulerError::Storage(format!("read {}: {}", path.display(), e)))?;

        let schedule: Schedule = serde_saphyr::from_str(&content)
            .map_err(|e| SchedulerError::Storage(format!("parse {}: {}", path.display(), e)))?;

        Ok(schedule)
    }

    /// Create and persist a new schedule.
    pub async fn create(&self, schedule: Schedule) -> Result<()> {
        let id = schedule.id.clone();

        // Persist to disk first
        self.persist(&schedule).await?;

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

        self.persist(&schedule_clone).await?;

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
        let path = self.schedule_path(id);
        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| {
                SchedulerError::Storage(format!("delete {}: {}", path.display(), e))
            })?;
        }

        debug!(schedule_id = %id, "Deleted schedule");
        Ok(())
    }

    /// Persist a schedule to disk.
    async fn persist(&self, schedule: &Schedule) -> Result<()> {
        // Ensure directory exists
        fs::create_dir_all(&self.schedules_path)
            .await
            .map_err(|e| SchedulerError::Storage(e.to_string()))?;

        let path = self.schedule_path(&schedule.id);
        let content = serde_saphyr::to_string(schedule)
            .map_err(|e| SchedulerError::Storage(format!("serialize: {}", e)))?;

        // Write atomically via temp file
        let temp_path = path.with_extension("yaml.tmp");
        fs::write(&temp_path, content).await.map_err(|e| {
            SchedulerError::Storage(format!("write {}: {}", temp_path.display(), e))
        })?;
        fs::rename(&temp_path, &path).await.map_err(|e| {
            SchedulerError::Storage(format!("rename {}: {}", temp_path.display(), e))
        })?;

        Ok(())
    }

    /// Get the file path for a schedule.
    fn schedule_path(&self, id: &str) -> PathBuf {
        self.schedules_path.join(format!("{}.yaml", id))
    }

    /// Get the path to the schedules directory.
    pub fn path(&self) -> &Path {
        &self.schedules_path
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

    #[tokio::test]
    async fn create_and_get_schedule() {
        let temp_dir = TempDir::new().unwrap();
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

        let schedule = test_schedule("sched_1", "my-agent");
        store.create(schedule.clone()).await.unwrap();

        let retrieved = store.get("sched_1").await.unwrap();
        assert_eq!(retrieved.id, "sched_1");
        assert_eq!(retrieved.agent, "my-agent");
    }

    #[tokio::test]
    async fn get_returns_none_for_missing() {
        let temp_dir = TempDir::new().unwrap();
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

        let result = store.get("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_by_agent_filters_correctly() {
        let temp_dir = TempDir::new().unwrap();
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

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
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

        store
            .create(test_schedule("sched_1", "my-agent"))
            .await
            .unwrap();
        store
            .update_status("sched_1", ScheduleStatus::Completed)
            .await
            .unwrap();

        // Reload from disk
        let new_store = ScheduleStore::new(temp_dir.path().join("schedules"));
        new_store.load().await.unwrap();

        let retrieved = new_store.get("sched_1").await.unwrap();
        assert_eq!(retrieved.status, ScheduleStatus::Completed);
    }

    #[tokio::test]
    async fn delete_removes_from_disk_and_cache() {
        let temp_dir = TempDir::new().unwrap();
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

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
            let store = ScheduleStore::new(temp_dir.path().join("schedules"));
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
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));
        let result = store.load().await.unwrap();

        assert_eq!(result.loaded, 2);
        assert!(store.get("sched_1").await.is_some());
        assert!(store.get("sched_2").await.is_some());
    }

    #[tokio::test]
    async fn list_active_excludes_completed() {
        let temp_dir = TempDir::new().unwrap();
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

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
        let store = ScheduleStore::new(temp_dir.path().join("schedules"));

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
