//! Session recovery on server startup.
//!
//! Scans the sessions directory for persisted sessions and restores them
//! to the in-memory session store.

use std::path::Path;

use tokio::fs;
use tracing::{debug, info, warn};

use super::error::{Result, SessionError};
use super::{Session, SessionStore, resume_session};
use crate::agent::OnDisconnect;
use crate::api::SessionStatus;

/// Result of session recovery on startup.
#[derive(Debug, Default)]
pub struct RecoveryResult {
    /// Number of sessions successfully recovered.
    pub recovered: usize,
    /// Number of sessions skipped (completed or invalid).
    pub skipped: usize,
    /// Errors encountered during recovery (session_id, error message).
    pub errors: Vec<(String, String)>,
}

/// Recover sessions from disk on server startup.
///
/// Scans the sessions directory for session subdirectories, loads their
/// snapshots, and registers them in the session store.
///
/// - `Active` sessions are registered as Active
/// - `Paused` sessions are registered as Paused
/// - `Running` sessions with `on_disconnect: continue` are registered as Active
///   (they were interrupted mid-stream and should be available for reconnect)
/// - `Completed` sessions are skipped
///
/// # Arguments
/// * `sessions_dir` - Base directory for sessions (e.g., `.agnx/sessions`)
/// * `store` - The session store to register recovered sessions into
pub async fn recover_sessions(sessions_dir: &Path, store: &SessionStore) -> Result<RecoveryResult> {
    let mut result = RecoveryResult::default();

    // Check if sessions directory exists
    if fs::metadata(sessions_dir).await.is_err() {
        debug!(
            sessions_dir = %sessions_dir.display(),
            "Sessions directory does not exist, nothing to recover"
        );
        return Ok(result);
    }

    // Read the sessions directory
    let mut entries = fs::read_dir(sessions_dir)
        .await
        .map_err(|e| SessionError::io(sessions_dir, e))?;

    // Process each entry
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| SessionError::io(sessions_dir, e))?
    {
        let path = entry.path();

        // Skip non-directories
        let file_type = entry
            .file_type()
            .await
            .map_err(|e| SessionError::io(&path, e))?;
        if !file_type.is_dir() {
            continue;
        }

        // Get the session ID from the directory name
        let session_id = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Try to recover this session
        match recover_single_session(sessions_dir, &session_id, store).await {
            Ok(true) => {
                result.recovered += 1;
            }
            Ok(false) => {
                result.skipped += 1;
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to recover session"
                );
                result.errors.push((session_id, e.to_string()));
            }
        }
    }

    if result.recovered > 0 || result.skipped > 0 || !result.errors.is_empty() {
        info!(
            recovered = result.recovered,
            skipped = result.skipped,
            errors = result.errors.len(),
            "Session recovery complete"
        );
    }

    Ok(result)
}

