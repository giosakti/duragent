//! SSE streaming for session message handling.
//!
//! Provides the `AccumulatingStream` wrapper that handles:
//! - Token accumulation during streaming
//! - Disconnect handling (pause/continue modes)
//! - Background continuation when client disconnects
//! - Automatic persistence of messages and snapshots

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::response::sse::Event;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::agent::OnDisconnect;
use crate::api::{SessionStatus, sse as sse_events};
use crate::background::BackgroundTasks;
use crate::llm::{ChatStream, StreamEvent, Usage};

use super::persist::{
    SessionContext, persist_assistant_message, record_event, write_session_snapshot,
};
use super::{SessionEventPayload, SessionStore};

// ============================================================================
// Public API
// ============================================================================

/// Configuration for an accumulating SSE stream.
pub struct StreamConfig {
    /// Session store for persisting messages.
    pub sessions: SessionStore,
    /// Session ID.
    pub session_id: String,
    /// Agent name.
    pub agent: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Message ID for this response.
    pub message_id: String,
    /// Idle timeout before closing stream.
    pub idle_timeout: Duration,
    /// Token for cancellation on disconnect.
    pub cancel_token: CancellationToken,
    /// Behavior when client disconnects.
    pub on_disconnect: OnDisconnect,
    /// Path to sessions directory for snapshots.
    pub sessions_path: PathBuf,
    /// Registry for background tasks.
    pub background_tasks: BackgroundTasks,
}

/// A stream wrapper that accumulates token content and stores the assistant message when done.
///
/// Features:
/// - Idle timeout via `tokio_stream::StreamExt::timeout()`
/// - Continue mode: spawns background task to complete LLM when client disconnects
/// - Pause mode: cancels LLM request and writes snapshot when client disconnects
/// - Drop safety: handles partial messages based on on_disconnect mode
/// - Emits `start` event before streaming, `done` event with message ID when complete
pub struct AccumulatingStream {
    /// The underlying LLM stream (Option to allow moving to background task in continue mode).
    inner: Option<FlattenedLLMStream>,
    message_id: String,
    accumulated: String,
    last_usage: Option<Usage>,
    sessions: SessionStore,
    session_id: String,
    agent: String,
    sessions_path: PathBuf,
    started: bool,
    finished: bool,
    cancel_token: CancellationToken,
    on_disconnect: OnDisconnect,
    background_tasks: BackgroundTasks,
    disconnect_tx: Option<oneshot::Sender<DisconnectPayload>>,
}

impl AccumulatingStream {
    /// Create a new accumulating stream from an LLM chat stream.
    #[must_use]
    pub fn new(inner: ChatStream, config: StreamConfig) -> Self {
        let StreamConfig {
            sessions,
            session_id,
            agent,
            created_at,
            message_id,
            idle_timeout,
            cancel_token,
            on_disconnect,
            sessions_path,
            background_tasks,
        } = config;

        // Clone the token for the stream wrapper
        let cancel_token_clone = cancel_token.clone();

        // Wrap the inner stream with timeout, cancellation, and flatten the nested Results
        let timed_stream = inner.timeout(idle_timeout);
        let flattened = tokio_stream::StreamExt::map(timed_stream, move |result| {
            // Check for cancellation before processing
            if cancel_token_clone.is_cancelled() {
                return Err(StreamError::Cancelled);
            }
            match result {
                Ok(Ok(event)) => Ok(event),
                Ok(Err(llm_err)) => Err(StreamError::Llm(llm_err)),
                Err(_elapsed) => Err(StreamError::Timeout),
            }
        });

        // Disconnect handler pattern: when this stream is dropped before completion,
        // the Drop impl sends a payload through this channel to trigger background
        // cleanup (persist partial content, update session status). See Drop impl.
        let (disconnect_tx, disconnect_rx) = oneshot::channel();
        let stream_ctx = StreamContext {
            sessions: sessions.clone(),
            session_id: session_id.clone(),
            agent: agent.clone(),
            created_at,
            message_id: message_id.clone(),
            sessions_path: sessions_path.clone(),
            on_disconnect,
        };
        let handler_tasks = background_tasks.clone();
        handler_tasks.spawn(async move {
            if let Ok(payload) = disconnect_rx.await {
                handle_disconnect(stream_ctx, payload).await;
            }
        });

        Self {
            inner: Some(Box::pin(flattened)),
            message_id,
            accumulated: String::new(),
            last_usage: None,
            sessions,
            session_id,
            agent,
            sessions_path,
            started: false,
            finished: false,
            cancel_token,
            on_disconnect,
            background_tasks,
            disconnect_tx: Some(disconnect_tx),
        }
    }

