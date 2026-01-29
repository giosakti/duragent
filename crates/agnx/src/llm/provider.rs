//! LLM provider trait and types.

use std::str::FromStr;

use async_trait::async_trait;
use futures::stream;
use serde::Deserialize;

use super::error::LLMError;
use super::types::{ChatRequest, ChatResponse, ChatStream, StreamEvent};

// ============================================================================
// Provider Enum
// ============================================================================

/// Supported model providers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub enum Provider {
    Anthropic,
    Ollama,
    OpenAI,
    OpenRouter,
    Other(String),
}

impl Provider {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Ollama => "ollama",
            Provider::OpenAI => "openai",
            Provider::OpenRouter => "openrouter",
            Provider::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Provider {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "anthropic" => Provider::Anthropic,
            "ollama" => Provider::Ollama,
            "openai" => Provider::OpenAI,
            "openrouter" => Provider::OpenRouter,
            other => Provider::Other(other.to_string()),
        })
    }
}

impl From<String> for Provider {
    fn from(s: String) -> Self {
        s.parse().unwrap()
    }
}

// ============================================================================
// LLMProvider Trait
// ============================================================================

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
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        let usage = response.usage;

        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::Token(content)),
            Ok(StreamEvent::Done { usage }),
        ])))
    }
}
