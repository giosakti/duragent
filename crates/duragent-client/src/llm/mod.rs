//! LLM types shared between client and server.

mod error;
mod types;

pub use error::{LLMError, check_response_error};
pub use types::{
    ChatRequest, ChatResponse, ChatStream, Choice, FunctionCall, FunctionDefinition, Message, Role,
    StreamEvent, ToolCall, ToolDefinition, Usage,
};
