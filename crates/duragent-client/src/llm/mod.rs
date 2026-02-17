//! LLM types shared between client and server.

mod error;

pub use error::{LLMError, check_response_error};

// Data types are defined in duragent-types; re-exported here for compatibility.
pub use duragent_types::llm::*;
pub use duragent_types::provider::Provider;

// ChatStream depends on LLMError which stays in this crate.
use futures::Stream;
use std::pin::Pin;
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LLMError>> + Send>>;
