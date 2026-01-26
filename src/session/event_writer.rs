//! JSONL event writer for session persistence.
//!
//! Writes session events as JSON lines to `{sessions_dir}/{session_id}/events.jsonl`.
//! Each write is followed by fsync for crash-safe durability.

use std::path::{Path, PathBuf};

use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;

use super::SessionEvent;
use super::error::{Result, SessionError};

/// Writes session events to a JSONL file.
pub struct EventWriter {
    file: File,
    path: PathBuf,
}

impl EventWriter {
    /// Create a new event writer for the given session.
    ///
    /// Creates the session directory if it doesn't exist.
    /// Opens the events file in append mode.
    ///
    /// # Arguments
    /// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
    /// * `session_id` - The session ID
    pub async fn new(sessions_dir: &Path, session_id: &str) -> Result<Self> {
        let session_dir = sessions_dir.join(session_id);
        fs::create_dir_all(&session_dir)
            .await
            .map_err(|e| SessionError::io(&session_dir, e))?;

        let path = session_dir.join("events.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| SessionError::io(&path, e))?;

        Ok(Self { file, path })
    }

    /// Append an event to the log file.
    ///
    /// Serializes the event as JSON, writes it as a single line, and calls fsync.
    pub async fn append(&mut self, event: &SessionEvent) -> Result<()> {
        let mut line = serde_json::to_string(event)?;
        line.push('\n');

        self.file
            .write_all(line.as_bytes())
            .await
            .map_err(|e| SessionError::io(&self.path, e))?;

        self.file
            .sync_all()
            .await
            .map_err(|e| SessionError::io(&self.path, e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Usage;
    use crate::session::SessionEventPayload;
    use tempfile::TempDir;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    fn events_path(temp_dir: &TempDir, session_id: &str) -> PathBuf {
        temp_dir
            .path()
            .join("sessions")
            .join(session_id)
            .join("events.jsonl")
    }

    #[tokio::test]
    async fn creates_session_directory() {
        let temp_dir = TempDir::new().unwrap();
        let _writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        let session_dir = temp_dir.path().join("sessions/test_session");
        assert!(session_dir.exists());
        assert!(session_dir.is_dir());
    }

    #[tokio::test]
    async fn creates_events_file() {
        let temp_dir = TempDir::new().unwrap();
        let _writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        let events_file = events_path(&temp_dir, "test_session");
        assert!(events_file.exists());
    }

    #[tokio::test]
    async fn append_writes_json_line() {
        let temp_dir = TempDir::new().unwrap();
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        let event = SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Hello, world!".to_string(),
            },
        );
        writer.append(&event).await.unwrap();

        let contents = std::fs::read_to_string(events_path(&temp_dir, "test_session")).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);

        let parsed: SessionEvent = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.seq, 1);
        match parsed.payload {
            SessionEventPayload::UserMessage { content } => {
                assert_eq!(content, "Hello, world!");
            }
            _ => panic!("unexpected event type"),
        }
    }

    #[tokio::test]
    async fn append_multiple_events() {
        let temp_dir = TempDir::new().unwrap();
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        let events = vec![
            SessionEvent::new(
                1,
                SessionEventPayload::SessionStart {
                    session_id: "test_session".to_string(),
                    agent: "my-agent".to_string(),
                },
            ),
            SessionEvent::new(
                2,
                SessionEventPayload::UserMessage {
                    content: "Hello".to_string(),
                },
            ),
            SessionEvent::new(
                3,
                SessionEventPayload::AssistantMessage {
                    content: "Hi there!".to_string(),
                    usage: Some(Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    }),
                },
            ),
        ];

        for event in &events {
            writer.append(event).await.unwrap();
        }

        let contents = std::fs::read_to_string(events_path(&temp_dir, "test_session")).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3);

        for (i, line) in lines.iter().enumerate() {
            let parsed: SessionEvent = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.seq, (i + 1) as u64);
        }
    }

    #[tokio::test]
    async fn append_to_existing_file() {
        let temp_dir = TempDir::new().unwrap();

        // Write first event
        {
            let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
                .await
                .unwrap();
            let event = SessionEvent::new(
                1,
                SessionEventPayload::UserMessage {
                    content: "First".to_string(),
                },
            );
            writer.append(&event).await.unwrap();
        }

        // Create new writer and append second event
        {
            let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
                .await
                .unwrap();
            let event = SessionEvent::new(
                2,
                SessionEventPayload::UserMessage {
                    content: "Second".to_string(),
                },
            );
            writer.append(&event).await.unwrap();
        }

        let contents = std::fs::read_to_string(events_path(&temp_dir, "test_session")).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: SessionEvent = serde_json::from_str(lines[0]).unwrap();
        let second: SessionEvent = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
    }
}
