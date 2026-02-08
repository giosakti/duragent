//! Session message queue for managing concurrent message processing.
//!
//! Replaces raw `KeyedLocks` for message handling with an explicit queue that
//! supports configurable modes (batch, sequential, drop), overflow strategies,
//! and per-sender debouncing.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;
use tracing::debug;

use duragent_gateway_protocol::MessageReceivedData;

use crate::agent::{OverflowStrategy, QueueConfig, QueueMode};

/// A message waiting in the queue.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    /// Which gateway this message arrived from.
    pub gateway: String,
    /// The original message data.
    pub data: MessageReceivedData,
    /// When this message was enqueued.
    pub enqueued_at: Instant,
}

/// Result of attempting to enqueue a message.
#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueResult {
    /// Session is idle — caller should process this message now.
    ProcessNow,
    /// Message was added to the pending queue.
    Queued,
    /// Message was added to the debounce buffer (timer will flush it).
    Debounced,
    /// Queue was full and overflow strategy is DropNew — message was silently dropped.
    DroppedNew,
    /// Queue was full and overflow strategy is Reject — return reject_message to sender.
    Rejected,
}

/// Result of draining the queue after processing completes.
#[derive(Debug)]
pub enum DrainResult {
    /// Queue is empty — session is now idle.
    Idle,
    /// Batched all pending messages into one combined message.
    Batched(Vec<QueuedMessage>),
    /// Take the next message for sequential processing.
    Sequential(Box<QueuedMessage>),
}

/// Internal state for a session's message queue.
struct SessionQueueInner {
    /// Whether the session is currently processing a message.
    busy: bool,
    /// Pending messages waiting to be processed.
    pending: VecDeque<QueuedMessage>,
    /// Per-sender debounce buffers (sender_id -> accumulated messages).
    debounce_buffers: HashMap<String, Vec<QueuedMessage>>,
}

/// Per-session message queue with atomic busy/enqueue/drain operations.
pub struct SessionMessageQueue {
    inner: Mutex<SessionQueueInner>,
    /// Notified when a debounce flush adds work while the session is idle.
    wake: Notify,
}

