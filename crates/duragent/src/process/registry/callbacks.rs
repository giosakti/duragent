use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, warn};

use crate::process::{CallbackKey, CallbackKind, CallbackTask, ProcessRegistryHandle};
use crate::session::SteeringMessage;

use super::COMPLETION_LOG_TAIL_BYTES;

const CALLBACK_DEDUPE_WINDOW: Duration = Duration::from_secs(2);
const CALLBACK_DEDUPE_MAX_ENTRIES: usize = 1024;

pub(crate) fn spawn_callback_workers(
    registry: ProcessRegistryHandle,
    receiver: mpsc::Receiver<CallbackTask>,
    worker_count: usize,
) {
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..worker_count {
        let registry = registry.clone();
        let receiver = Arc::clone(&receiver);
        tokio::spawn(async move {
            loop {
                let task = {
                    let mut rx = receiver.lock().await;
                    rx.recv().await
                };

                let Some(task) = task else {
                    break;
                };

                match task {
                    CallbackTask::Completion { handle_id } => {
                        registry.run_completion_callback(&handle_id).await;
                    }
                    CallbackTask::ScreenHalted { handle_id } => {
                        registry.run_screen_halted_callback(&handle_id).await;
                    }
                }
            }
        });
    }
}

impl ProcessRegistryHandle {
    /// Fire a completion callback for a finished process.
    ///
    /// Follows the same pattern as `scheduler/service.rs:execute_task_payload`:
    /// inject user message into session, build executor, run agentic loop,
    /// send response via gateway.
    pub(crate) async fn fire_completion_callback(&self, handle_id: &str) {
        if !self
            .should_enqueue_callback(CallbackKey {
                kind: CallbackKind::Completion,
                handle_id: handle_id.to_string(),
            })
            .await
        {
            debug!(handle = %handle_id, "Dropping duplicate completion callback");
            return;
        }

        if let Err(e) = self
            .callback_tx
            .send(CallbackTask::Completion {
                handle_id: handle_id.to_string(),
            })
            .await
        {
            warn!(
                handle = %handle_id,
                error = %e,
                "Callback queue closed; dropping completion callback"
            );
        }
    }

    /// Fire a screen-halted callback for a process.
    ///
    /// Called by the screen watcher when it detects the screen has stabilized.
    /// Tries to steer the message into a running agentic loop first. If no loop
    /// is running, acquires the lock and runs its own agentic loop.
    pub(crate) async fn fire_screen_halted_callback(&self, handle_id: &str) {
        if !self
            .should_enqueue_callback(CallbackKey {
                kind: CallbackKind::ScreenHalted,
                handle_id: handle_id.to_string(),
            })
            .await
        {
            debug!(handle = %handle_id, "Dropping duplicate screen halted callback");
            return;
        }

        if let Err(e) = self
            .callback_tx
            .send(CallbackTask::ScreenHalted {
                handle_id: handle_id.to_string(),
            })
            .await
        {
            warn!(
                handle = %handle_id,
                error = %e,
                "Callback queue closed; dropping screen halted callback"
            );
        }
    }

    async fn should_enqueue_callback(&self, key: CallbackKey) -> bool {
        let now = Instant::now();
        let mut map = self.callback_dedupe.lock().await;
        if let Some(last) = map.get(&key) {
            if now.duration_since(*last) < CALLBACK_DEDUPE_WINDOW {
                return false;
            }
        }
        map.insert(key, now);
        if map.len() > CALLBACK_DEDUPE_MAX_ENTRIES {
            map.retain(|_, ts| now.duration_since(*ts) < CALLBACK_DEDUPE_WINDOW);
        }
        true
    }

    // ========================================================================
    // Private — callback runners (called by worker pool)
    // ========================================================================

