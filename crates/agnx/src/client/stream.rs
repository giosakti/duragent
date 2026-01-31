//! SSE stream consumer for client.
//!
//! Parses Server-Sent Events from the `/stream` endpoint into typed events.
//! Reuses the shared SSE parser from `crate::sse`.

use futures::StreamExt;
use reqwest::Response;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::sse as sse_events;
use crate::llm::Usage;
use crate::sse::SseEventStream;

use super::error::{ClientError, Result};

/// Channel buffer size for SSE event stream.
///
/// 32 is sufficient for typical LLM token rates (10-50 tokens/sec).
/// If the buffer fills, backpressure slows the network read (correct behavior).
const SSE_CHANNEL_BUFFER: usize = 32;

/// Events received from the SSE stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientStreamEvent {
    /// Stream has started.
    Start,
    /// A token of content.
    Token { content: String },
    /// Stream completed successfully.
    Done {
        message_id: String,
        usage: Option<Usage>,
    },
    /// Stream was cancelled.
    Cancelled,
    /// An error occurred.
    Error { message: String },
    /// Tool execution requires approval.
    ApprovalRequired { call_id: String, command: String },
}

/// Create a stream of `ClientStreamEvent` from an SSE response.
///
/// Spawns a background task to read and parse SSE events, sending them through a channel.
pub fn into_event_stream(
    response: Response,
) -> impl futures::Stream<Item = Result<ClientStreamEvent>> {
    let (tx, rx) = mpsc::channel(SSE_CHANNEL_BUFFER);

    tokio::spawn(async move {
        let result = process_sse_stream(response, tx.clone()).await;
        if let Err(e) = result {
            let _ = tx.send(Err(e)).await;
        }
    });

    ReceiverStream::new(rx)
}

/// Process the SSE stream and send events through the channel.
async fn process_sse_stream(
    response: Response,
    tx: mpsc::Sender<Result<ClientStreamEvent>>,
) -> Result<()> {
    let byte_stream = response.bytes_stream();
    let mut event_stream = SseEventStream::new(byte_stream);
    let mut seen_done = false;

    while let Some(result) = event_stream.next().await {
        let sse_event = result.map_err(ClientError::Http)?;

        // Get event type, defaulting to empty string if not specified
        let event_type = sse_event.event.as_deref().unwrap_or("");

        let client_event = parse_event(event_type, &sse_event.data)?;

        // Track if we've seen a terminal event
        if matches!(
            client_event,
            ClientStreamEvent::Done { .. }
                | ClientStreamEvent::Cancelled
                | ClientStreamEvent::Error { .. }
        ) {
            seen_done = true;
        }

        if tx.send(Ok(client_event)).await.is_err() {
            // Receiver dropped, stop processing
            return Ok(());
        }
    }

    // Stream ended - if we never saw a terminal event, it's an unexpected end
    if !seen_done {
        return Err(ClientError::SseStreamEnded);
    }

    Ok(())
}

/// Parse a complete SSE event from event type and data.
fn parse_event(event_type: &str, data: &str) -> Result<ClientStreamEvent> {
    match event_type {
        sse_events::START => Ok(ClientStreamEvent::Start),
        sse_events::TOKEN => {
            let parsed: TokenData = serde_json::from_str(data)
                .map_err(|e| ClientError::SseParseError(e.to_string()))?;
            Ok(ClientStreamEvent::Token {
                content: parsed.content,
            })
        }
        sse_events::DONE => {
            let parsed: DoneData = serde_json::from_str(data)
                .map_err(|e| ClientError::SseParseError(e.to_string()))?;
            Ok(ClientStreamEvent::Done {
                message_id: parsed.message_id,
                usage: parsed.usage,
            })
        }
        sse_events::CANCELLED => Ok(ClientStreamEvent::Cancelled),
        sse_events::ERROR => {
            let parsed: ErrorData = serde_json::from_str(data)
                .map_err(|e| ClientError::SseParseError(e.to_string()))?;
            Ok(ClientStreamEvent::Error {
                message: parsed.message,
            })
        }
        sse_events::APPROVAL_REQUIRED => {
            let parsed: ApprovalRequiredData = serde_json::from_str(data)
                .map_err(|e| ClientError::SseParseError(e.to_string()))?;
            Ok(ClientStreamEvent::ApprovalRequired {
                call_id: parsed.call_id,
                command: parsed.command,
            })
        }
        _ => Err(ClientError::SseParseError(format!(
            "Unknown event type: {}",
            event_type
        ))),
    }
}

// SSE JSON payloads

#[derive(Deserialize)]
struct TokenData {
    content: String,
}

#[derive(Deserialize)]
struct DoneData {
    message_id: String,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ErrorData {
    message: String,
}

#[derive(Deserialize)]
struct ApprovalRequiredData {
    call_id: String,
    command: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_start() {
        let event = parse_event(sse_events::START, "{}").unwrap();
        assert_eq!(event, ClientStreamEvent::Start);
    }

    #[test]
    fn parse_event_token() {
        let event = parse_event(sse_events::TOKEN, r#"{"content":"Hello"}"#).unwrap();
        assert_eq!(
            event,
            ClientStreamEvent::Token {
                content: "Hello".to_string()
            }
        );
    }

    #[test]
    fn parse_event_done() {
        let event = parse_event(
            sse_events::DONE,
            r#"{"message_id":"msg_123","usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#,
        )
        .unwrap();

        match event {
            ClientStreamEvent::Done { message_id, usage } => {
                assert_eq!(message_id, "msg_123");
                let u = usage.unwrap();
                assert_eq!(u.prompt_tokens, 10);
                assert_eq!(u.completion_tokens, 20);
                assert_eq!(u.total_tokens, 30);
            }
            _ => panic!("Expected Done event"),
        }
    }

    #[test]
    fn parse_event_cancelled() {
        let event = parse_event(sse_events::CANCELLED, "{}").unwrap();
        assert_eq!(event, ClientStreamEvent::Cancelled);
    }

    #[test]
    fn parse_event_error() {
        let event =
            parse_event(sse_events::ERROR, r#"{"message":"Something went wrong"}"#).unwrap();
        assert_eq!(
            event,
            ClientStreamEvent::Error {
                message: "Something went wrong".to_string()
            }
        );
    }
}
