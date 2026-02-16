//! File-based storage implementations.
//!
//! These implementations store data on the local filesystem using:
//! - YAML for structured documents (snapshots, schedules, agents)
//! - JSONL for append-only logs (events, run logs)
//!
//! All writes use atomic operations (temp file + rename) to prevent corruption.

use std::io::Write;
use std::path::Path;

use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::error::{StorageError, StorageResult};

mod agent;
mod policy;
mod run_log;
mod schedule;
mod session;

pub use agent::FileAgentCatalog;
pub use policy::FilePolicyStore;
pub use run_log::FileRunLogStore;
pub use schedule::FileScheduleStore;
pub use session::FileSessionStore;

/// Write data to a temp file, fsync it, then atomically rename to the final path.
///
/// The temp file name is generated internally using a ULID to avoid collisions
/// from concurrent writers targeting the same final path.
pub(super) async fn atomic_write_file(final_path: &Path, data: &[u8]) -> StorageResult<()> {
    let file_name = final_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let temp_path = final_path.with_file_name(format!("{}.{}.tmp", file_name, ulid::Ulid::new()));

    let mut file = fs::File::create(&temp_path)
        .await
        .map_err(|e| StorageError::file_io(&temp_path, e))?;
    file.write_all(data)
        .await
        .map_err(|e| StorageError::file_io(&temp_path, e))?;
    file.sync_all()
        .await
        .map_err(|e| StorageError::file_io(&temp_path, e))?;
    fs::rename(&temp_path, final_path)
        .await
        .map_err(|e| StorageError::file_io(final_path, e))?;
    Ok(())
}

/// Sync version of [`atomic_write_file`] for use inside `spawn_blocking`.
pub(super) fn atomic_write_file_sync(final_path: &Path, data: &[u8]) -> StorageResult<()> {
    let file_name = final_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let temp_path = final_path.with_file_name(format!("{}.{}.tmp", file_name, ulid::Ulid::new()));

    let mut file =
        std::fs::File::create(&temp_path).map_err(|e| StorageError::file_io(&temp_path, e))?;
    file.write_all(data)
        .map_err(|e| StorageError::file_io(&temp_path, e))?;
    file.sync_all()
        .map_err(|e| StorageError::file_io(&temp_path, e))?;
    std::fs::rename(&temp_path, final_path).map_err(|e| StorageError::file_io(final_path, e))?;
    Ok(())
}