    /// Save accumulated content as assistant message.
    ///
    /// Records the event only (no snapshot) — snapshots are written at session boundaries
    /// (pause, complete, status changes) to avoid O(N²) IO overhead.
    fn save_accumulated(&mut self) {
        if !self.accumulated.is_empty() {
            let sessions = self.sessions.clone();
            let session_id = self.session_id.clone();
            let agent = self.agent.clone();
            let content = std::mem::take(&mut self.accumulated);
            let usage = self.last_usage.take();
            let sessions_path = self.sessions_path.clone();

            debug!(
                session_id = %session_id,
                content_len = content.len(),
                "Saving accumulated content"
            );

            self.background_tasks.spawn(async move {
                if let Err(e) = persist_assistant_message(&sessions, &sessions_path, &session_id, &agent, content, usage).await {
                    warn!(session_id = %session_id, error = %e, "Failed to persist assistant message");
                }
            });
        }
    }
}

impl futures::Stream for AccumulatingStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        if self.finished {
            return Poll::Ready(None);
        }

        // Emit start event on first poll
        if !self.started {
            self.started = true;
            let event = Event::default().event(sse_events::START).data("{}");
            return Poll::Ready(Some(Ok(event)));
        }

        // Get a reference to the inner stream; return None if already taken (continue mode)
        let Some(inner) = self.inner.as_mut() else {
            self.finished = true;
            return Poll::Ready(None);
        };

        match futures::Stream::poll_next(inner.as_mut(), cx) {
            Poll::Ready(Some(Ok(StreamEvent::Token(content)))) => {
                self.accumulated.push_str(&content);
                let event = Event::default()
                    .event(sse_events::TOKEN)
                    .json_data(TokenData { content })
                    .unwrap_or_else(|_| Event::default().event(sse_events::TOKEN).data("{}"));
                Poll::Ready(Some(Ok(event)))
            }

            Poll::Ready(Some(Ok(StreamEvent::Done { usage }))) => {
                self.finished = true;
                self.last_usage = usage.clone();
                self.save_accumulated();
                let event = Event::default()
                    .event(sse_events::DONE)
                    .json_data(DoneData {
                        message_id: self.message_id.clone(),
                        usage,
                    })
                    .unwrap_or_else(|_| Event::default().event(sse_events::DONE).data("{}"));
                Poll::Ready(Some(Ok(event)))
            }

            Poll::Ready(Some(Err(StreamError::Timeout))) => {
                self.finished = true;
                self.save_accumulated();
                let event = Event::default()
                    .event(sse_events::ERROR)
                    .json_data(ErrorData {
                        message: "Stream idle timeout".to_string(),
                    })
                    .unwrap_or_else(|_| Event::default().event(sse_events::ERROR).data("{}"));
                Poll::Ready(Some(Ok(event)))
            }

            Poll::Ready(Some(Err(StreamError::Llm(e)))) => {
                self.finished = true;
                self.save_accumulated();
                let event = Event::default()
                    .event(sse_events::ERROR)
                    .json_data(ErrorData {
                        message: e.to_string(),
                    })
                    .unwrap_or_else(|_| Event::default().event(sse_events::ERROR).data("{}"));
                Poll::Ready(Some(Ok(event)))
            }

            Poll::Ready(Some(Err(StreamError::Cancelled))) => {
                self.finished = true;
                self.save_accumulated();
                let event = Event::default().event(sse_events::CANCELLED).data("{}");
                Poll::Ready(Some(Ok(event)))
            }

            Poll::Ready(Some(Ok(StreamEvent::Cancelled))) => {
                self.finished = true;
                self.save_accumulated();
                let event = Event::default().event(sse_events::CANCELLED).data("{}");
                Poll::Ready(Some(Ok(event)))
            }

            // ToolCalls are not handled in the current SSE stream (requires agentic loop)
            // For now, we skip them - they'll be handled by the agentic loop in send_message
            Poll::Ready(Some(Ok(StreamEvent::ToolCalls(_)))) => {
                // Continue polling for next event
                cx.waker().wake_by_ref();
                Poll::Pending
            }

            Poll::Ready(None) => {
                self.finished = true;
                self.save_accumulated();
                Poll::Ready(None)
            }

            Poll::Pending => Poll::Pending,
        }
    }
}

