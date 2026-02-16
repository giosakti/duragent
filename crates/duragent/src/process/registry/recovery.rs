use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{Semaphore, oneshot};
use tracing::{debug, info, warn};

use crate::process::{
    ProcessBackendKind, ProcessEntry, ProcessMeta, ProcessRegistryHandle, ProcessStatus,
};

use super::monitor;
use super::tmux;
use super::{DEFAULT_CLEANUP_AGE_SECS, RECOVERY_CALLBACK_CONCURRENCY};

/// Spawn a periodic cleanup task. Returns its handle for shutdown.
pub fn spawn_cleanup_task(registry: ProcessRegistryHandle) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5 * 60)); // Every 5 min
        loop {
            interval.tick().await;
            registry.cleanup_old(None).await;
        }
    })
}

impl ProcessRegistryHandle {
    /// Send cancel signal to all active monitors.
    pub fn shutdown(&self) {
        let mut cancelled = 0u32;

        for mut entry in self.entries.iter_mut() {
            if let Some(tx) = entry.cancel_tx.take() {
                let _ = tx.send(());
                cancelled += 1;
            }
        }

        if cancelled > 0 {
            info!(cancelled, "Sent cancel to active process monitors");
        }
    }

    /// Recover processes from disk after restart.
    ///
    /// Scans `.meta.json` files, re-adopts tmux sessions, marks others as lost.
    pub async fn recover(&self) {
        let mut entries = match tokio::fs::read_dir(&self.processes_dir).await {
            Ok(e) => e,
            Err(e) => {
                debug!(error = %e, "No processes dir to recover from");
                return;
            }
        };

        let mut recovered = 0u32;
        let mut lost = 0u32;
        let mut lost_handles = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".meta.json"))
            {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => match serde_json::from_str::<ProcessMeta>(&content) {
                        Ok(mut meta) => {
                            if meta.backend == ProcessBackendKind::Local
                                && meta.tmux_session.is_some()
                            {
                                meta.backend = ProcessBackendKind::Tmux;
                            }

                            if !meta.status.is_terminal() {
                                // Check if tmux session still exists
                                if let Some(ref tmux_name) = meta.tmux_session
                                    && tmux::has_session(tmux_name).await
                                {
                                    // Re-adopt: start a new monitor
                                    let (cancel_tx, cancel_rx) = oneshot::channel();
                                    let entry = ProcessEntry {
                                        meta: meta.clone(),
                                        cancel_tx: Some(cancel_tx),
                                        stdin: None,
                                        watcher_cancel_tx: None,
                                    };
                                    self.entries.insert(meta.handle.clone(), entry);

                                    monitor::spawn_tmux_monitor(
                                        meta.handle.clone(),
                                        tmux_name.clone(),
                                        meta.log_path.clone(),
                                        meta.timeout_seconds,
                                        self.clone(),
                                        cancel_rx,
                                    );

                                    info!(
                                        handle = %meta.handle,
                                        tmux = %tmux_name,
                                        "Re-adopted tmux process"
                                    );
                                    recovered += 1;
                                    continue;
                                }

                                // Non-tmux or tmux session gone → mark as lost
                                meta.status = ProcessStatus::Lost;
                                meta.completed_at = Some(Utc::now());
                                self.persist_meta(&meta).await;
                                let entry = ProcessEntry {
                                    meta: meta.clone(),
                                    cancel_tx: None,
                                    stdin: None,
                                    watcher_cancel_tx: None,
                                };
                                self.entries.insert(meta.handle.clone(), entry);
                                lost += 1;
                                lost_handles.push(meta.handle.clone());
                            } else {
                                // Already terminal — just load into registry for queries
                                let entry = ProcessEntry {
                                    meta: meta.clone(),
                                    cancel_tx: None,
                                    stdin: None,
                                    watcher_cancel_tx: None,
                                };
                                self.entries.insert(meta.handle.clone(), entry);
                            }
                        }
                        Err(e) => {
                            warn!(path = %path.display(), error = %e, "Failed to parse meta.json");
                        }
                    },
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to read meta.json");
                    }
                }
            }
        }

        if !lost_handles.is_empty() {
            let semaphore = Arc::new(Semaphore::new(RECOVERY_CALLBACK_CONCURRENCY));
            for handle_id in lost_handles {
                let semaphore = semaphore.clone();
                let registry = self.clone();
                tokio::spawn(async move {
                    if let Ok(_permit) = semaphore.acquire().await {
                        registry.fire_completion_callback(&handle_id).await;
                    } else {
                        warn!(
                            handle = %handle_id,
                            "Recovery callback semaphore closed; skipping"
                        );
                    }
                });
            }
        }

        if recovered > 0 || lost > 0 {
            info!(recovered, lost, "Process recovery complete");
        }
    }

    /// Remove old completed entries and their files.
    pub async fn cleanup_old(&self, max_age_secs: Option<u64>) {
        let max_age = Duration::from_secs(max_age_secs.unwrap_or(DEFAULT_CLEANUP_AGE_SECS));
        let now = Utc::now();
        let mut to_remove = Vec::new();

        for entry in self.entries.iter() {
            if entry.meta.status.is_terminal()
                && let Some(completed_at) = entry.meta.completed_at
            {
                let age = (now - completed_at).to_std().unwrap_or(Duration::ZERO);
                if age > max_age {
                    to_remove.push(entry.key().clone());
                }
            }
        }

        for handle_id in to_remove {
            if let Some((_, entry)) = self.entries.remove(&handle_id) {
                // Remove log file
                let _ = tokio::fs::remove_file(&entry.meta.log_path).await;
                // Remove meta file
                let meta_path = self
                    .processes_dir
                    .join(format!("{}.meta.json", entry.meta.handle));
                let _ = tokio::fs::remove_file(&meta_path).await;
                debug!(handle = %handle_id, "Cleaned up old process");
            }
        }
    }
}
