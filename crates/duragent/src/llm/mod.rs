//! LLM provider client for chat completions.

// Re-export LLM data types from duragent-types (via duragent-client re-export)
pub use duragent_client::llm::*;

#[cfg(feature = "server")]
mod anthropic;
#[cfg(feature = "server")]
mod openai;
#[cfg(feature = "server")]
mod provider;
#[cfg(feature = "server")]
mod registry;

#[cfg(feature = "server")]
pub use anthropic::{AnthropicAuth, AnthropicProvider};
#[cfg(feature = "server")]
pub use openai::OpenAICompatibleProvider;
#[cfg(feature = "server")]
pub use provider::LLMProvider;
#[cfg(feature = "server")]
pub use registry::ProviderRegistry;