// ============================================================================
// Implementation Details
// ============================================================================

impl Drop for AccumulatingStream {
    fn drop(&mut self) {
        // If stream wasn't finished normally, handle based on on_disconnect mode
        if !self.finished {
            let payload = match self.on_disconnect {
                OnDisconnect::Continue => {
                    info!(
                        session_id = %self.session_id,
                        message_id = %self.message_id,
                        accumulated_len = self.accumulated.len(),
                        "SSE stream dropped before completion with on_disconnect: continue"
                    );
                    DisconnectPayload {
                        inner: self.inner.take(),
                        accumulated: std::mem::take(&mut self.accumulated),
                    }
                }
                OnDisconnect::Pause => {
                    info!(
                        session_id = %self.session_id,
                        message_id = %self.message_id,
                        accumulated_len = self.accumulated.len(),
                        "SSE stream dropped before completion with on_disconnect: pause"
                    );
                    // Cancel the LLM request
                    self.cancel_token.cancel();
                    DisconnectPayload {
                        inner: None,
                        accumulated: std::mem::take(&mut self.accumulated),
                    }
                }
            };

            if let Some(tx) = self.disconnect_tx.take()
                && tx.send(payload).is_err()
            {
                debug!(
                    session_id = %self.session_id,
                    "Disconnect handler already dropped"
                );
            }
        }
    }
}

// ============================================================================
// Disconnect Handling
// ============================================================================

/// Context for stream lifecycle operations (disconnect handling, background continuation).
struct StreamContext {
    sessions: SessionStore,
    session_id: String,
    agent: String,
    created_at: DateTime<Utc>,
    message_id: String,
    sessions_path: PathBuf,
    on_disconnect: OnDisconnect,
}

impl StreamContext {
    /// Create a SessionContext for snapshot operations.
    fn session_context(&self, status: SessionStatus) -> SessionContext<'_> {
        SessionContext {
            sessions: &self.sessions,
            sessions_path: &self.sessions_path,
            session_id: &self.session_id,
            agent: &self.agent,
            created_at: self.created_at,
            status,
            on_disconnect: self.on_disconnect,
            // SSE streams are created via HTTP API, not gateways
            gateway: None,
            gateway_chat_id: None,
            pending_approval: None,
        }
    }
}

/// Payload sent when the SSE stream is dropped unexpectedly.
struct DisconnectPayload {
    inner: Option<FlattenedLLMStream>,
    accumulated: String,
}

