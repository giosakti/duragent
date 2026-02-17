//! Evaluation methods for session snapshots.
//!
//! Extends `SessionSnapshot` with compatibility checks and replay logic.
//! The data definition lives in `duragent-types`; evaluation lives here.

use crate::session::SessionSnapshot;

/// Extension trait for `SessionSnapshot` evaluation logic.
pub trait SessionSnapshotEval {
    /// Check if this snapshot is compatible with the current schema.
    fn is_compatible(&self) -> bool;

    /// Get the sequence from which to replay events.
    fn replay_from_seq(&self) -> u64;
}

impl SessionSnapshotEval for SessionSnapshot {
    fn is_compatible(&self) -> bool {
        matches!(self.schema_version.as_str(), "1" | "2")
    }

    fn replay_from_seq(&self) -> u64 {
        if self.schema_version == "1" {
            self.last_event_seq
        } else {
            self.checkpoint_seq
        }
    }
}
