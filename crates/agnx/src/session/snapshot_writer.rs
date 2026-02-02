//! Atomic snapshot writer for session state persistence.
//!
//! Writes session snapshots as YAML to `{sessions_dir}/{session_id}/state.yaml`.
//! Uses atomic write (temp file + rename) to prevent partial writes on crash.

use std::path::Path;

use tokio::fs;

use super::SessionSnapshot;
use super::error::{Result, SessionError};

/// Write a session snapshot atomically.
///
/// Creates the session directory if it doesn't exist.
/// Writes to a temp file first, then atomically renames to the final path.
///
/// # Arguments
/// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
/// * `session_id` - The session ID
/// * `snapshot` - The snapshot to write
pub async fn write_snapshot(
    sessions_dir: &Path,
    session_id: &str,
    snapshot: &SessionSnapshot,
) -> Result<()> {
    let session_dir = sessions_dir.join(session_id);
    fs::create_dir_all(&session_dir)
        .await
        .map_err(|e| SessionError::io(&session_dir, e))?;

    let final_path = session_dir.join("state.yaml");
    let temp_path = session_dir.join("state.yaml.tmp");

    let yaml = serde_saphyr::to_string(snapshot)?;

    fs::write(&temp_path, yaml.as_bytes())
        .await
        .map_err(|e| SessionError::io(&temp_path, e))?;

    fs::rename(&temp_path, &final_path)
        .await
        .map_err(|e| SessionError::io(&final_path, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SessionStatus;
    use crate::llm::{Message, Role};
    use crate::session::SessionConfig;
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    fn state_path(temp_dir: &TempDir, session_id: &str) -> PathBuf {
        temp_dir
            .path()
            .join("sessions")
            .join(session_id)
            .join("state.yaml")
    }

    #[tokio::test]
    async fn creates_session_directory() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = SessionSnapshot::new(
            "test_session".to_string(),
            "test-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            0,
            vec![],
            SessionConfig::default(),
        );

        write_snapshot(&sessions_dir(&temp_dir), "test_session", &snapshot)
            .await
            .unwrap();

        let session_dir = temp_dir.path().join("sessions/test_session");
        assert!(session_dir.exists());
        assert!(session_dir.is_dir());
    }

    #[tokio::test]
    async fn writes_yaml_file() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = SessionSnapshot::new(
            "test_session".to_string(),
            "my-agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            42,
            vec![
                Message::text(Role::User, "Hello"),
                Message::text(Role::Assistant, "Hi there!"),
            ],
            SessionConfig::default(),
        );

        write_snapshot(&sessions_dir(&temp_dir), "test_session", &snapshot)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(state_path(&temp_dir, "test_session")).unwrap();
        assert!(contents.contains("session_id: test_session"));
        assert!(contents.contains("agent: my-agent"));
        assert!(contents.contains("status: active"));
        assert!(contents.contains("last_event_seq: 42"));
    }

    #[tokio::test]
    async fn written_file_is_valid_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = SessionSnapshot::new(
            "roundtrip_test".to_string(),
            "test-agent".to_string(),
            SessionStatus::Paused,
            Utc::now(),
            100,
            vec![Message::text(Role::User, "Test message")],
            SessionConfig::default(),
        );

        write_snapshot(&sessions_dir(&temp_dir), "roundtrip_test", &snapshot)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(state_path(&temp_dir, "roundtrip_test")).unwrap();
        let parsed: SessionSnapshot = serde_saphyr::from_str(&contents).unwrap();

        assert_eq!(parsed.session_id, "roundtrip_test");
        assert_eq!(parsed.agent, "test-agent");
        assert_eq!(parsed.status, SessionStatus::Paused);
        assert_eq!(parsed.last_event_seq, 100);
        assert_eq!(parsed.conversation.len(), 1);
    }

    #[tokio::test]
    async fn overwrites_existing_snapshot() {
        let temp_dir = TempDir::new().unwrap();

        // Write first snapshot
        let snapshot1 = SessionSnapshot::new(
            "test_session".to_string(),
            "agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            10,
            vec![],
            SessionConfig::default(),
        );
        write_snapshot(&sessions_dir(&temp_dir), "test_session", &snapshot1)
            .await
            .unwrap();

        // Write second snapshot (should overwrite)
        let snapshot2 = SessionSnapshot::new(
            "test_session".to_string(),
            "agent".to_string(),
            SessionStatus::Completed,
            Utc::now(),
            50,
            vec![],
            SessionConfig::default(),
        );
        write_snapshot(&sessions_dir(&temp_dir), "test_session", &snapshot2)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(state_path(&temp_dir, "test_session")).unwrap();
        let parsed: SessionSnapshot = serde_saphyr::from_str(&contents).unwrap();

        assert_eq!(parsed.status, SessionStatus::Completed);
        assert_eq!(parsed.last_event_seq, 50);
    }

    #[tokio::test]
    async fn no_temp_file_after_success() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = SessionSnapshot::new(
            "test_session".to_string(),
            "agent".to_string(),
            SessionStatus::Active,
            Utc::now(),
            0,
            vec![],
            SessionConfig::default(),
        );

        write_snapshot(&sessions_dir(&temp_dir), "test_session", &snapshot)
            .await
            .unwrap();

        let temp_path = temp_dir.path().join("sessions/test_session/state.yaml.tmp");
        assert!(!temp_path.exists());
    }
}