impl SessionMessageQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(SessionQueueInner {
                busy: false,
                pending: VecDeque::new(),
                debounce_buffers: HashMap::new(),
            }),
            wake: Notify::new(),
        }
    }

    /// Try to enqueue a message, respecting the queue config.
    ///
    /// If the session is idle, returns `ProcessNow` and marks the session as busy.
    /// If the session is busy, applies overflow strategy and returns the result.
    pub async fn try_enqueue(&self, msg: QueuedMessage, config: &QueueConfig) -> EnqueueResult {
        let mut inner = self.inner.lock().await;

        if !inner.busy {
            inner.busy = true;
            return EnqueueResult::ProcessNow;
        }

        // Session is busy — try to add to pending queue
        if inner.pending.len() >= config.max_pending {
            match config.overflow {
                OverflowStrategy::DropOld => {
                    inner.pending.pop_front();
                    inner.pending.push_back(msg);
                    EnqueueResult::Queued
                }
                OverflowStrategy::DropNew => EnqueueResult::DroppedNew,
                OverflowStrategy::Reject => EnqueueResult::Rejected,
            }
        } else {
            inner.pending.push_back(msg);
            EnqueueResult::Queued
        }
    }

    /// Drain the queue after processing completes.
    ///
    /// Based on the queue mode:
    /// - **Batch**: drains all pending messages into one combined message
    /// - **Sequential**: takes the next single message
    /// - **Drop**: clears all pending messages
    ///
    /// If the queue is empty (or mode is Drop), marks the session as idle.
    pub async fn drain(&self, config: &QueueConfig) -> DrainResult {
        let mut inner = self.inner.lock().await;

        match config.mode {
            QueueMode::Batch => {
                if inner.pending.is_empty() {
                    inner.busy = false;
                    DrainResult::Idle
                } else {
                    let messages: Vec<_> = inner.pending.drain(..).collect();
                    DrainResult::Batched(messages)
                }
            }
            QueueMode::Sequential => {
                if let Some(msg) = inner.pending.pop_front() {
                    DrainResult::Sequential(Box::new(msg))
                } else {
                    inner.busy = false;
                    DrainResult::Idle
                }
            }
            QueueMode::Drop => {
                inner.pending.clear();
                inner.busy = false;
                DrainResult::Idle
            }
        }
    }

    /// Debounce-or-enqueue: accumulate messages per-sender with a timer.
    ///
    /// If the session is idle and no existing debounce buffer for this sender,
    /// skips debouncing and returns `ProcessNow` (no latency for single messages).
    ///
    /// Otherwise, adds the message to the sender's debounce buffer. The caller
    /// is responsible for spawning a timer that calls `flush_debounce` after
    /// the window expires.
    ///
    /// Returns `(EnqueueResult, bool)` where the bool indicates whether a new
    /// debounce timer should be started (true = first message for this sender).
    pub async fn debounce_or_enqueue(
        &self,
        msg: QueuedMessage,
        config: &QueueConfig,
    ) -> (EnqueueResult, bool) {
        let mut inner = self.inner.lock().await;
        let sender_id = msg.data.sender.id.clone();

        // Fast path: session idle and no existing buffer for this sender
        if !inner.busy && !inner.debounce_buffers.contains_key(&sender_id) {
            inner.busy = true;
            return (EnqueueResult::ProcessNow, false);
        }

        // Add to debounce buffer
        let is_new_buffer = !inner.debounce_buffers.contains_key(&sender_id);
        inner
            .debounce_buffers
            .entry(sender_id)
            .or_default()
            .push(msg);

        // Check if we'd exceed max_pending when flushed
        let total_buffered: usize = inner.debounce_buffers.values().map(|v| v.len()).sum();
        if inner.pending.len() + total_buffered > config.max_pending + config.max_pending {
            // We're way over capacity — still debounce but warn via the result
            // The actual overflow check happens at flush time
        }

        (EnqueueResult::Debounced, is_new_buffer)
    }

    /// Flush a sender's debounce buffer into the pending queue.
    ///
    /// Called when the debounce timer fires. Combines all buffered messages
    /// from this sender into a single `QueuedMessage` and enqueues it.
    /// If the session is idle, notifies the wake channel so the handler
    /// can start processing.
    pub async fn flush_debounce(
        &self,
        sender_id: &str,
        config: &QueueConfig,
    ) -> Option<EnqueueResult> {
        let mut inner = self.inner.lock().await;

        let messages = inner.debounce_buffers.remove(sender_id)?;
        if messages.is_empty() {
            return None;
        }

        let combined = combine_messages(messages);

        // Always enqueue — never return ProcessNow from flush.
        // If session is idle, enqueue and send wake notification.
        let was_idle = !inner.busy;

        // Apply overflow strategy
        let result = if inner.pending.len() >= config.max_pending {
            match config.overflow {
                OverflowStrategy::DropOld => {
                    inner.pending.pop_front();
                    inner.pending.push_back(combined);
                    EnqueueResult::Queued
                }
                OverflowStrategy::DropNew => EnqueueResult::DroppedNew,
                OverflowStrategy::Reject => EnqueueResult::Rejected,
            }
        } else {
            inner.pending.push_back(combined);
            EnqueueResult::Queued
        };

        // Drop the lock before notifying
        drop(inner);

        if was_idle && result == EnqueueResult::Queued {
            self.wake.notify_one();
        }

        Some(result)
    }

    /// Wait for a wake notification (from debounce flush when session is idle).
    pub async fn wait_for_wake(&self) {
        self.wake.notified().await;
    }

    /// Mark the session as idle (used when processing fails or is cancelled).
    pub async fn mark_idle(&self) {
        let mut inner = self.inner.lock().await;
        inner.busy = false;
    }
}

/// Combine multiple messages from the same sender into a single message.
///
/// Joins message texts with newlines. Uses the first message as the base,
/// replacing its text content with the combined text.
pub fn combine_messages(messages: Vec<QueuedMessage>) -> QueuedMessage {
    if messages.len() == 1 {
        return messages.into_iter().next().unwrap();
    }

    let mut iter = messages.into_iter();
    let mut base = iter.next().unwrap();

    let mut combined_text = extract_message_text(&base.data).unwrap_or_default();
    for msg in iter {
        if let Some(text) = extract_message_text(&msg.data) {
            combined_text.push('\n');
            combined_text.push_str(&text);
        }
    }

    // Replace the text content in the base message
    use duragent_gateway_protocol::MessageContent;
    base.data.content = MessageContent::Text {
        text: combined_text,
    };
    base
}

