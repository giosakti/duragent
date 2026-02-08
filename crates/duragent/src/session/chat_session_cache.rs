//! Cache for mapping (gateway, chat_id, agent) to session_id.
//!
//! This cache enables long-lived sessions for gateway chats, allowing:
//! - Scheduled task results to appear in the same conversation
//! - Users to follow up on task results naturally
//! - Sessions to be an implementation detail, not user-facing

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::store::SessionStore;
use crate::sync::KeyedLocks;

/// Cache mapping (gateway, chat_id, agent) to session_id.
///
/// Thread-safe cache for looking up existing sessions for gateway chats.
/// The agent is included in the key so that routing rule changes create new sessions.
#[derive(Clone)]
pub struct ChatSessionCache {
    cache: Arc<RwLock<HashMap<String, String>>>,
    inflight: KeyedLocks,
}

impl Default for ChatSessionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatSessionCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            inflight: KeyedLocks::new(),
        }
    }

    /// Build the cache key for a (gateway, chat_id, agent) tuple.
    pub fn key(gateway: &str, chat_id: &str, agent: &str) -> String {
        format!("{}\0{}\0{}", gateway, chat_id, agent)
    }

    /// Get the session_id for a (gateway, chat_id, agent) tuple.
    pub async fn get(&self, gateway: &str, chat_id: &str, agent: &str) -> Option<String> {
        let key = Self::key(gateway, chat_id, agent);
        let cache = self.cache.read().await;
        cache.get(&key).cloned()
    }

    /// Insert a session_id for a (gateway, chat_id, agent) tuple.
    pub async fn insert(&self, gateway: &str, chat_id: &str, agent: &str, session_id: &str) {
        let key = Self::key(gateway, chat_id, agent);
        let mut cache = self.cache.write().await;
        cache.insert(key, session_id.to_string());
    }

    /// Remove all cache entries matching a given session_id.
    pub async fn remove_by_session_id(&self, session_id: &str) {
        let mut cache = self.cache.write().await;
        cache.retain(|_key, cached_id| cached_id != session_id);
    }

    /// Atomically get or insert a session_id for a (gateway, chat_id, agent) tuple.
    ///
    /// This prevents race conditions where two concurrent callers could both miss
    /// the cache and create duplicate sessions. The closures are only called if no
    /// valid entry exists for this key.
    ///
    /// # Arguments
    /// * `gateway` - Gateway name
    /// * `chat_id` - Chat identifier
    /// * `agent` - Agent name
    /// * `validator` - Closure that returns true if the cached session_id is still valid
    /// * `creator` - Closure that creates a new session_id if needed
    ///
    /// Returns the existing session_id if found and valid, or the result of calling the creator.
    pub async fn get_or_insert_with<V, VFut, C, CFut, E>(
        &self,
        gateway: &str,
        chat_id: &str,
        agent: &str,
        validator: V,
        creator: C,
    ) -> Result<String, E>
    where
        V: Fn(String) -> VFut,
        VFut: std::future::Future<Output = bool>,
        C: FnOnce() -> CFut,
        CFut: std::future::Future<Output = Result<String, E>>,
    {
        let key = Self::key(gateway, chat_id, agent);
        let lock = self.inflight.get(&key);
        let _guard = lock.lock().await;

        // First, try to get with just a read lock (common case)
        if let Some(session_id) = {
            let cache = self.cache.read().await;
            cache.get(&key).cloned()
        } {
            if validator(session_id.clone()).await {
                return Ok(session_id);
            }

            // Session was deleted, remove stale entry
            let mut cache = self.cache.write().await;
            cache.remove(&key);
        }

        // Create new session and insert
        let session_id = creator().await?;
        let mut cache = self.cache.write().await;
        if let Some(existing) = cache.get(&key) {
            return Ok(existing.clone());
        }
        cache.insert(key, session_id.clone());
        Ok(session_id)
    }

    /// Rebuild the cache from recovered sessions.
    ///
    /// Loads snapshots via the session store and rebuilds the cache from
    /// their stored gateway info.
    pub async fn rebuild_from_sessions(
        &self,
        store: &Arc<dyn SessionStore>,
        session_ids: &[String],
    ) {
        let mut rebuilt = 0;

        for session_id in session_ids {
            // Load snapshot to get gateway info and agent
            let snapshot = match store.load_snapshot(session_id).await {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to load snapshot for cache rebuild"
                    );
                    continue;
                }
            };

            // Check if this session has gateway routing info
            if let (Some(gateway), Some(chat_id)) =
                (&snapshot.config.gateway, &snapshot.config.gateway_chat_id)
            {
                self.insert(gateway, chat_id, &snapshot.agent, session_id)
                    .await;
                rebuilt += 1;

                debug!(
                    gateway = %gateway,
                    chat_id = %chat_id,
                    agent = %snapshot.agent,
                    session_id = %session_id,
                    "Rebuilt chat session cache entry"
                );
            }
        }

        if rebuilt > 0 {
            tracing::info!(
                entries = rebuilt,
                "Rebuilt chat session cache from sessions"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_cache_is_empty() {
        let cache = ChatSessionCache::new();
        assert!(cache.get("telegram", "123", "agent1").await.is_none());
    }

    #[tokio::test]
    async fn insert_and_get() {
        let cache = ChatSessionCache::new();
        cache
            .insert("telegram", "123", "agent1", "session_abc")
            .await;

        assert_eq!(
            cache.get("telegram", "123", "agent1").await,
            Some("session_abc".to_string())
        );
    }

    #[tokio::test]
    async fn different_agents_have_different_sessions() {
        let cache = ChatSessionCache::new();
        cache.insert("telegram", "123", "agent1", "session_1").await;
        cache.insert("telegram", "123", "agent2", "session_2").await;

        assert_eq!(
            cache.get("telegram", "123", "agent1").await,
            Some("session_1".to_string())
        );
        assert_eq!(
            cache.get("telegram", "123", "agent2").await,
            Some("session_2".to_string())
        );
    }

    #[tokio::test]
    async fn different_chats_have_different_sessions() {
        let cache = ChatSessionCache::new();
        cache.insert("telegram", "123", "agent1", "session_1").await;
        cache.insert("telegram", "456", "agent1", "session_2").await;

        assert_eq!(
            cache.get("telegram", "123", "agent1").await,
            Some("session_1".to_string())
        );
        assert_eq!(
            cache.get("telegram", "456", "agent1").await,
            Some("session_2".to_string())
        );
    }

    #[tokio::test]
    async fn different_gateways_have_different_sessions() {
        let cache = ChatSessionCache::new();
        cache.insert("telegram", "123", "agent1", "session_1").await;
        cache.insert("discord", "123", "agent1", "session_2").await;

        assert_eq!(
            cache.get("telegram", "123", "agent1").await,
            Some("session_1".to_string())
        );
        assert_eq!(
            cache.get("discord", "123", "agent1").await,
            Some("session_2".to_string())
        );
    }

    #[tokio::test]
    async fn remove_by_session_id_removes_matching_entries() {
        let cache = ChatSessionCache::new();
        cache.insert("telegram", "123", "agent1", "session_A").await;
        cache.insert("telegram", "456", "agent1", "session_A").await;
        cache.insert("discord", "789", "agent2", "session_B").await;

        cache.remove_by_session_id("session_A").await;

        assert!(cache.get("telegram", "123", "agent1").await.is_none());
        assert!(cache.get("telegram", "456", "agent1").await.is_none());
        assert_eq!(
            cache.get("discord", "789", "agent2").await,
            Some("session_B".to_string())
        );
    }

    #[tokio::test]
    async fn remove_by_session_id_noop_when_not_found() {
        let cache = ChatSessionCache::new();
        cache.insert("telegram", "123", "agent1", "session_A").await;

        cache.remove_by_session_id("nonexistent").await;

        assert_eq!(
            cache.get("telegram", "123", "agent1").await,
            Some("session_A".to_string())
        );
    }

    #[test]
    fn key_format() {
        assert_eq!(
            ChatSessionCache::key("telegram", "123", "agent1"),
            "telegram\0123\0agent1"
        );
    }
}
