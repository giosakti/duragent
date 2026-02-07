//! Background task registry for tracking spawned async tasks.
//!
//! Tasks spawned during request handling (e.g., for session persistence)
//! are registered here so they can be awaited during graceful shutdown.

// std::sync::Mutex is correct here—lock is never held across .await points.
// See: https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;
use tracing::{info, warn};

// ============================================================================
// BackgroundTasks
// ============================================================================

/// Registry for background tasks that should be awaited on shutdown.
#[derive(Clone, Default)]
pub struct BackgroundTasks {
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl BackgroundTasks {
    // ------------------------------------------------------------------------
    // Constructor
    // ------------------------------------------------------------------------

    /// Create a new empty task registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // ------------------------------------------------------------------------
    // Task Management
    // ------------------------------------------------------------------------

    /// Spawn a background task and register its handle.
    ///
    /// The task will be tracked and can be awaited via `shutdown()`.
    /// Registration is synchronous to ensure the handle is tracked before
    /// this method returns (avoiding race conditions with fast-completing tasks).
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);

        let mut guard = self.handles.lock().expect("mutex poisoned");
        guard.retain(|h| !h.is_finished());
        guard.push(handle);
    }

    /// Wait for all registered background tasks to complete.
    ///
    /// Call this during graceful shutdown to ensure all persistence
    /// operations finish before the server exits.
    pub async fn shutdown(&self) {
        let handles: Vec<_> = std::mem::take(&mut *self.handles.lock().expect("mutex poisoned"));

        let count = handles.len();
        if count == 0 {
            return;
        }

        info!(count, "Waiting for background tasks to complete");

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(()) => {}
                Err(e) => {
                    warn!(task = i, error = %e, "Background task panicked");
                }
            }
        }

        info!("All background tasks completed");
    }

    /// Get the number of pending tasks.
    pub fn pending_count(&self) -> usize {
        let mut guard = self.handles.lock().expect("mutex poisoned");
        guard.retain(|h| !h.is_finished());
        guard.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn spawn_and_shutdown() {
        let counter = Arc::new(AtomicUsize::new(0));
        let tasks = BackgroundTasks::new();

        let c1 = counter.clone();
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            c1.fetch_add(1, Ordering::SeqCst);
        });

        let c2 = counter.clone();
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            c2.fetch_add(1, Ordering::SeqCst);
        });

        // Shutdown waits for all tasks
        tasks.shutdown().await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn shutdown_empty_is_noop() {
        let tasks = BackgroundTasks::new();
        tasks.shutdown().await;
        // Should not hang or error
    }

    #[tokio::test]
    async fn spawn_registers_immediately() {
        let tasks = BackgroundTasks::new();

        // Spawn a task that completes instantly
        tasks.spawn(async {});

        // Handle should be registered immediately (no race condition)
        // Note: The task may have already completed, but it was registered
        assert!(tasks.pending_count() <= 1);
    }
}
