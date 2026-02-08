//! File-based session storage implementation.
//!
//! Stores session events as JSONL files and snapshots as YAML files.
//!
//! Directory structure:
//! ```text
//! {sessions_dir}/
//!   {session_id}/
//!     events.jsonl       # Append-only event log
//!     state.yaml         # Atomic snapshot
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::session::{SessionEvent, SessionSnapshot};
use crate::store::error::{StorageError, StorageResult};
use crate::store::session::SessionStore;

/// File-based implementation of `SessionStore`.
///
/// Sessions are stored in subdirectories of `sessions_dir`, with each session
/// having its own `events.jsonl` (append-only event log) and `state.yaml`
/// (atomic snapshot).
#[derive(Debug, Clone)]
pub struct FileSessionStore {
    sessions_dir: PathBuf,
}

impl FileSessionStore {
    /// Create a new file session store.
    ///
    /// The sessions directory will be created when the first session is stored.
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: sessions_dir.into(),
        }
    }

    /// Get the directory path for a session.
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(session_id)
    }

    /// Get the events file path for a session.
    fn events_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events.jsonl")
    }

    /// Get the snapshot file path for a session.
    fn snapshot_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.yaml")
    }

    /// Ensure the session directory exists.
    async fn ensure_session_dir(&self, session_id: &str) -> StorageResult<()> {
        let dir = self.session_dir(session_id);
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| StorageError::file_io(&dir, e))
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    // ========================================================================
    // Index / Lifecycle
    // ========================================================================

    async fn list(&self) -> StorageResult<Vec<String>> {
        let mut sessions = Vec::new();

        let mut entries = match fs::read_dir(&self.sessions_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StorageError::file_io(&self.sessions_dir, e)),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| StorageError::file_io(&self.sessions_dir, e))?
        {
            let path = entry.path();
            if path.is_dir() {
                // Check if it has a state.yaml (valid session)
                if path.join("state.yaml").exists()
                    && let Some(name) = path.file_name()
                {
                    sessions.push(name.to_string_lossy().to_string());
                }
            }
        }

        Ok(sessions)
    }

    async fn delete(&self, session_id: &str) -> StorageResult<()> {
        let dir = self.session_dir(session_id);

        match fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::file_io(&dir, e)),
        }
    }

    // ========================================================================
    // Events (append-only)
    // ========================================================================

    async fn load_events(
        &self,
        session_id: &str,
        after_seq: u64,
    ) -> StorageResult<Vec<SessionEvent>> {
        let path = self.events_path(session_id);

        let file = match File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StorageError::file_io(&path, e)),
        };

        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut events = Vec::new();

        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| StorageError::file_io(&path, e))?
        {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Skip malformed lines (crash recovery)
            let Ok(event) = serde_json::from_str::<SessionEvent>(trimmed) else {
                continue;
            };

            if event.seq > after_seq {
                events.push(event);
            }
        }

        Ok(events)
    }

    async fn append_events(&self, session_id: &str, events: &[SessionEvent]) -> StorageResult<()> {
        if events.is_empty() {
            return Ok(());
        }

        self.ensure_session_dir(session_id).await?;
        let path = self.events_path(session_id);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        // Serialize all events to a single buffer for efficiency
        let mut buffer = String::new();
        for event in events {
            let line = serde_json::to_string(event)
                .map_err(|e| StorageError::serialization(e.to_string()))?;
            buffer.push_str(&line);
            buffer.push('\n');
        }

        file.write_all(buffer.as_bytes())
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        // fsync for durability
        file.sync_all()
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        Ok(())
    }

    // ========================================================================
    // Snapshots
    // ========================================================================

    async fn load_snapshot(&self, session_id: &str) -> StorageResult<Option<SessionSnapshot>> {
        let path = self.snapshot_path(session_id);

        let contents = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StorageError::file_io(&path, e)),
        };

        let snapshot: SessionSnapshot = serde_saphyr::from_str(&contents)
            .map_err(|e| StorageError::file_deserialization(&path, e.to_string()))?;

        if !snapshot.is_compatible() {
            return Err(StorageError::file_incompatible_schema(
                &path,
                SessionSnapshot::SCHEMA_VERSION,
                &snapshot.schema_version,
            ));
        }

        Ok(Some(snapshot))
    }

    async fn save_snapshot(
        &self,
        session_id: &str,
        snapshot: &SessionSnapshot,
    ) -> StorageResult<()> {
        self.ensure_session_dir(session_id).await?;

        let final_path = self.snapshot_path(session_id);
        let temp_path = self.session_dir(session_id).join("state.yaml.tmp");

        let yaml = serde_saphyr::to_string(snapshot)
            .map_err(|e| StorageError::serialization(e.to_string()))?;

        // Write to temp file first
        fs::write(&temp_path, yaml.as_bytes())
            .await
            .map_err(|e| StorageError::file_io(&temp_path, e))?;

        // Atomic rename
        fs::rename(&temp_path, &final_path)
            .await
            .map_err(|e| StorageError::file_io(&final_path, e))?;

        Ok(())
    }

    // ========================================================================
    // Compaction
    // ========================================================================

    async fn compact_events(
        &self,
        session_id: &str,
        up_to_seq: u64,
        archive: bool,
    ) -> StorageResult<()> {
        let path = self.events_path(session_id);

        let contents = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(StorageError::file_io(&path, e)),
        };

        let mut old_lines = Vec::new();
        let mut retained_lines = Vec::new();

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Try to parse seq; keep unparseable lines in retained (safe default)
            match serde_json::from_str::<SessionEvent>(trimmed) {
                Ok(event) if event.seq <= up_to_seq => {
                    old_lines.push(line);
                }
                _ => {
                    retained_lines.push(line);
                }
            }
        }

        if old_lines.is_empty() {
            return Ok(());
        }

        // Archive old events if requested
        if archive {
            let archive_path = self.session_dir(session_id).join("events.archive.jsonl");
            let mut archive_buf = String::new();
            for line in &old_lines {
                archive_buf.push_str(line);
                archive_buf.push('\n');
            }

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&archive_path)
                .await
                .map_err(|e| StorageError::file_io(&archive_path, e))?;
            file.write_all(archive_buf.as_bytes())
                .await
                .map_err(|e| StorageError::file_io(&archive_path, e))?;
            file.sync_all()
                .await
                .map_err(|e| StorageError::file_io(&archive_path, e))?;
        }

        // Write retained lines to temp file, then atomic rename
        let temp_path = self.session_dir(session_id).join("events.jsonl.tmp");
        let mut retained_buf = String::new();
        for line in &retained_lines {
            retained_buf.push_str(line);
            retained_buf.push('\n');
        }

        fs::write(&temp_path, retained_buf.as_bytes())
            .await
            .map_err(|e| StorageError::file_io(&temp_path, e))?;
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| StorageError::file_io(&path, e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::OnDisconnect;
    use crate::api::SessionStatus;
    use crate::llm::{Message, Role};
    use crate::session::{SessionConfig, SessionEventPayload};
    use chrono::Utc;
    use tempfile::TempDir;

    fn create_store(temp_dir: &TempDir) -> FileSessionStore {
        FileSessionStore::new(temp_dir.path().join("sessions"))
    }

    fn create_test_event(seq: u64, content: &str) -> SessionEvent {
        SessionEvent::new(
            seq,
            SessionEventPayload::UserMessage {
                content: content.to_string(),
                sender_id: None,
                sender_name: None,
            },
        )
    }

    fn create_test_snapshot(session_id: &str, last_seq: u64) -> SessionSnapshot {
        SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            last_seq,
            last_seq, // checkpoint_seq matches last_event_seq
            vec![Message::text(Role::User, "Hello")],
            SessionConfig::default(),
        )
    }

    #[tokio::test]
    async fn list_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        // Create a few sessions
        for id in ["session1", "session2", "session3"] {
            let snapshot = create_test_snapshot(id, 1);
            store.save_snapshot(id, &snapshot).await.unwrap();
        }

        let mut sessions = store.list().await.unwrap();
        sessions.sort();
        assert_eq!(sessions, vec!["session1", "session2", "session3"]);
    }

    #[tokio::test]
    async fn list_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let sessions = store.list().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn delete_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        // Create session with events and snapshot
        store
            .append_events("session1", &[create_test_event(1, "test")])
            .await
            .unwrap();
        store
            .save_snapshot("session1", &create_test_snapshot("session1", 1))
            .await
            .unwrap();

        // Verify it exists
        assert!(store.load_snapshot("session1").await.unwrap().is_some());

        // Delete
        store.delete("session1").await.unwrap();

        // Verify it's gone
        assert!(store.load_snapshot("session1").await.unwrap().is_none());
        assert!(store.load_events("session1", 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_ok() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        // Should not error
        store.delete("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn load_events_after_seq() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events: Vec<_> = (1..=5)
            .map(|i| create_test_event(i, &format!("msg{}", i)))
            .collect();
        store.append_events("session1", &events).await.unwrap();

        let loaded = store.load_events("session1", 3).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].seq, 4);
        assert_eq!(loaded[1].seq, 5);
    }

    #[tokio::test]
    async fn load_events_nonexistent_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events = store.load_events("nonexistent", 0).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let snapshot = create_test_snapshot("session1", 42);
        store.save_snapshot("session1", &snapshot).await.unwrap();

        let loaded = store.load_snapshot("session1").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.session_id, "session1");
        assert_eq!(loaded.last_event_seq, 42);
    }

    #[tokio::test]
    async fn append_empty_events_ok() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        store.append_events("session1", &[]).await.unwrap();
        // No file created
        let events = store.load_events("session1", 0).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn append_and_load_events() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events = vec![
            SessionEvent::new(
                1,
                SessionEventPayload::SessionStart {
                    agent: "test-agent".to_string(),
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
                },
            ),
            create_test_event(2, "Hello"),
            create_test_event(3, "World"),
        ];

        store.append_events("session1", &events).await.unwrap();

        // Load all events
        let loaded = store.load_events("session1", 0).await.unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].seq, 1);
        assert_eq!(loaded[1].seq, 2);
        assert_eq!(loaded[2].seq, 3);
    }

    #[tokio::test]
    async fn load_snapshot_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let snapshot = store.load_snapshot("nonexistent").await.unwrap();
        assert!(snapshot.is_none());
    }

    #[tokio::test]
    async fn compact_events_discard_mode() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events: Vec<_> = (1..=5)
            .map(|i| create_test_event(i, &format!("msg{}", i)))
            .collect();
        store.append_events("session1", &events).await.unwrap();

        // Compact events up to seq 3 (discard mode, archive=false)
        store.compact_events("session1", 3, false).await.unwrap();

        // Only events 4 and 5 should remain
        let remaining = store.load_events("session1", 0).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].seq, 4);
        assert_eq!(remaining[1].seq, 5);

        // No archive file should exist
        let archive_path = temp_dir
            .path()
            .join("sessions")
            .join("session1")
            .join("events.archive.jsonl");
        assert!(!archive_path.exists());
    }

    #[tokio::test]
    async fn compact_events_archive_mode() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events: Vec<_> = (1..=5)
            .map(|i| create_test_event(i, &format!("msg{}", i)))
            .collect();
        store.append_events("session1", &events).await.unwrap();

        // Compact events up to seq 3 (archive mode)
        store.compact_events("session1", 3, true).await.unwrap();

        // Only events 4 and 5 should remain in events.jsonl
        let remaining = store.load_events("session1", 0).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].seq, 4);

        // Archive file should contain events 1-3
        let archive_path = temp_dir
            .path()
            .join("sessions")
            .join("session1")
            .join("events.archive.jsonl");
        assert!(archive_path.exists());
        let archive_contents = tokio::fs::read_to_string(&archive_path).await.unwrap();
        let archive_lines: Vec<_> = archive_contents.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(archive_lines.len(), 3);
    }

    #[tokio::test]
    async fn compact_events_nonexistent_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        // Should not error
        store
            .compact_events("nonexistent", 10, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn compact_events_noop_when_nothing_to_compact() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_store(&temp_dir);

        let events: Vec<_> = (5..=8)
            .map(|i| create_test_event(i, &format!("msg{}", i)))
            .collect();
        store.append_events("session1", &events).await.unwrap();

        // Compact up to seq 2 — nothing qualifies
        store.compact_events("session1", 2, false).await.unwrap();

        let remaining = store.load_events("session1", 0).await.unwrap();
        assert_eq!(remaining.len(), 4);
    }
}
