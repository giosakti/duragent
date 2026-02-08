//! File-based storage implementations.
//!
//! These implementations store data on the local filesystem using:
//! - YAML for structured documents (snapshots, schedules, agents)
//! - JSONL for append-only logs (events, run logs)
//!
//! All writes use atomic operations (temp file + rename) to prevent corruption.

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
pub(super) async fn atomic_write_file(
    temp_path: &Path,
    final_path: &Path,
    data: &[u8],
) -> StorageResult<()> {
    let mut file = fs::File::create(temp_path)
        .await
        .map_err(|e| StorageError::file_io(temp_path, e))?;
    file.write_all(data)
        .await
        .map_err(|e| StorageError::file_io(temp_path, e))?;
    file.sync_all()
        .await
        .map_err(|e| StorageError::file_io(temp_path, e))?;
    fs::rename(temp_path, final_path)
        .await
        .map_err(|e| StorageError::file_io(final_path, e))?;
    Ok(())
}