async fn handle_disconnect(ctx: StreamContext, payload: DisconnectPayload) {
    match ctx.on_disconnect {
        OnDisconnect::Continue => {
            let Some(inner) = payload.inner else {
                warn!(
                    session_id = %ctx.session_id,
                    message_id = %ctx.message_id,
                    "Missing stream for background continuation"
                );
                if !payload.accumulated.is_empty()
                    && let Err(e) = persist_assistant_message(
                        &ctx.sessions,
                        &ctx.sessions_path,
                        &ctx.session_id,
                        &ctx.agent,
                        payload.accumulated,
                        None,
                    )
                    .await
                {
                    warn!(session_id = %ctx.session_id, error = %e, "Failed to persist assistant message");
                }

                if let Err(e) =
                    write_session_snapshot(&ctx.session_context(SessionStatus::Active)).await
                {
                    warn!(
                        session_id = %ctx.session_id,
                        error = %e,
                        "Failed to write snapshot after missing stream"
                    );
                }
                return;
            };

            info!(
                session_id = %ctx.session_id,
                message_id = %ctx.message_id,
                accumulated_len = payload.accumulated.len(),
                "Client disconnected with on_disconnect: continue, starting background task"
            );

            continue_stream_in_background(inner, ctx, payload.accumulated).await;
        }
        OnDisconnect::Pause => {
            info!(
                session_id = %ctx.session_id,
                "Client disconnected with on_disconnect: pause, pausing session"
            );

            if !payload.accumulated.is_empty()
                && let Err(e) = persist_assistant_message(
                    &ctx.sessions,
                    &ctx.sessions_path,
                    &ctx.session_id,
                    &ctx.agent,
                    payload.accumulated,
                    None,
                )
                .await
            {
                warn!(session_id = %ctx.session_id, error = %e, "Failed to persist assistant message");
            }

            if ctx
                .sessions
                .set_status(&ctx.session_id, SessionStatus::Paused)
                .await
                .is_err()
            {
                warn!(
                    session_id = %ctx.session_id,
                    "Failed to set session status to paused"
                );
                return;
            }

            if let Err(e) =
                write_session_snapshot(&ctx.session_context(SessionStatus::Paused)).await
            {
                warn!(
                    session_id = %ctx.session_id,
                    error = %e,
                    "Failed to write snapshot on pause"
                );
            } else {
                info!(
                    session_id = %ctx.session_id,
                    "Session paused and snapshot written"
                );
            }
        }
    }
}

// ============================================================================
// Background Continuation
// ============================================================================

/// Continue consuming the LLM stream in the background after client disconnects.
///
/// This function:
/// 1. Continues reading tokens from the LLM stream
/// 2. Logs all events to the session's event log (JSONL)
/// 3. Writes a Running snapshot to indicate the session is still processing
/// 4. Saves the complete message to the session store when done
async fn continue_stream_in_background(
    mut stream: FlattenedLLMStream,
    ctx: StreamContext,
    accumulated: String,
) {
    if ctx
        .sessions
        .set_status(&ctx.session_id, SessionStatus::Running)
        .await
        .is_err()
    {
        warn!(
            session_id = %ctx.session_id,
            "Failed to set session status to running"
        );
    }

    if let Err(e) = write_session_snapshot(&ctx.session_context(SessionStatus::Running)).await {
        warn!(
            session_id = %ctx.session_id,
            error = %e,
            "Failed to write Running snapshot for background continue"
        );
    } else {
        debug!(
            session_id = %ctx.session_id,
            "Wrote Running snapshot for background continue"
        );
    }

    info!(
        session_id = %ctx.session_id,
        message_id = %ctx.message_id,
        accumulated_len = accumulated.len(),
        "Background continue task started"
    );

    let result = consume_stream_to_completion(
        &mut stream,
        &ctx.sessions,
        &ctx.sessions_path,
        &ctx.session_id,
        &ctx.message_id,
        accumulated,
    )
    .await;

    // Save the complete accumulated message
    if !result.accumulated.is_empty()
        && let Err(e) = persist_assistant_message(
            &ctx.sessions,
            &ctx.sessions_path,
            &ctx.session_id,
            &ctx.agent,
            result.accumulated,
            result.usage,
        )
        .await
    {
        warn!(session_id = %ctx.session_id, error = %e, "Failed to persist assistant message");
    }

    // Write final snapshot with Active status (completed processing)
    if ctx
        .sessions
        .set_status(&ctx.session_id, SessionStatus::Active)
        .await
        .is_err()
    {
        warn!(
            session_id = %ctx.session_id,
            "Failed to set session status to active"
        );
    }

    if let Err(e) = write_session_snapshot(&ctx.session_context(SessionStatus::Active)).await {
        warn!(
            session_id = %ctx.session_id,
            error = %e,
            "Failed to write final snapshot after background continue"
        );
    } else {
        debug!(
            session_id = %ctx.session_id,
            "Wrote final Active snapshot after background continue"
        );
    }

    info!(
        session_id = %ctx.session_id,
        message_id = %ctx.message_id,
        "Background continue task completed"
    );
}

