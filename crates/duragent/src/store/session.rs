//! Session storage trait.
//!
//! Defines the interface for persisting session events and snapshots.

use async_trait::async_trait;

use crate::session::{SessionEvent, SessionSnapshot};

use super::error::StorageResult;

/// Storage interface for session persistence.
///
/// Combines append-only event log and point-in-time snapshot storage.
#[async_trait]
pub trait SessionStore: Send + Sync {
    // ========================================================================
    // Index / Lifecycle
    // ========================================================================

    /// List all session IDs.
    ///
    /// Used for recovery on startup.
    async fn list(&self) -> StorageResult<Vec<String>>;

    /// Delete a session and all its data.
    ///
    /// Removes both the event log and snapshot.
    async fn delete(&self, session_id: &str) -> StorageResult<()>;

    // ========================================================================
    // Events (append-only)
    // ========================================================================

    /// Load events from the session's event log.
    ///
    /// Returns events with sequence number greater than `after_seq`.
    /// Used for replaying events after loading a snapshot.
    async fn load_events(
        &self,
        session_id: &str,
        after_seq: u64,
    ) -> StorageResult<Vec<SessionEvent>>;

    /// Append events to the session's event log.
    ///
    /// Events must be persisted durably before returning.
    async fn append_events(&self, session_id: &str, events: &[SessionEvent]) -> StorageResult<()>;

    // ========================================================================
    // Snapshots
    // ========================================================================

    /// Load the most recent snapshot for a session.
    ///
    /// Returns `Ok(None)` if no snapshot exists yet.
    async fn load_snapshot(&self, session_id: &str) -> StorageResult<Option<SessionSnapshot>>;

    /// Save a snapshot for a session.
    ///
    /// Must be atomic - either fully succeeds or has no effect.
    async fn save_snapshot(
        &self,
        session_id: &str,
        snapshot: &SessionSnapshot,
    ) -> StorageResult<()>;

    // ========================================================================
    // Compaction
    // ========================================================================

    /// Compact events up to the given sequence from the event log.
    ///
    /// Events with `seq <= up_to_seq` are removed from `events.jsonl`.
    /// If `archive` is true, old events are appended to `events.archive.jsonl` first.
    /// Safe to call after a successful snapshot that covers these events.
    async fn compact_events(
        &self,
        session_id: &str,
        up_to_seq: u64,
        archive: bool,
    ) -> StorageResult<()>;
}
