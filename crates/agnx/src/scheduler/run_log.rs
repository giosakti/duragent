//! Per-schedule run logging in JSONL format.
//!
//! Stores run history at `.agnx/schedules/runs/{schedule_id}.jsonl`.
//! Auto-prunes files larger than 1MB, keeping the 1000 most recent entries.

use std::path::{Path, PathBuf};

use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use super::schedule::RunLogEntry;

/// Maximum log file size before pruning (1MB).
const MAX_LOG_SIZE: u64 = 1_024 * 1_024;

/// Number of entries to keep when pruning.
const ENTRIES_TO_KEEP: usize = 1000;

/// Run log writer for a schedule.
#[derive(Clone)]
pub struct RunLog {
    /// Base path for run logs (e.g., `.agnx/schedules/runs`).
    runs_path: PathBuf,
}

impl RunLog {
    /// Create a new run log at the given path.
    pub fn new(runs_path: PathBuf) -> Self {
        Self { runs_path }
    }

    /// Append a run entry to a schedule's log.
    pub async fn append(&self, schedule_id: &str, entry: &RunLogEntry) -> std::io::Result<()> {
        // Ensure directory exists
        fs::create_dir_all(&self.runs_path).await?;

        let path = self.log_path(schedule_id);

        // Check if pruning is needed
        if let Ok(metadata) = fs::metadata(&path).await
            && metadata.len() > MAX_LOG_SIZE
            && let Err(e) = self.prune(&path).await
        {
            warn!(path = %path.display(), error = %e, "Failed to prune run log");
        }

        // Append entry as JSONL
        let mut line =
            serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;
        line.push('\n');

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        debug!(schedule_id = %schedule_id, status = ?entry.status, "Run log entry appended");
        Ok(())
    }

    /// Read the most recent entries from a schedule's log.
    pub async fn read_recent(
        &self,
        schedule_id: &str,
        limit: usize,
    ) -> std::io::Result<Vec<RunLogEntry>> {
        let path = self.log_path(schedule_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let entries: Vec<RunLogEntry> = content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        // Return most recent (last N entries)
        let start = entries.len().saturating_sub(limit);
        Ok(entries[start..].to_vec())
    }

    /// Delete the log file for a schedule.
    pub async fn delete(&self, schedule_id: &str) -> std::io::Result<()> {
        let path = self.log_path(schedule_id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    /// Prune a log file to keep only the most recent entries.
    async fn prune(&self, path: &Path) -> std::io::Result<()> {
        let content = fs::read_to_string(path).await?;
        let entries: Vec<&str> = content.lines().collect();

        if entries.len() <= ENTRIES_TO_KEEP {
            return Ok(());
        }

        // Keep only the most recent entries
        let start = entries.len().saturating_sub(ENTRIES_TO_KEEP);
        let kept: Vec<&str> = entries[start..].to_vec();
        let new_content = kept.join("\n") + "\n";

        // Write atomically via temp file
        let temp_path = path.with_extension("jsonl.tmp");
        fs::write(&temp_path, new_content).await?;
        fs::rename(&temp_path, path).await?;

        debug!(
            path = %path.display(),
            before = entries.len(),
            after = kept.len(),
            "Pruned run log"
        );
        Ok(())
    }

    /// Get the log file path for a schedule.
    fn log_path(&self, schedule_id: &str) -> PathBuf {
        self.runs_path.join(format!("{}.jsonl", schedule_id))
    }

    /// Get the runs directory path.
    pub fn path(&self) -> &Path {
        &self.runs_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::schedule::RunStatus;
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

    #[tokio::test]
    async fn append_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        let entry = test_entry(1000, RunStatus::Ok);
        run_log.append("sched_1", &entry).await.unwrap();

        let path = temp_dir.path().join("runs").join("sched_1.jsonl");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn append_multiple_entries() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        run_log
            .append("sched_1", &test_entry(1000, RunStatus::Ok))
            .await
            .unwrap();
        run_log
            .append("sched_1", &test_entry(2000, RunStatus::Error))
            .await
            .unwrap();
        run_log
            .append("sched_1", &test_entry(3000, RunStatus::Ok))
            .await
            .unwrap();

        let entries = run_log.read_recent("sched_1", 10).await.unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ts, 1000);
        assert_eq!(entries[2].ts, 3000);
    }

    #[tokio::test]
    async fn read_recent_limits_results() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        for i in 0..10 {
            run_log
                .append("sched_1", &test_entry(i * 1000, RunStatus::Ok))
                .await
                .unwrap();
        }

        let entries = run_log.read_recent("sched_1", 3).await.unwrap();
        assert_eq!(entries.len(), 3);
        // Should be the last 3 entries
        assert_eq!(entries[0].ts, 7000);
        assert_eq!(entries[1].ts, 8000);
        assert_eq!(entries[2].ts, 9000);
    }

    #[tokio::test]
    async fn read_recent_returns_empty_for_missing() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        let entries = run_log.read_recent("nonexistent", 10).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        run_log
            .append("sched_1", &test_entry(1000, RunStatus::Ok))
            .await
            .unwrap();

        let path = temp_dir.path().join("runs").join("sched_1.jsonl");
        assert!(path.exists());

        run_log.delete("sched_1").await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn prune_keeps_recent_entries() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));
        let path = temp_dir.path().join("runs").join("sched_1.jsonl");

        // Create directory
        fs::create_dir_all(temp_dir.path().join("runs"))
            .await
            .unwrap();

        // Write more entries than ENTRIES_TO_KEEP
        let mut content = String::new();
        for i in 0..1500 {
            let entry = test_entry(i * 1000, RunStatus::Ok);
            content.push_str(&serde_json::to_string(&entry).unwrap());
            content.push('\n');
        }
        fs::write(&path, &content).await.unwrap();

        run_log.prune(&path).await.unwrap();

        let entries = run_log.read_recent("sched_1", 2000).await.unwrap();
        assert_eq!(entries.len(), ENTRIES_TO_KEEP);
        // Should keep the most recent
        assert_eq!(entries[0].ts, 500 * 1000); // Entry 500
        assert_eq!(entries[999].ts, 1499 * 1000); // Entry 1499
    }

    #[tokio::test]
    async fn error_entry_serializes_correctly() {
        let temp_dir = TempDir::new().unwrap();
        let run_log = RunLog::new(temp_dir.path().join("runs"));

        let entry = RunLogEntry {
            ts: 1000,
            status: RunStatus::Error,
            duration_ms: Some(50),
            error: Some("Gateway unavailable".to_string()),
            next_run_at: None,
        };

        run_log.append("sched_1", &entry).await.unwrap();

        let entries = run_log.read_recent("sched_1", 1).await.unwrap();
        assert_eq!(entries[0].status, RunStatus::Error);
        assert_eq!(entries[0].error.as_deref(), Some("Gateway unavailable"));
    }
}
