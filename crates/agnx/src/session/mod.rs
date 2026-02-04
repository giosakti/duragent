//! Session management for Agnx.
//!
//! v0.1.0: In-memory session store.
//! v0.2.0: Persistent storage with JSONL event log + YAML snapshots.
//! v0.4.0: Agentic loop for tool-using agents.
//!         Per-session actor architecture for lock-free concurrency.
//!         Refactored persistence into store/ module with trait-based interfaces.

mod actor;
mod agentic_loop;
mod chat_session_cache;
mod events;
mod handle;
mod registry;
mod snapshot;
mod sse_stream;

// Types and errors
pub use actor::{ActorError, SessionMetadata};
pub use chat_session_cache::ChatSessionCache;
pub use events::{
    ApprovalDecisionType, SessionEndReason, SessionEvent, SessionEventPayload, ToolResultData,
};
pub use handle::SessionHandle;
pub use registry::{RecoveryResult, SessionRegistry};
pub use snapshot::{PendingApproval, SessionConfig, SessionSnapshot};

// Streaming
pub use sse_stream::{AccumulatingStream, StreamConfig};

// Agentic loop
pub use agentic_loop::{AgenticError, AgenticResult, resume_agentic_loop, run_agentic_loop};