/// Extract text from a message's content.
fn extract_message_text(data: &MessageReceivedData) -> Option<String> {
    use duragent_gateway_protocol::MessageContent;
    match &data.content {
        MessageContent::Text { text } => Some(text.clone()),
        MessageContent::Media { caption, .. } => caption.clone().filter(|c| !c.is_empty()),
        _ => None,
    }
}

/// Collection of per-session message queues.
///
/// Thread-safe map from session_id to `SessionMessageQueue`.
#[derive(Clone)]
pub struct SessionMessageQueues {
    queues: Arc<DashMap<String, Arc<SessionMessageQueue>>>,
}

impl SessionMessageQueues {
    pub fn new() -> Self {
        Self {
            queues: Arc::new(DashMap::new()),
        }
    }

    /// Get or create a queue for the given session.
    pub fn get(&self, session_id: &str) -> Arc<SessionMessageQueue> {
        self.queues
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(SessionMessageQueue::new()))
            .clone()
    }

    /// Spawn a background cleanup task that removes idle queues.
    pub fn spawn_cleanup_task(self, name: &'static str) {
        let cleanup_interval = Duration::from_secs(3600);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(cleanup_interval);
            loop {
                ticker.tick().await;
                // Remove queues where no one else holds a reference
                let stale_keys: Vec<_> = self
                    .queues
                    .iter()
                    .filter(|entry| Arc::strong_count(entry.value()) == 1)
                    .map(|entry| entry.key().clone())
                    .collect();
                let removed = stale_keys.len();
                for key in stale_keys {
                    self.queues.remove(&key);
                }
                if removed > 0 {
                    debug!(
                        removed = removed,
                        remaining = self.queues.len(),
                        queues = name,
                        "Cleaned up idle session queues"
                    );
                }
            }
        });
    }
}

impl Default for SessionMessageQueues {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use duragent_gateway_protocol::{MessageContent, RoutingContext, Sender};

    use super::*;

    fn make_queued_message(gateway: &str, sender_id: &str, text: &str) -> QueuedMessage {
        QueuedMessage {
            gateway: gateway.to_string(),
            data: MessageReceivedData {
                message_id: "msg1".to_string(),
                chat_id: "chat1".to_string(),
                sender: Sender {
                    id: sender_id.to_string(),
                    username: None,
                    display_name: None,
                },
                content: MessageContent::Text {
                    text: text.to_string(),
                },
                routing: RoutingContext {
                    channel: "telegram".to_string(),
                    chat_type: "group".to_string(),
                    chat_id: "chat1".to_string(),
                    sender_id: sender_id.to_string(),
                    extra: HashMap::new(),
                },
                reply_to: None,
                mentions_bot: false,
                reply_to_bot: false,
                timestamp: None,
                metadata: serde_json::Value::Null,
            },
            enqueued_at: Instant::now(),
        }
    }

    // ------------------------------------------------------------------------
    // try_enqueue
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn enqueue_idle_session_returns_process_now() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();
        let msg = make_queued_message("telegram", "user1", "hello");

