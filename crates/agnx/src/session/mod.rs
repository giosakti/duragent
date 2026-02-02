//! Session management for Agnx.
//!
//! v0.1.0: In-memory session store.
//! v0.2.0: Persistent storage with JSONL event log + YAML snapshots.
//! v0.4.0: Agentic loop for tool-using agents.
//!         Per-session actor architecture for lock-free concurrency.

mod actor;
mod agentic;
mod chat_session_cache;
mod error;
mod event_reader;
mod event_writer;
mod events;
mod handle;
mod registry;
mod resume;
mod snapshot;
mod snapshot_loader;
mod snapshot_writer;
mod stream;

// Types and errors
pub use actor::{ActorError, SessionMetadata};
pub use chat_session_cache::ChatSessionCache;
pub use error::{Result, SessionError};
pub use events::{
    ApprovalDecisionType, SessionEndReason, SessionEvent, SessionEventPayload, ToolResultData,
};
pub use handle::SessionHandle;
pub use registry::{RecoveryResult, SessionRegistry};
pub use snapshot::{PendingApproval, SessionConfig, SessionSnapshot};

// Event I/O
pub use event_reader::EventReader;
pub use event_writer::EventWriter;
pub use snapshot_loader::load_snapshot;
pub use snapshot_writer::write_snapshot;

// Resume (for attach command and gateway cache rebuild)
pub use resume::{ResumedSession, resume_session};

// Streaming
pub use stream::{AccumulatingStream, StreamConfig};

// Agentic loop
pub use agentic::{AgenticError, AgenticResult, resume_agentic_loop, run_agentic_loop};
