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
use super::types::{
    ChatRequest, ChatResponse, ChatStream, FunctionCall, Message, StreamEvent, ToolCall,
    ToolDefinition, Usage,
};
use crate::sse_parser::SseEventStream;

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
            tools: request.tools,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    stream: bool,
    /// Request usage stats in streaming response (OpenAI/OpenRouter support).
    stream_options: StreamOptions,
}

#[derive(serde::Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// Adapter that converts SSE lines into OpenAI StreamEvents.
struct OpenAIStreamAdapter<S> {
    inner: SseEventStream<S>,
    done: bool,
    /// Accumulated tool calls from streaming chunks.
    /// Tool calls come in pieces: first the id and name, then arguments in chunks.
    tool_calls: Vec<ToolCallAccumulator>,
    /// Usage from the final chunk (when stream_options.include_usage is true).
    usage: Option<Usage>,
}

/// Accumulates tool call data from streaming chunks.
#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    function_name: String,
    arguments: String,
}

impl<S> OpenAIStreamAdapter<S> {
    fn new(inner: SseEventStream<S>) -> Self {
        Self {
            inner,
            done: false,
            tool_calls: Vec::new(),
            usage: None,
        }
    }

    /// Convert accumulated tool calls into final ToolCall structs.
    fn finalize_tool_calls(&mut self) -> Vec<ToolCall> {
        std::mem::take(&mut self.tool_calls)
            .into_iter()
            .filter(|tc| !tc.id.is_empty())
            .map(|tc| ToolCall {
                id: tc.id,
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: tc.function_name,
                    arguments: tc.arguments,
                },
            })
            .collect()
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

                        // If we accumulated tool calls, emit them before Done
                        if !self.tool_calls.is_empty() {
                            let tool_calls = self.finalize_tool_calls();
                            if !tool_calls.is_empty() {
                                return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(tool_calls))));
                            }
                        }

                        return Poll::Ready(Some(Ok(StreamEvent::Done {
                            usage: self.usage.take(),
                        })));
                    }

                    // Parse JSON chunk
                    match serde_json::from_str::<StreamChunk>(&data) {
                        Ok(chunk) => {
                            // Capture usage if present (typically in final chunk)
                            if let Some(usage) = chunk.usage {
                                self.usage = Some(usage);
                            }

                            if let Some(choice) = chunk.choices.first() {
                                // Handle content tokens
                                if let Some(ref content) = choice.delta.content
                                    && !content.is_empty()
                                {
                                    return Poll::Ready(Some(Ok(StreamEvent::Token(
                                        content.clone(),
                                    ))));
                                }

                                // Handle tool calls (accumulated across chunks)
                                if let Some(ref tool_calls) = choice.delta.tool_calls {
                                    for tc in tool_calls {
                                        // Ensure we have enough slots
                                        while self.tool_calls.len() <= tc.index {
                                            self.tool_calls.push(ToolCallAccumulator::default());
                                        }

                                        let acc = &mut self.tool_calls[tc.index];

                                        // ID comes in first chunk
                                        if let Some(ref id) = tc.id {
                                            acc.id = id.clone();
                                        }

                                        // Function name and arguments come in subsequent chunks
                                        if let Some(ref func) = tc.function {
                                            if let Some(ref name) = func.name {
                                                acc.function_name = name.clone();
                                            }
                                            if let Some(ref args) = func.arguments {
                                                acc.arguments.push_str(args);
                                            }
                                        }
                                    }
                                }

                                // Check for finish_reason "tool_calls"
                                if choice.finish_reason.as_deref() == Some("tool_calls") {
                                    let tool_calls = self.finalize_tool_calls();
                                    if !tool_calls.is_empty() {
                                        return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(
                                            tool_calls,
                                        ))));
                                    }
                                }
                            }
                            // Skip chunks without relevant content
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

                    // Emit any pending tool calls
                    if !self.tool_calls.is_empty() {
                        let tool_calls = self.finalize_tool_calls();
                        if !tool_calls.is_empty() {
                            return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(tool_calls))));
                        }
                    }

                    return Poll::Ready(Some(Ok(StreamEvent::Done {
                        usage: self.usage.take(),
                    })));
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
    /// Usage stats (present when stream_options.include_usage is true).
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(serde::Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct StreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<StreamToolCall>>,
}

/// Tool call chunk in streaming response.
#[derive(serde::Deserialize)]
struct StreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<StreamFunctionCall>,
}

/// Function call chunk in streaming response.
#[derive(serde::Deserialize)]
struct StreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}
