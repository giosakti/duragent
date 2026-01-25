//! Snapshot loader for session state recovery.
//!
//! Loads session snapshots from `{sessions_dir}/{session_id}/state.yaml`.
//! Returns `Ok(None)` if the snapshot file does not exist.

use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio::fs;

use super::SessionSnapshot;

/// Load a session snapshot from disk.
///
/// Returns `Ok(None)` if the snapshot file does not exist.
/// Returns an error if the file exists but cannot be read or parsed,
/// or if the schema version is incompatible.
///
/// # Arguments
/// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
/// * `session_id` - The session ID
pub async fn load_snapshot(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<Option<SessionSnapshot>> {
    let path = sessions_dir.join(session_id).join("state.yaml");

    let contents = match fs::read_to_string(&path).await {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read snapshot: {}", path.display()));
        }
    };

    let snapshot: SessionSnapshot = serde_saphyr::from_str(&contents)
        .with_context(|| format!("failed to parse snapshot YAML: {}", path.display()))?;

    if !snapshot.is_compatible() {
        bail!(
            "incompatible snapshot schema version: expected {}, got {}",
            SessionSnapshot::SCHEMA_VERSION,
            snapshot.schema_version
        );
    }

    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Message, Role};
    use crate::session::{SessionConfig, SnapshotStatus};
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    fn create_test_snapshot(session_id: &str) -> SessionSnapshot {
        SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            SnapshotStatus::Active,
            Utc::now(),
            42,
            vec![
                Message {
                    role: Role::User,
                    content: "Hello".to_string(),
                },
                Message {
                    role: Role::Assistant,
                    content: "Hi there!".to_string(),
                },
            ],
            SessionConfig::default(),
        )
    }

    async fn write_test_snapshot(temp_dir: &TempDir, session_id: &str, snapshot: &SessionSnapshot) {
        let session_dir = sessions_dir(temp_dir).join(session_id);
        fs::create_dir_all(&session_dir).await.unwrap();

        let yaml = serde_saphyr::to_string(snapshot).unwrap();
        let path = session_dir.join("state.yaml");
        fs::write(&path, yaml).await.unwrap();
    }

    #[tokio::test]
    async fn load_existing_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = create_test_snapshot("session_abc123");

        write_test_snapshot(&temp_dir, "session_abc123", &snapshot).await;

        let loaded = load_snapshot(&sessions_dir(&temp_dir), "session_abc123")
            .await
            .unwrap();

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.session_id, "session_abc123");
        assert_eq!(loaded.agent, "test-agent");
        assert_eq!(loaded.status, SnapshotStatus::Active);
        assert_eq!(loaded.last_event_seq, 42);
        assert_eq!(loaded.conversation.len(), 2);
    }

    #[tokio::test]
    async fn load_nonexistent_snapshot_returns_none() {
        let temp_dir = TempDir::new().unwrap();

        let result = load_snapshot(&sessions_dir(&temp_dir), "no_such_session").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_invalid_yaml_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let session_dir = sessions_dir(&temp_dir).join("bad_session");
        fs::create_dir_all(&session_dir).await.unwrap();

        let path = session_dir.join("state.yaml");
        fs::write(&path, "not: valid: yaml: [[[").await.unwrap();

        let result = load_snapshot(&sessions_dir(&temp_dir), "bad_session").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to parse snapshot YAML"));
    }

    #[tokio::test]
    async fn load_incompatible_schema_version_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let mut snapshot = create_test_snapshot("old_session");
        snapshot.schema_version = "0".to_string();

        write_test_snapshot(&temp_dir, "old_session", &snapshot).await;

        let result = load_snapshot(&sessions_dir(&temp_dir), "old_session").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("incompatible snapshot schema version")
        );
        assert!(err.to_string().contains("expected 1"));
        assert!(err.to_string().contains("got 0"));
    }

    #[tokio::test]
    async fn load_snapshot_preserves_conversation() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot = create_test_snapshot("conv_session");

        write_test_snapshot(&temp_dir, "conv_session", &snapshot).await;

        let loaded = load_snapshot(&sessions_dir(&temp_dir), "conv_session")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.conversation.len(), 2);
        assert_eq!(loaded.conversation[0].role, Role::User);
        assert_eq!(loaded.conversation[0].content, "Hello");
        assert_eq!(loaded.conversation[1].role, Role::Assistant);
        assert_eq!(loaded.conversation[1].content, "Hi there!");
    }
}
