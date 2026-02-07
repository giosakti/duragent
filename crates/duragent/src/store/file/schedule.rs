//! File-based schedule storage implementation.
//!
//! Stores schedules as individual YAML files at `{schedules_dir}/{id}.yaml`.

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;

use crate::scheduler::Schedule;
use crate::store::error::{StorageError, StorageResult};
use crate::store::schedule::ScheduleStore;

/// File-based implementation of `ScheduleStore`.
///
/// Each schedule is stored as a separate YAML file in the schedules directory.
/// Uses atomic writes (temp file + rename) to prevent corruption.
#[derive(Debug, Clone)]
pub struct FileScheduleStore {
    schedules_dir: PathBuf,
}

impl FileScheduleStore {
    /// Create a new file schedule store.
    pub fn new(schedules_dir: impl Into<PathBuf>) -> Self {
        Self {
            schedules_dir: schedules_dir.into(),
        }
    }

    /// Get the file path for a schedule.
    fn schedule_path(&self, id: &str) -> PathBuf {
        self.schedules_dir.join(format!("{}.yaml", id))
    }

    /// Ensure the schedules directory exists.
    async fn ensure_dir(&self) -> StorageResult<()> {
        fs::create_dir_all(&self.schedules_dir)
            .await
            .map_err(|e| StorageError::file_io(&self.schedules_dir, e))
    }
}

#[async_trait]
impl ScheduleStore for FileScheduleStore {
    async fn list(&self) -> StorageResult<Vec<Schedule>> {
        let mut schedules = Vec::new();

        let mut entries = match fs::read_dir(&self.schedules_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StorageError::file_io(&self.schedules_dir, e)),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| StorageError::file_io(&self.schedules_dir, e))?
        {
            let path = entry.path();

            // Skip directories and non-YAML files
            if path.is_dir() {
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "yaml") {
                continue;
            }

            let content = match fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to read schedule");
                    continue;
                }
            };

            match serde_saphyr::from_str::<Schedule>(&content) {
                Ok(schedule) => schedules.push(schedule),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to parse schedule");
                    continue;
                }
            }
        }

        Ok(schedules)
    }

    async fn load(&self, id: &str) -> StorageResult<Option<Schedule>> {
        let path = self.schedule_path(id);

        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StorageError::file_io(&path, e)),
        };

        let schedule: Schedule = serde_saphyr::from_str(&content)
            .map_err(|e| StorageError::file_deserialization(&path, e.to_string()))?;

        Ok(Some(schedule))
    }

    async fn save(&self, schedule: &Schedule) -> StorageResult<()> {
        self.ensure_dir().await?;

        let path = self.schedule_path(&schedule.id);
        let temp_path = path.with_extension("yaml.tmp");

        let content = serde_saphyr::to_string(schedule)
            .map_err(|e| StorageError::serialization(e.to_string()))?;

        // Write to temp file first
        fs::write(&temp_path, content)
            .await
            .map_err(|e| StorageError::file_io(&temp_path, e))?;

        // Atomic rename
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> StorageResult<()> {
        let path = self.schedule_path(id);

        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::file_io(&path, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{ScheduleDestination, SchedulePayload, ScheduleStatus, ScheduleTiming};
    use chrono::Utc;
    use tempfile::TempDir;

    fn test_schedule(id: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            agent: "test-agent".to_string(),
            created_by_session: "session_123".to_string(),
            destination: ScheduleDestination {
                gateway: "telegram".to_string(),
                chat_id: "12345".to_string(),
            },
            timing: ScheduleTiming::At {
                at: Utc::now() + chrono::Duration::hours(1),
            },
            payload: SchedulePayload::Message {
                message: "Test".to_string(),
            },
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry: None,
        }
    }

    fn create_store(temp_dir: &TempDir) -> FileScheduleStore {
        FileScheduleStore::new(temp_dir.path().join("schedules"))
    }

    #[tokio::test]
    async fn list() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store.save(&test_schedule("sched_1")).await.unwrap();
        store.save(&test_schedule("sched_2")).await.unwrap();
        store.save(&test_schedule("sched_3")).await.unwrap();

        let schedules = store.list().await.unwrap();
        assert_eq!(schedules.len(), 3);
    }

    #[tokio::test]
    async fn list_empty() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let schedules = store.list().await.unwrap();
        assert!(schedules.is_empty());
    }

    #[tokio::test]
    async fn load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let loaded = store.load("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let schedule = test_schedule("sched_1");
        store.save(&schedule).await.unwrap();

        let loaded = store.load("sched_1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, "sched_1");
    }

    #[tokio::test]
    async fn delete() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store.save(&test_schedule("sched_1")).await.unwrap();
        assert!(store.load("sched_1").await.unwrap().is_some());

        store.delete("sched_1").await.unwrap();
        assert!(store.load("sched_1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_ok() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store.delete("nonexistent").await.unwrap();
    }
}
