//! JSONL event reader for session replay.
//!
//! Reads session events from `{sessions_dir}/{session_id}/events.jsonl`.
//! Handles truncated or malformed lines gracefully by skipping them.

use std::path::{Path, PathBuf};

use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::SessionEvent;
use super::error::{Result, SessionError};

/// Reads session events from a JSONL file.
pub struct EventReader {
    reader: BufReader<File>,
    path: PathBuf,
}

impl EventReader {
    /// Open an event reader for the given session.
    ///
    /// Returns `Ok(None)` if the events file does not exist.
    ///
    /// # Arguments
    /// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
    /// * `session_id` - The session ID
    pub async fn open(sessions_dir: &Path, session_id: &str) -> Result<Option<Self>> {
        let path = sessions_dir.join(session_id).join("events.jsonl");

        let file = match File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SessionError::io(&path, e)),
        };

        let reader = BufReader::new(file);
        Ok(Some(Self { reader, path }))
    }

    /// Read the next event from the log.
    ///
    /// Returns `Ok(None)` at end of file.
    /// Skips lines that cannot be parsed as valid JSON.
    pub async fn next(&mut self) -> Result<Option<SessionEvent>> {
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(|e| SessionError::io(&self.path, e))?;

            if bytes_read == 0 {
                return Ok(None);
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<SessionEvent>(trimmed) {
                Ok(event) => return Ok(Some(event)),
                Err(_) => {
                    // Skip malformed lines (truncated or invalid JSON)
                    continue;
                }
            }
        }
    }

    /// Read all events from the log.
    pub async fn read_all(&mut self) -> Result<Vec<SessionEvent>> {
        let mut events = Vec::new();
        while let Some(event) = self.next().await? {
            events.push(event);
        }
        Ok(events)
    }

    /// Read events starting from a given sequence number.
    ///
    /// Returns all events with `seq >= start_seq`.
    pub async fn read_from_seq(&mut self, start_seq: u64) -> Result<Vec<SessionEvent>> {
        let mut events = Vec::new();
        while let Some(event) = self.next().await? {
            if event.seq >= start_seq {
                events.push(event);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::OnDisconnect;
    use crate::llm::Usage;
    use crate::session::{EventWriter, SessionEventPayload};
    use tempfile::TempDir;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    #[tokio::test]
    async fn open_nonexistent_file_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let result = EventReader::open(&sessions_dir(&temp_dir), "nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_single_event() {
        let temp_dir = TempDir::new().unwrap();
        // Write an event
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();
        let event = SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Hello".to_string(),
            },
        );
        writer.append(&event).await.unwrap();

        // Read it back
        let mut reader = EventReader::open(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap()
            .unwrap();

        let read_event = reader.next().await.unwrap().unwrap();
        assert_eq!(read_event.seq, 1);
        match read_event.payload {
            SessionEventPayload::UserMessage { content } => {
                assert_eq!(content, "Hello");
            }
            _ => panic!("unexpected event type"),
        }

        // Should return None at EOF
        assert!(reader.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_all_events() {
        let temp_dir = TempDir::new().unwrap();
        // Write multiple events
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        let events = vec![
            SessionEvent::new(
                1,
                SessionEventPayload::SessionStart {
                    agent: "my-agent".to_string(),
                    on_disconnect: OnDisconnect::Pause,
                    gateway: None,
                    gateway_chat_id: None,
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
                    agent: "my-agent".to_string(),
                    content: "Hi!".to_string(),
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

        // Read all
        let mut reader = EventReader::open(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap()
            .unwrap();

        let read_events = reader.read_all().await.unwrap();
        assert_eq!(read_events.len(), 3);
        assert_eq!(read_events[0].seq, 1);
        assert_eq!(read_events[1].seq, 2);
        assert_eq!(read_events[2].seq, 3);
    }

    #[tokio::test]
    async fn read_from_seq() {
        let temp_dir = TempDir::new().unwrap();
        // Write multiple events
        let mut writer = EventWriter::new(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap();

        for i in 1..=5 {
            let event = SessionEvent::new(
                i,
                SessionEventPayload::UserMessage {
                    content: format!("Message {}", i),
                },
            );
            writer.append(&event).await.unwrap();
        }

        // Read from seq 3
        let mut reader = EventReader::open(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap()
            .unwrap();

        let events = reader.read_from_seq(3).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 3);
        assert_eq!(events[1].seq, 4);
        assert_eq!(events[2].seq, 5);
    }

    #[tokio::test]
    async fn skips_malformed_json() {
        let temp_dir = TempDir::new().unwrap();
        let session_dir = sessions_dir(&temp_dir).join("test_session");
        tokio::fs::create_dir_all(&session_dir).await.unwrap();

        // Write a file with some malformed lines
        let events_path = session_dir.join("events.jsonl");
        let valid_event = SessionEvent::new(
            1,
            SessionEventPayload::UserMessage {
                content: "Valid".to_string(),
            },
        );
        let valid_json = serde_json::to_string(&valid_event).unwrap();

        let content = format!(
            "{}\n\
             {{\"truncated\n\
             not json at all\n\
             {}\n\
             \n",
            valid_json,
            serde_json::to_string(&SessionEvent::new(
                2,
                SessionEventPayload::UserMessage {
                    content: "Also valid".to_string(),
                },
            ))
            .unwrap()
        );
        tokio::fs::write(&events_path, content).await.unwrap();

        // Read should skip malformed lines
        let mut reader = EventReader::open(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap()
            .unwrap();

        let events = reader.read_all().await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
    }

    #[tokio::test]
    async fn read_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let session_dir = sessions_dir(&temp_dir).join("test_session");
        tokio::fs::create_dir_all(&session_dir).await.unwrap();

        let events_path = session_dir.join("events.jsonl");
        tokio::fs::write(&events_path, "").await.unwrap();

        let mut reader = EventReader::open(&sessions_dir(&temp_dir), "test_session")
            .await
            .unwrap()
            .unwrap();

        let events = reader.read_all().await.unwrap();
        assert!(events.is_empty());
    }
}