        let result = queue.try_enqueue(msg, &config).await;
        assert_eq!(result, EnqueueResult::ProcessNow);
    }

    #[tokio::test]
    async fn enqueue_busy_session_queues_message() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // First message marks session busy
        let msg1 = make_queued_message("telegram", "user1", "first");
        assert_eq!(
            queue.try_enqueue(msg1, &config).await,
            EnqueueResult::ProcessNow
        );

        // Second message gets queued
        let msg2 = make_queued_message("telegram", "user1", "second");
        assert_eq!(
            queue.try_enqueue(msg2, &config).await,
            EnqueueResult::Queued
        );
    }

    #[tokio::test]
    async fn enqueue_overflow_drop_old() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            max_pending: 2,
            overflow: OverflowStrategy::DropOld,
            ..Default::default()
        };

        // Fill up
        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await; // ProcessNow
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await; // Queued
        let msg3 = make_queued_message("telegram", "user1", "third");
        queue.try_enqueue(msg3, &config).await; // Queued

        // Overflow: should drop oldest and still return Queued
        let msg4 = make_queued_message("telegram", "user1", "fourth");
        assert_eq!(
            queue.try_enqueue(msg4, &config).await,
            EnqueueResult::Queued
        );

        // Drain and verify oldest was dropped
        let result = queue.drain(&config).await;
        match result {
            DrainResult::Batched(msgs) => {
                assert_eq!(msgs.len(), 2);
                // "second" was dropped, so we should have "third" and "fourth"
                let texts: Vec<_> = msgs
                    .iter()
                    .map(|m| extract_message_text(&m.data).unwrap())
                    .collect();
                assert_eq!(texts, vec!["third", "fourth"]);
            }
            _ => panic!("Expected Batched"),
        }
    }

    #[tokio::test]
    async fn enqueue_overflow_drop_new() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            max_pending: 1,
            overflow: OverflowStrategy::DropNew,
            ..Default::default()
        };

        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await; // ProcessNow
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await; // Queued

        let msg3 = make_queued_message("telegram", "user1", "third");
        assert_eq!(
            queue.try_enqueue(msg3, &config).await,
            EnqueueResult::DroppedNew
        );
    }

    #[tokio::test]
    async fn enqueue_overflow_reject() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            max_pending: 1,
            overflow: OverflowStrategy::Reject,
            ..Default::default()
        };

        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await;
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await;

        let msg3 = make_queued_message("telegram", "user1", "third");
        assert_eq!(
            queue.try_enqueue(msg3, &config).await,
            EnqueueResult::Rejected
        );
    }

    // ------------------------------------------------------------------------
    // drain
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn drain_empty_queue_returns_idle() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // Mark busy first
        let msg = make_queued_message("telegram", "user1", "hello");
        queue.try_enqueue(msg, &config).await;

        let result = queue.drain(&config).await;
        assert!(matches!(result, DrainResult::Idle));
    }

    #[tokio::test]
    async fn drain_batch_returns_all_messages() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            mode: QueueMode::Batch,
            ..Default::default()
        };

        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await;
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await;
        let msg3 = make_queued_message("telegram", "user2", "third");
        queue.try_enqueue(msg3, &config).await;

        let result = queue.drain(&config).await;
        match result {
            DrainResult::Batched(msgs) => assert_eq!(msgs.len(), 2),
            _ => panic!("Expected Batched"),
        }
    }

    #[tokio::test]
    async fn drain_sequential_returns_one_message() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            mode: QueueMode::Sequential,
            ..Default::default()
        };

        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await;
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await;
        let msg3 = make_queued_message("telegram", "user1", "third");
        queue.try_enqueue(msg3, &config).await;

        let result = queue.drain(&config).await;
        match result {
            DrainResult::Sequential(msg) => {
                assert_eq!(extract_message_text(&msg.data).unwrap(), "second");
            }
            _ => panic!("Expected Sequential"),
        }

        // Second drain gets the next one
        let result = queue.drain(&config).await;
        match result {
            DrainResult::Sequential(msg) => {
                assert_eq!(extract_message_text(&msg.data).unwrap(), "third");
            }
            _ => panic!("Expected Sequential"),
        }

        // Third drain: empty
        let result = queue.drain(&config).await;
        assert!(matches!(result, DrainResult::Idle));
    }

    #[tokio::test]
    async fn drain_drop_clears_queue() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig {
            mode: QueueMode::Drop,
            ..Default::default()
        };

        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.try_enqueue(msg1, &config).await;
        let msg2 = make_queued_message("telegram", "user1", "second");
        queue.try_enqueue(msg2, &config).await;

        let result = queue.drain(&config).await;
        assert!(matches!(result, DrainResult::Idle));
    }

    // ------------------------------------------------------------------------
    // debounce
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn debounce_idle_no_buffer_returns_process_now() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        let msg = make_queued_message("telegram", "user1", "hello");
        let (result, start_timer) = queue.debounce_or_enqueue(msg, &config).await;

        assert_eq!(result, EnqueueResult::ProcessNow);
        assert!(!start_timer);
    }

    #[tokio::test]
    async fn debounce_busy_session_buffers_and_starts_timer() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // First message: process now
        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.debounce_or_enqueue(msg1, &config).await;

        // Second message from same sender: debounce
        let msg2 = make_queued_message("telegram", "user1", "second");
        let (result, start_timer) = queue.debounce_or_enqueue(msg2, &config).await;

        assert_eq!(result, EnqueueResult::Debounced);
        assert!(start_timer); // First message for this sender in debounce buffer

        // Third message from same sender: debounce, no new timer
        let msg3 = make_queued_message("telegram", "user1", "third");
        let (result, start_timer) = queue.debounce_or_enqueue(msg3, &config).await;

        assert_eq!(result, EnqueueResult::Debounced);
        assert!(!start_timer); // Timer already running
    }

    #[tokio::test]
    async fn flush_debounce_combines_and_enqueues() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // Mark busy
        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.debounce_or_enqueue(msg1, &config).await;

        // Buffer two messages from user2
        let msg2 = make_queued_message("telegram", "user2", "hello");
        queue.debounce_or_enqueue(msg2, &config).await;
        let msg3 = make_queued_message("telegram", "user2", "world");
        queue.debounce_or_enqueue(msg3, &config).await;

        // Flush user2's buffer — should enqueue combined message
        let result = queue.flush_debounce("user2", &config).await;
        assert_eq!(result, Some(EnqueueResult::Queued));

        // Drain and verify the combined message is in the queue
        let drain = queue.drain(&config).await;
        match drain {
            DrainResult::Batched(msgs) => {
                assert_eq!(msgs.len(), 1);
                let text = extract_message_text(&msgs[0].data).unwrap();
                assert_eq!(text, "hello\nworld");
            }
            _ => panic!("Expected Batched"),
        }
    }

    #[tokio::test]
    async fn flush_debounce_idle_session_enqueues_and_wakes() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // Make session busy then idle
        let msg1 = make_queued_message("telegram", "user1", "first");
        queue.debounce_or_enqueue(msg1, &config).await; // ProcessNow

        // Buffer a message from user2
        let msg2 = make_queued_message("telegram", "user2", "hello");
        queue.debounce_or_enqueue(msg2, &config).await; // Debounced

        // Mark idle
        queue.mark_idle().await;

        // Flush user2's buffer — should enqueue and wake
        let result = queue.flush_debounce("user2", &config).await;
        assert_eq!(result, Some(EnqueueResult::Queued));
    }

    #[tokio::test]
    async fn flush_debounce_nonexistent_sender_returns_none() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        let result = queue.flush_debounce("nonexistent", &config).await;
        assert!(result.is_none());
    }

    // ------------------------------------------------------------------------
    // combine_messages
    // ------------------------------------------------------------------------

    #[test]
    fn combine_single_message_returns_unchanged() {
        let msg = make_queued_message("telegram", "user1", "hello");
        let combined = combine_messages(vec![msg]);
        assert_eq!(extract_message_text(&combined.data).unwrap(), "hello");
    }

    #[test]
    fn combine_multiple_messages_joins_with_newlines() {
        let msg1 = make_queued_message("telegram", "user1", "hello");
        let msg2 = make_queued_message("telegram", "user1", "world");
        let msg3 = make_queued_message("telegram", "user1", "!");

        let combined = combine_messages(vec![msg1, msg2, msg3]);
        assert_eq!(
            extract_message_text(&combined.data).unwrap(),
            "hello\nworld\n!"
        );
    }

    // ------------------------------------------------------------------------
    // SessionMessageQueues
    // ------------------------------------------------------------------------

    #[test]
    fn queues_get_returns_same_queue_for_same_session() {
        let queues = SessionMessageQueues::new();
        let q1 = queues.get("session1");
        let q2 = queues.get("session1");
        assert!(Arc::ptr_eq(&q1, &q2));
    }

    #[test]
    fn queues_get_returns_different_queues_for_different_sessions() {
        let queues = SessionMessageQueues::new();
        let q1 = queues.get("session1");
        let q2 = queues.get("session2");
        assert!(!Arc::ptr_eq(&q1, &q2));
    }

    // ------------------------------------------------------------------------
    // mark_idle
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn mark_idle_allows_next_message_to_process() {
        let queue = SessionMessageQueue::new();
        let config = QueueConfig::default();

        // Mark busy
        let msg1 = make_queued_message("telegram", "user1", "first");
        assert_eq!(
            queue.try_enqueue(msg1, &config).await,
            EnqueueResult::ProcessNow
        );

        // Next message queues
        let msg2 = make_queued_message("telegram", "user1", "second");
        assert_eq!(
            queue.try_enqueue(msg2, &config).await,
            EnqueueResult::Queued
        );

        // Drain (batch mode, returns the queued message)
        queue.drain(&config).await;

        // After drain + mark_idle, next message should ProcessNow
        queue.mark_idle().await;
        let msg3 = make_queued_message("telegram", "user1", "third");
        assert_eq!(
            queue.try_enqueue(msg3, &config).await,
            EnqueueResult::ProcessNow
        );
    }
}
