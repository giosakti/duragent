//! OpenAI-compatible LLM provider.
//!
//! Works with OpenAI, OpenRouter, Ollama, and other compatible APIs.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;

use super::error::LLMError;
use super::provider::LLMProvider;
use super::types::{ChatRequest, ChatResponse, ChatStream, Message, StreamEvent, Usage};
use crate::sse::SseEventStream;

/// OpenAI-compatible provider (works for OpenAI, OpenRouter, Ollama).
pub struct OpenAICompatibleProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAICompatibleProvider {
    #[must_use]
    pub fn new(client: Client, base_url: String, api_key: Option<String>) -> Self {
        Self {
            client,
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatibleProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LLMError> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let response = req.json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        Ok(response.json().await?)
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LLMError> {
        let url = format!("{}/chat/completions", self.base_url);

        let stream_request = StreamRequest {
            model: request.model,
            messages: request.messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
        };

        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let response = req.json(&stream_request).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = SseEventStream::new(byte_stream);
        let event_stream = OpenAIStreamAdapter::new(sse_stream);

        Ok(Box::pin(event_stream))
    }
}

// ============================================================================
// Streaming Types
// ============================================================================

#[derive(serde::Serialize)]
struct StreamRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

/// Adapter that converts SSE lines into OpenAI StreamEvents.
struct OpenAIStreamAdapter<S> {
    inner: SseEventStream<S>,
    done: bool,
}

impl<S> OpenAIStreamAdapter<S> {
    fn new(inner: SseEventStream<S>) -> Self {
        Self { inner, done: false }
    }
}

impl<S> Stream for OpenAIStreamAdapter<S>
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
                    let data = event.data;
                    if data.is_empty() {
                        continue;
                    }

                    // Handle OpenAI's [DONE] marker
                    if data == "[DONE]" {
                        self.done = true;
                        return Poll::Ready(Some(Ok(StreamEvent::Done { usage: None })));
                    }

                    // Parse JSON chunk
                    match serde_json::from_str::<StreamChunk>(&data) {
                        Ok(chunk) => {
                            if let Some(choice) = chunk.choices.first()
                                && let Some(ref content) = choice.delta.content
                                && !content.is_empty()
                            {
                                return Poll::Ready(Some(Ok(StreamEvent::Token(content.clone()))));
                            }
                            // Skip chunks without content (e.g., role-only or usage-only)
                        }
                        Err(e) => {
                            tracing::debug!(data = %data, error = %e, "failed to parse SSE chunk");
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(LLMError::Request(e))));
                }
                Poll::Ready(None) => {
                    self.done = true;
                    return Poll::Ready(Some(Ok(StreamEvent::Done { usage: None })));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// OpenAI SSE stream chunk.
#[derive(serde::Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
    /// Usage is present in the API but not used during streaming.
    #[serde(default)]
    #[expect(dead_code, reason = "field required for serde deserialization")]
    usage: Option<Usage>,
}

#[derive(serde::Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(serde::Deserialize)]
struct StreamDelta {
    content: Option<String>,
}
