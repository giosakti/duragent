//! Session management for Duragent.
//!
//! # Architecture
//!
//! ```text
//!  ┌─────────────────┐        ┌───────────────┐
//!  │ SessionRegistry │──owns──▶ SessionActor  │  (one per session, runs in a tokio task)
//!  │  (ID → Handle)  │        │  owns state,  │
//!  └────────┬────────┘        │  serializes   │
//!           │                 │  mutations    │
//!           │ clone           └───────▲───────┘
//!           ▼                         │ mpsc messages
//!  ┌─────────────────┐                │
//!  │  SessionHandle  │────────────────┘  (cheap cloneable sender)
//!  └─────────────────┘
//!
//!  ┌───────────────────┐
//!  │ ChatSessionCache  │  Maps (gateway, chat_id, agent) → session_id.
//!  │                   │  Used by gateway routing to find/create sessions.
//!  └───────────────────┘
//! ```
//!
//! - **SessionActor** — owns mutable session state; processes messages sequentially
//!   via an mpsc channel so no locks are held across await points.
//! - **SessionHandle** — cloneable reference that sends messages to an actor.
//!   All external code interacts with sessions through handles.
//! - **SessionRegistry** — maps session IDs to handles; manages actor lifecycle
//!   (create, recover, destroy).
//! - **ChatSessionCache** — maps (gateway, chat_id, agent) tuples to session IDs
//!   so gateway messages route to the correct long-lived session.

mod actor;
mod actor_types;
mod agentic_loop;
mod chat_session_cache;
mod events;
mod handle;
mod registry;
mod snapshot;
mod sse_stream;

// Types and errors
pub use actor_types::{
    ActorError, DEFAULT_ACTOR_MESSAGE_LIMIT, DEFAULT_SILENT_BUFFER_CAP, SessionMetadata,
    SilentMessageEntry, actor_message_limit,
};
pub use chat_session_cache::ChatSessionCache;
pub use events::{
    ApprovalDecisionType, SessionEndReason, SessionEvent, SessionEventPayload, ToolResultData,
};
pub use handle::SessionHandle;
pub use registry::{CreateSessionOpts, RecoveryResult, SessionRegistry};
pub use snapshot::{CheckpointState, SessionConfig, SessionSnapshot};

// Streaming
pub use sse_stream::{AccumulatingStream, StreamConfig};

// Agentic loop
pub use agentic_loop::{
    AGENTIC_LOOP_LOCK_TIMEOUT_SECS, AgenticError, AgenticResult, PendingApproval, ResumeContext,
    STEERING_CHANNEL_CAPACITY, SteeringMessage, SteeringReceiver, SteeringSender,
    resume_agentic_loop, run_agentic_loop,
};