/// Result of consuming a stream in background.
struct ConsumeResult {
    /// Accumulated content from the stream.
    accumulated: String,
    /// Token usage if available.
    usage: Option<Usage>,
}

/// Consume a stream in background, accumulating tokens and recording errors.
///
/// Returns the accumulated content and usage when the stream completes.
async fn consume_stream_to_completion(
    stream: &mut FlattenedLLMStream,
    sessions: &SessionStore,
    sessions_path: &Path,
    session_id: &str,
    message_id: &str,
    mut accumulated: String,
) -> ConsumeResult {
    let mut last_usage: Option<Usage> = None;

    while let Some(result) = stream.next().await {
        match result {
            Ok(StreamEvent::Token(content)) => {
                accumulated.push_str(&content);
            }
            Ok(StreamEvent::Done { usage }) => {
                debug!(
                    session_id = %session_id,
                    message_id = %message_id,
                    accumulated_len = accumulated.len(),
                    "Background stream completed"
                );
                last_usage = usage;
                break;
            }
            Ok(StreamEvent::Cancelled) => {
                warn!(
                    session_id = %session_id,
                    message_id = %message_id,
                    "Background stream unexpectedly cancelled"
                );
                break;
            }
            Err(StreamError::Timeout) => {
                warn!(
                    session_id = %session_id,
                    message_id = %message_id,
                    "Background stream timed out"
                );
                if let Err(e) = record_event(
                    sessions,
                    sessions_path,
                    session_id,
                    SessionEventPayload::Error {
                        code: "timeout".to_string(),
                        message: "stream idle timeout in background".to_string(),
                    },
                )
                .await
                {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to write timeout error event"
                    );
                }
                break;
            }
            Err(StreamError::Llm(e)) => {
                warn!(
                    session_id = %session_id,
                    message_id = %message_id,
                    error = %e,
                    "Background stream LLM error"
                );
                if let Err(err) = record_event(
                    sessions,
                    sessions_path,
                    session_id,
                    SessionEventPayload::Error {
                        code: "llm_error".to_string(),
                        message: e.to_string(),
                    },
                )
                .await
                {
                    warn!(
                        session_id = %session_id,
                        error = %err,
                        "Failed to write LLM error event"
                    );
                }
                break;
            }
            Err(StreamError::Cancelled) => {
                warn!(
                    session_id = %session_id,
                    message_id = %message_id,
                    "Background stream cancelled (unexpected)"
                );
                break;
            }
            // ToolCalls are not handled in background stream (requires agentic loop)
            Ok(StreamEvent::ToolCalls(_)) => {
                // Skip tool calls in background stream for now
            }
        }
    }

    ConsumeResult {
        accumulated,
        usage: last_usage,
    }
}

// ============================================================================
// Internal Types
// ============================================================================

/// Unified error type for streaming, flattening nested Results.
enum StreamError {
    Llm(crate::llm::LLMError),
    Timeout,
    Cancelled,
}

/// Inner stream type that flattens `Result<Result<T, LLMError>, Elapsed>` into `Result<T, StreamError>`.
type FlattenedLLMStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent, StreamError>> + Send>>;

#[derive(Serialize)]
struct TokenData {
    content: String,
}

#[derive(Serialize)]
struct DoneData {
    message_id: String,
    usage: Option<Usage>,
}

#[derive(Serialize)]
struct ErrorData {
    message: String,
}