/// Recover a single session from disk.
///
/// Returns `Ok(true)` if the session was recovered, `Ok(false)` if it was skipped
/// (e.g., completed sessions), or an error if recovery failed.
async fn recover_single_session(
    sessions_dir: &Path,
    session_id: &str,
    store: &SessionStore,
) -> Result<bool> {
    // Use the existing resume_session to load and replay events
    let resumed = match resume_session(sessions_dir, session_id).await? {
        Some(r) => r,
        None => {
            debug!(
                session_id = %session_id,
                "No snapshot found, skipping"
            );
            return Ok(false);
        }
    };

    // Determine if we should recover this session
    let status = match resumed.status {
        SessionStatus::Active => SessionStatus::Active,
        SessionStatus::Paused => SessionStatus::Paused,
        SessionStatus::Running => {
            // Running sessions were interrupted; treat based on on_disconnect config
            match resumed.config.on_disconnect {
                OnDisconnect::Continue => {
                    // Session was running in background when server crashed
                    // Register as Active so it can be reconnected
                    debug!(
                        session_id = %session_id,
                        "Recovering interrupted background session as Active"
                    );
                    SessionStatus::Active
                }
                OnDisconnect::Pause => {
                    // This shouldn't normally happen (pause mode doesn't use Running status)
                    // but handle it gracefully
                    debug!(
                        session_id = %session_id,
                        "Recovering Running session with pause mode as Paused"
                    );
                    SessionStatus::Paused
                }
            }
        }
        SessionStatus::Completed => {
            debug!(
                session_id = %session_id,
                "Skipping completed session"
            );
            return Ok(false);
        }
    };

    // Capture values for logging before moving
    let session_id_for_log = resumed.session_id.clone();
    let messages_count = resumed.conversation.len();

    // Build the Session struct from resumed data (messages stored separately)
    let session = Session {
        id: resumed.session_id,
        agent: resumed.agent,
        status,
        created_at: resumed.created_at,
        updated_at: chrono::Utc::now(),
        last_event_seq: resumed.last_event_seq,
    };

    // Register the session with its messages
    store.register(session, resumed.conversation).await;

    info!(
        session_id = %session_id_for_log,
        status = %status,
        messages = messages_count,
        "Recovered session"
    );

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Message, Role};
    use crate::session::{SessionConfig, SessionSnapshot, write_snapshot};
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sessions_dir(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("sessions")
    }

    async fn write_test_snapshot(
        temp_dir: &TempDir,
        session_id: &str,
        status: SessionStatus,
        on_disconnect: OnDisconnect,
    ) {
        let sessions_path = sessions_dir(temp_dir);
        let snapshot = SessionSnapshot::new(
            session_id.to_string(),
            "test-agent".to_string(),
            status,
            Utc::now(),
            1,
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
            SessionConfig { on_disconnect },
        );
        write_snapshot(&sessions_path, session_id, &snapshot)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn recover_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn recover_active_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        write_test_snapshot(
            &temp_dir,
            "session_active",
            SessionStatus::Active,
            OnDisconnect::Pause,
        )
        .await;

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 1);
        assert_eq!(result.skipped, 0);

        let session = store.get("session_active").await.unwrap();
        assert_eq!(session.status, SessionStatus::Active);
        assert_eq!(session.agent, "test-agent");

        let messages = store.get_messages("session_active").await.unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn recover_paused_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        write_test_snapshot(
            &temp_dir,
            "session_paused",
            SessionStatus::Paused,
            OnDisconnect::Pause,
        )
        .await;

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 1);

        let session = store.get("session_paused").await.unwrap();
        assert_eq!(session.status, SessionStatus::Paused);
    }

    #[tokio::test]
    async fn recover_running_session_continue_mode() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        write_test_snapshot(
            &temp_dir,
            "session_running",
            SessionStatus::Running,
            OnDisconnect::Continue,
        )
        .await;

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 1);

        let session = store.get("session_running").await.unwrap();
        assert_eq!(session.status, SessionStatus::Active);
    }

    #[tokio::test]
    async fn skip_completed_session() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        write_test_snapshot(
            &temp_dir,
            "session_completed",
            SessionStatus::Completed,
            OnDisconnect::Pause,
        )
        .await;

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 1);

        let session = store.get("session_completed").await;
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn recover_multiple_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        write_test_snapshot(
            &temp_dir,
            "session_1",
            SessionStatus::Active,
            OnDisconnect::Pause,
        )
        .await;
        write_test_snapshot(
            &temp_dir,
            "session_2",
            SessionStatus::Paused,
            OnDisconnect::Pause,
        )
        .await;
        write_test_snapshot(
            &temp_dir,
            "session_3",
            SessionStatus::Completed,
            OnDisconnect::Pause,
        )
        .await;

        let result = recover_sessions(&sessions_dir(&temp_dir), &store)
            .await
            .unwrap();

        assert_eq!(result.recovered, 2);
        assert_eq!(result.skipped, 1);

        assert!(store.get("session_1").await.is_some());
        assert!(store.get("session_2").await.is_some());
        assert!(store.get("session_3").await.is_none());
    }

    #[tokio::test]
    async fn recover_nonexistent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();
        let nonexistent = temp_dir.path().join("does_not_exist");

        let result = recover_sessions(&nonexistent, &store).await.unwrap();

        assert_eq!(result.recovered, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn recover_handles_invalid_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::new();

        // Create a session directory with an invalid snapshot
        let sessions_path = sessions_dir(&temp_dir);
        let session_dir = sessions_path.join("session_invalid");
        fs::create_dir_all(&session_dir).await.unwrap();
        fs::write(session_dir.join("state.yaml"), "invalid: yaml: [[[")
            .await
            .unwrap();

        // Also create a valid session
        write_test_snapshot(
            &temp_dir,
            "session_valid",
            SessionStatus::Active,
            OnDisconnect::Pause,
        )
        .await;

        let result = recover_sessions(&sessions_path, &store).await.unwrap();

        assert_eq!(result.recovered, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].0, "session_invalid");

        // Valid session should still be recovered
        assert!(store.get("session_valid").await.is_some());
    }
}
