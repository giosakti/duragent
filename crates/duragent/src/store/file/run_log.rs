//! File-based run log storage implementation.
//!
//! Stores run history as JSONL files at `{runs_dir}/{schedule_id}.jsonl`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::scheduler::RunLogEntry;
use crate::store::error::{StorageError, StorageResult};
use crate::store::run_log::RunLogStore;
use crate::sync::KeyedLocks;

/// Maximum log file size before pruning (1MB).
const MAX_LOG_SIZE: u64 = 1_024 * 1_024;

/// Number of entries to keep when pruning.
const ENTRIES_TO_KEEP: usize = 1000;

/// File-based implementation of `RunLogStore`.
///
/// Run logs are stored as JSONL files with automatic pruning when they
/// exceed a size threshold.
#[derive(Clone)]
pub struct FileRunLogStore {
    runs_dir: PathBuf,
    /// Per-schedule locks to serialize operations.
    locks: Arc<Mutex<KeyedLocks>>,
}

impl FileRunLogStore {
    /// Create a new file run log store.
    pub fn new(runs_dir: impl Into<PathBuf>) -> Self {
        Self {
            runs_dir: runs_dir.into(),
            locks: Arc::new(Mutex::new(KeyedLocks::new())),
        }
    }

    /// Get the log file path for a schedule.
    fn log_path(&self, schedule_id: &str) -> PathBuf {
        self.runs_dir.join(format!("{}.jsonl", schedule_id))
    }

    /// Ensure the runs directory exists.
    async fn ensure_dir(&self) -> StorageResult<()> {
        fs::create_dir_all(&self.runs_dir)
            .await
            .map_err(|e| StorageError::file_io(&self.runs_dir, e))
    }

    /// Prune a log file to keep only the most recent entries.
    async fn prune(&self, path: &PathBuf) -> StorageResult<()> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| StorageError::file_io(path, e))?;

        let entries: Vec<&str> = content.lines().collect();

        if entries.len() <= ENTRIES_TO_KEEP {
            return Ok(());
        }

        // Keep only the most recent entries
        let start = entries.len().saturating_sub(ENTRIES_TO_KEEP);
        let kept: Vec<&str> = entries[start..].to_vec();
        let new_content = kept.join("\n") + "\n";

        // Write atomically via temp file with fsync
        let temp_path = path.with_extension("jsonl.tmp");
        super::atomic_write_file(&temp_path, path, new_content.as_bytes()).await?;

        tracing::debug!(
            path = %path.display(),
            before = entries.len(),
            after = kept.len(),
            "Pruned run log"
        );

        Ok(())
    }
}

#[async_trait]
impl RunLogStore for FileRunLogStore {
    async fn load_recent(
        &self,
        schedule_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<RunLogEntry>> {
        let locks = self.locks.lock().await;
        let lock = locks.get(schedule_id);
        let _guard = lock.lock().await;
        drop(locks);

        let path = self.log_path(schedule_id);

        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StorageError::file_io(&path, e)),
        };

        let entries: Vec<RunLogEntry> = content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        // Return most recent (last N entries)
        let start = entries.len().saturating_sub(limit);
        Ok(entries[start..].to_vec())
    }

    async fn append(&self, schedule_id: &str, entry: &RunLogEntry) -> StorageResult<()> {
        let locks = self.locks.lock().await;
        let lock = locks.get(schedule_id);
        let _guard = lock.lock().await;
        drop(locks);

        self.ensure_dir().await?;

        let path = self.log_path(schedule_id);

        // Check if pruning is needed
        if let Ok(metadata) = fs::metadata(&path).await
            && metadata.len() > MAX_LOG_SIZE
            && let Err(e) = self.prune(&path).await
        {
            tracing::warn!(path = %path.display(), error = %e, "Failed to prune run log");
        }

        // Append entry as JSONL
        let mut line =
            serde_json::to_string(entry).map_err(|e| StorageError::serialization(e.to_string()))?;
        line.push('\n');

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        file.flush()
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        Ok(())
    }

    async fn delete(&self, schedule_id: &str) -> StorageResult<()> {
        let locks = self.locks.lock().await;
        let lock = locks.get(schedule_id);
        let _guard = lock.lock().await;
        drop(locks);

        let path = self.log_path(schedule_id);

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
    use crate::scheduler::RunStatus;
    use tempfile::TempDir;

    fn test_entry(ts: i64, status: RunStatus) -> RunLogEntry {
        RunLogEntry {
            ts,
            status,
            duration_ms: Some(100),
            error: None,
            next_run_at: None,
        }
    }

    fn create_store(temp_dir: &TempDir) -> FileRunLogStore {
        FileRunLogStore::new(temp_dir.path().join("runs"))
    }

    #[tokio::test]
    async fn append_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store
            .append("sched_1", &test_entry(1000, RunStatus::Ok))
            .await
            .unwrap();
        store
            .append("sched_1", &test_entry(2000, RunStatus::Error))
            .await
            .unwrap();

        let entries = store.load_recent("sched_1", 10).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ts, 1000);
        assert_eq!(entries[1].ts, 2000);
    }

    #[tokio::test]
    async fn load_recent_limits() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        for i in 0..10 {
            store
                .append("sched_1", &test_entry(i * 1000, RunStatus::Ok))
                .await
                .unwrap();
        }

        let entries = store.load_recent("sched_1", 3).await.unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ts, 7000);
        assert_eq!(entries[2].ts, 9000);
    }

    #[tokio::test]
    async fn load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let entries = store.load_recent("nonexistent", 10).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn delete() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store
            .append("sched_1", &test_entry(1000, RunStatus::Ok))
            .await
            .unwrap();

        store.delete("sched_1").await.unwrap();

        let entries = store.load_recent("sched_1", 10).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_ok() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store.delete("nonexistent").await.unwrap();
    }
}
