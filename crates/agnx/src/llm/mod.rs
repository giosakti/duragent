//! LLM provider client for chat completions.

mod anthropic;
mod error;
mod openai;
mod provider;
mod registry;
mod types;

pub use anthropic::{AnthropicAuth, AnthropicProvider};
pub use error::LLMError;
pub use openai::OpenAICompatibleProvider;
pub use provider::{LLMProvider, Provider};
pub use registry::ProviderRegistry;
pub use types::{
    ChatRequest, ChatStream, FunctionCall, FunctionDefinition, Message, Role, StreamEvent,
    ToolCall, ToolDefinition, Usage,
};
