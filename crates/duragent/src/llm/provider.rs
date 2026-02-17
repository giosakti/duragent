//! LLM provider trait.

use async_trait::async_trait;
use futures::stream;

use super::{ChatRequest, ChatResponse, ChatStream, LLMError, StreamEvent};

/// Trait for LLM providers with different API formats.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Make a chat completion request.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LLMError>;

    /// Make a streaming chat completion request.
    ///
    /// Returns a stream of events (tokens and done signal).
    /// Default implementation calls the non-streaming API and emits the full response
    /// as a single chunk. Override this for native token-by-token streaming.
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LLMError> {
        let response = self.chat(request).await?;
        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        let usage = response.usage;

        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::Token(content)),
            Ok(StreamEvent::Done { usage }),
        ])))
    }
}