    async fn run_completion_callback(&self, handle_id: &str) {
        let meta = match self.entries.get(handle_id) {
            Some(entry) => entry.meta.clone(),
            None => return,
        };

        // Auto-cancel any schedules linked to this process
        if let Some(ref scheduler) = self.scheduler {
            scheduler
                .cancel_by_process_handle(handle_id, &meta.agent)
                .await;
        }

        // Only fire callback for async processes (not wait:true)
        // Check if we have gateway info to respond to
        let (gateway, chat_id) = match (&meta.gateway, &meta.chat_id) {
            (Some(gw), Some(cid)) => (gw.clone(), cid.clone()),
            _ => {
                debug!(handle = %handle_id, "No gateway/chat_id for callback, skipping");
                return;
            }
        };

        // Read tail of log for the completion message
        let log_tail = match Self::read_log_tail(&meta.log_path, COMPLETION_LOG_TAIL_BYTES).await {
            Ok((content, truncated)) => {
                if truncated {
                    format!("...\n{}", content)
                } else {
                    content
                }
            }
            Err(_) => "(log unavailable)".to_string(),
        };

        let duration = meta
            .completed_at
            .map(|c| (c - meta.spawned_at).num_seconds())
            .unwrap_or(0);

        let label_str = meta
            .label
            .as_deref()
            .map(|l| format!("\nLabel: {}", l))
            .unwrap_or_default();

        let completion_message = format!(
            "[Background Process Completed]\n\n\
             Handle: {}\
             {}\n\
             Command: {}\n\
             Status: {}\n\
             Duration: {}s\n\n\
             Output (last {} bytes):\n{}",
            meta.handle,
            label_str,
            meta.command,
            meta.status,
            duration,
            COMPLETION_LOG_TAIL_BYTES,
            log_tail,
        );

        // Try to steer into a running agentic loop first
        if let Some(tx_ref) = self.services.steering_channels.get(&meta.session_id) {
            let mut persisted = false;
            if let Some(handle) = self.services.session_registry.get(&meta.session_id)
                && handle
                    .add_user_message(completion_message.clone())
                    .await
                    .is_ok()
            {
                persisted = true;
            }
            let steering_msg = SteeringMessage {
                content: completion_message.clone(),
                sender_id: None,
                sender_label: None,
                persisted,
            };
            // Clone sender and drop DashMap guard before awaiting send.
            let tx = tx_ref.clone();
            drop(tx_ref);
            if tx.send(steering_msg).await.is_ok() {
                return;
            }

            // Channel closed — loop just ended. Fall through to run_callback.
            let persist_on_fallback = !persisted;
            if let Err(e) = self
                .run_callback(
                    &meta,
                    &gateway,
                    &chat_id,
                    &completion_message,
                    None,
                    persist_on_fallback,
                )
                .await
            {
                error!(
                    handle = %handle_id,
                    error = %e,
                    "Failed to fire completion callback"
                );
            }
            return;
        }

        if let Err(e) = self
            .run_callback(&meta, &gateway, &chat_id, &completion_message, None, true)
            .await
        {
            error!(
                handle = %handle_id,
                error = %e,
                "Failed to fire completion callback"
            );
        }
    }

    async fn run_screen_halted_callback(&self, handle_id: &str) {
        let meta = match self.entries.get(handle_id) {
            Some(entry) => entry.meta.clone(),
            None => return,
        };

        let (gateway, chat_id) = match (&meta.gateway, &meta.chat_id) {
            (Some(gw), Some(cid)) => (gw.clone(), cid.clone()),
            _ => {
                debug!(handle = %handle_id, "No gateway/chat_id for screen callback, skipping");
                return;
            }
        };

        let message = format!(
            "[Process {} screen halted]\n\n\
             The interactive process screen has stopped changing. \
             Use background_process capture to check if it needs input.",
            handle_id,
        );

        // Try to steer into a running agentic loop first
        if let Some(tx_ref) = self.services.steering_channels.get(&meta.session_id) {
            // Persist the message for durability
            let mut persisted = false;
            if let Some(handle) = self.services.session_registry.get(&meta.session_id)
                && handle.add_user_message(message.clone()).await.is_ok()
            {
                persisted = true;
            }
            let steering_msg = SteeringMessage {
                content: message.clone(),
                sender_id: None,
                sender_label: None,
                persisted,
            };
            // Clone sender and drop DashMap guard before awaiting send.
            let tx = tx_ref.clone();
            drop(tx_ref);
            if tx.send(steering_msg).await.is_ok() {
                return;
            }

            // Channel closed — loop just ended. Fall through to run_callback.
            let persist_on_fallback = !persisted;
            if let Err(e) = self
                .run_callback(
                    &meta,
                    &gateway,
                    &chat_id,
                    &message,
                    Some(self.clone()),
                    persist_on_fallback,
                )
                .await
            {
                error!(
                    handle = %handle_id,
                    error = %e,
                    "Failed to fire screen halted callback"
                );
            }
            return;
        }

        // No running loop — run our own
        if let Err(e) = self
            .run_callback(
                &meta,
                &gateway,
                &chat_id,
                &message,
                Some(self.clone()),
                true,
            )
            .await
        {
            error!(
                handle = %handle_id,
                error = %e,
                "Failed to fire screen halted callback"
            );
        }
    }
}
