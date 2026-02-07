//! Synchronization primitives for Duragent.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::debug;

/// Default interval between cleanup runs (1 hour).
pub const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

/// Default max idle age before a lock is considered stale (2 hours).
pub const DEFAULT_MAX_IDLE_AGE: Duration = Duration::from_secs(7200);

/// Internal storage type for keyed locks: maps key to (lock, last_access_time).
type LockStorage = DashMap<String, (Arc<Mutex<()>>, Instant)>;

/// Per-key async mutex with automatic stale entry cleanup.
///
/// Provides fine-grained locking where different keys can be accessed concurrently
/// while operations on the same key are serialized. Tracks last-access time for
/// each key to enable periodic cleanup of stale entries.
///
/// # Example
///
/// ```ignore
/// let locks = KeyedLocks::new();
///
/// // Different keys can lock concurrently
/// let lock_a = locks.get("key_a");
/// let lock_b = locks.get("key_b");
///
/// // Same key serializes
/// let _guard = lock_a.lock().await;
/// // Another call to locks.get("key_a").lock().await would wait
/// ```
#[derive(Clone)]
pub struct KeyedLocks {
    locks: Arc<LockStorage>,
}

impl KeyedLocks {
    /// Create a new empty lock collection.
    pub fn new() -> Self {
        Self {
            locks: Arc::new(DashMap::new()),
        }
    }

    /// Create a new lock collection with automatic cleanup.
    ///
    /// Spawns a background task that cleans up stale entries using default intervals
    /// (1 hour cleanup, 2 hour max age).
    pub fn with_cleanup(name: &'static str) -> Self {
        let locks = Self::new();
        locks.clone().spawn_cleanup_task(name);
        locks
    }

    /// Get or create a lock for the given key.
    ///
    /// Updates the last-access timestamp on each call for cleanup tracking.
    pub fn get(&self, key: &str) -> Arc<Mutex<()>> {
        let now = Instant::now();
        self.locks
            .entry(key.to_string())
            .and_modify(|(_, last_access)| *last_access = now)
            .or_insert_with(|| (Arc::new(Mutex::new(())), now))
            .0
            .clone()
    }

    /// Remove stale lock entries that haven't been accessed recently.
    ///
    /// Only removes entries where:
    /// 1. The lock hasn't been accessed within `max_age`
    /// 2. No one else holds a reference to the lock (strong_count == 1)
    ///
    /// Returns the number of entries removed.
    pub fn cleanup_stale(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        let stale_keys: Vec<_> = self
            .locks
            .iter()
            .filter(|entry| {
                let (lock, last_access) = entry.value();
                // Only remove if no one is waiting (strong_count == 1 means only DashMap holds it)
                // and it hasn't been accessed recently
                Arc::strong_count(lock) == 1 && now.duration_since(*last_access) > max_age
            })
            .map(|entry| entry.key().clone())
            .collect();

        let count = stale_keys.len();
        for key in stale_keys {
            self.locks.remove(&key);
        }
        count
    }

    /// Spawn a background task that periodically cleans up stale entries.
    ///
    /// Uses default intervals (1 hour cleanup, 2 hour max age).
    /// The task runs indefinitely until the runtime shuts down.
    pub fn spawn_cleanup_task(self, name: &'static str) {
        self.spawn_cleanup_task_with(DEFAULT_CLEANUP_INTERVAL, DEFAULT_MAX_IDLE_AGE, name);
    }

    /// Spawn cleanup task with custom intervals.
    pub fn spawn_cleanup_task_with(
        self,
        interval: Duration,
        max_age: Duration,
        name: &'static str,
    ) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let removed = self.cleanup_stale(max_age);
                if removed > 0 {
                    debug!(
                        removed = removed,
                        remaining = self.len(),
                        locks = name,
                        "Cleaned up stale locks"
                    );
                }
            }
        });
    }

    /// Return the number of lock entries currently held.
    pub fn len(&self) -> usize {
        self.locks.len()
    }

    /// Return true if there are no lock entries.
    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }
}

impl Default for KeyedLocks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_same_lock_for_same_key() {
        let locks = KeyedLocks::new();

        let lock1 = locks.get("key1");
        let lock2 = locks.get("key1");

        assert!(Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn get_returns_different_locks_for_different_keys() {
        let locks = KeyedLocks::new();

        let lock1 = locks.get("key1");
        let lock2 = locks.get("key2");

        assert!(!Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn cleanup_removes_stale_entries() {
        let locks = KeyedLocks::new();

        // Insert with old timestamp by manipulating directly
        let old_time = Instant::now() - Duration::from_secs(10);
        locks
            .locks
            .insert("stale".to_string(), (Arc::new(Mutex::new(())), old_time));

        // Insert fresh entry
        locks.get("fresh");

        assert_eq!(locks.len(), 2);

        // Cleanup with 5 second max age
        let removed = locks.cleanup_stale(Duration::from_secs(5));

        assert_eq!(removed, 1);
        assert_eq!(locks.len(), 1);
        assert!(locks.locks.contains_key("fresh"));
        assert!(!locks.locks.contains_key("stale"));
    }

    #[test]
    fn cleanup_preserves_locks_with_active_references() {
        let locks = KeyedLocks::new();

        // Insert with old timestamp
        let old_time = Instant::now() - Duration::from_secs(10);
        let lock = Arc::new(Mutex::new(()));
        locks
            .locks
            .insert("held".to_string(), (Arc::clone(&lock), old_time));

        // Keep a reference (simulates someone holding the lock)
        let _held = Arc::clone(&lock);

        // Cleanup should NOT remove because strong_count > 1
        let removed = locks.cleanup_stale(Duration::from_secs(5));

        assert_eq!(removed, 0);
        assert_eq!(locks.len(), 1);
    }

    #[test]
    fn cleanup_on_empty_is_safe() {
        let locks = KeyedLocks::new();
        let removed = locks.cleanup_stale(Duration::from_secs(5));
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn locks_serialize_same_key_access() {
        let locks = KeyedLocks::new();
        let lock = locks.get("key1");

        let guard = lock.try_lock();
        assert!(guard.is_ok());

        // Same lock should fail to acquire immediately
        let lock2 = locks.get("key1");
        let guard2 = lock2.try_lock();
        assert!(guard2.is_err());
    }

    #[tokio::test]
    async fn different_keys_can_lock_concurrently() {
        let locks = KeyedLocks::new();

        let lock1 = locks.get("key1");
        let lock2 = locks.get("key2");

        let _guard1 = lock1.try_lock().unwrap();
        let guard2 = lock2.try_lock();

        // Different keys should be able to lock concurrently
        assert!(guard2.is_ok());
    }
}
