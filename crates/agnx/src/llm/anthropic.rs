//! Anthropic LLM provider with native API format.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;

use super::error::LLMError;
use super::provider::LLMProvider;
use super::types::{
    ChatRequest, ChatResponse, ChatStream, Choice, Message, Role, StreamEvent, Usage,
};
use crate::sse::SseEventStream;

/// Anthropic provider with native API format.
pub struct AnthropicProvider {
    client: Client,
    base_url: String,
    api_key: String,
    api_version: String,
}

impl AnthropicProvider {
    pub const DEFAULT_API_VERSION: &'static str = "2023-06-01";

    #[must_use]
    pub fn new(client: Client, api_key: String, base_url: String) -> Self {
        Self {
            client,
            base_url,
            api_key,
            api_version: Self::DEFAULT_API_VERSION.to_string(),
        }
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LLMError> {
        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_request = to_request(&request, None);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .json(&anthropic_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        let anthropic_response: Response = response.json().await?;
        Ok(from_response(anthropic_response))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LLMError> {
        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_request = to_request(&request, Some(true));

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.api_version)
            .json(&anthropic_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = SseEventStream::new(byte_stream);
        let event_stream = AnthropicStreamAdapter::new(sse_stream);

        Ok(Box::pin(event_stream))
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(serde::Serialize)]
struct Request {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<RequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(serde::Serialize)]
struct RequestMessage {
    role: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct Response {
    id: String,
    content: Vec<Content>,
    stop_reason: Option<String>,
    usage: Option<ResponseUsage>,
}

#[derive(serde::Deserialize)]
struct Content {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

#[derive(serde::Deserialize)]
struct ResponseUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ============================================================================
// Conversions
// ============================================================================

fn to_request(request: &ChatRequest, stream: Option<bool>) -> Request {
    let mut system = None;
    let mut messages = Vec::new();

    for msg in &request.messages {
        match msg.role {
            Role::System => {
                system = Some(msg.content.clone());
            }
            Role::User => {
                messages.push(RequestMessage {
                    role: "user".to_string(),
                    content: msg.content.clone(),
                });
            }
            Role::Assistant => {
                messages.push(RequestMessage {
                    role: "assistant".to_string(),
                    content: msg.content.clone(),
                });
            }
        }
    }

    Request {
        model: request.model.clone(),
        max_tokens: request.max_tokens.unwrap_or(4096),
        system,
        messages,
        temperature: request.temperature,
        stream,
    }
}

fn from_response(response: Response) -> ChatResponse {
    let content = response
        .content
        .into_iter()
        .filter(|c| c.content_type == "text")
        .map(|c| c.text)
        .collect::<Vec<_>>()
        .join("");

    ChatResponse {
        id: response.id,
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content,
            },
            finish_reason: response.stop_reason,
        }],
        usage: response.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        }),
    }
}

// ============================================================================
// Streaming
// ============================================================================

/// Adapter that converts SSE lines into Anthropic StreamEvents.
struct AnthropicStreamAdapter<S> {
    inner: SseEventStream<S>,
    done: bool,
    usage: Option<Usage>,
}

impl<S> AnthropicStreamAdapter<S> {
    fn new(inner: SseEventStream<S>) -> Self {
        Self {
            inner,
            done: false,
            usage: None,
        }
    }
}

impl<S> Stream for AnthropicStreamAdapter<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<StreamEvent, LLMError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if event.data.is_empty() {
                        continue;
                    }

                    // Parse Anthropic event JSON
                    match serde_json::from_str::<AnthropicStreamEvent>(&event.data) {
                        Ok(parsed) => match parsed {
                            AnthropicStreamEvent::ContentBlockDelta { delta } => {
                                if let Some(text) = delta.text
                                    && !text.is_empty()
                                {
                                    return Poll::Ready(Some(Ok(StreamEvent::Token(text))));
                                }
                            }
                            AnthropicStreamEvent::MessageDelta { usage: Some(u), .. } => {
                                self.usage = Some(Usage {
                                    prompt_tokens: 0,
                                    completion_tokens: u.output_tokens,
                                    total_tokens: u.output_tokens,
                                });
                            }
                            AnthropicStreamEvent::MessageStop => {
                                self.done = true;
                                return Poll::Ready(Some(Ok(StreamEvent::Done {
                                    usage: self.usage.take(),
                                })));
                            }
                            _ => {}
                        },
                        Err(e) => {
                            tracing::debug!(
                                data = %event.data,
                                error = %e,
                                "failed to parse Anthropic SSE event"
                            );
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(LLMError::Request(e))));
                }
                Poll::Ready(None) => {
                    self.done = true;
                    return Poll::Ready(Some(Ok(StreamEvent::Done {
                        usage: self.usage.take(),
                    })));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Anthropic SSE stream events.
///
/// Many fields are present in the API response but only some are used for streaming.
/// The unused fields are kept for complete deserialization.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[expect(dead_code, reason = "fields required for serde deserialization")]
enum AnthropicStreamEvent {
    MessageStart {
        message: Option<serde_json::Value>,
    },
    ContentBlockStart {
        index: Option<u32>,
        content_block: Option<serde_json::Value>,
    },
    ContentBlockDelta {
        delta: Delta,
    },
    ContentBlockStop {
        index: Option<u32>,
    },
    MessageDelta {
        delta: Option<serde_json::Value>,
        usage: Option<StreamUsage>,
    },
    MessageStop,
    Ping,
    #[serde(other)]
    Unknown,
}

#[derive(serde::Deserialize)]
struct Delta {
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct StreamUsage {
    output_tokens: u32,
}
