//! Shared API types used by both server handlers and client.
//!
//! These types define the contract between server and client.
//! Changes here affect both sides, preventing silent drift.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// ID Prefixes
// ============================================================================

/// ID prefix for sessions.
pub const SESSION_ID_PREFIX: &str = "session_";

/// ID prefix for messages.
pub const MESSAGE_ID_PREFIX: &str = "msg_";

// ============================================================================
// SSE Event Names
// ============================================================================

/// SSE event type names used in streaming responses.
pub mod sse {
    pub const START: &str = "start";
    pub const TOKEN: &str = "token";
    pub const DONE: &str = "done";
    pub const ERROR: &str = "error";
    pub const CANCELLED: &str = "cancelled";
    pub const APPROVAL_REQUIRED: &str = "approval_required";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
}

// ============================================================================
// Agent Types
// ============================================================================

/// Summary of an agent in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Detailed agent information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetailResponse {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentMetadataResponse,
    pub spec: AgentSpecResponse,
}

/// Agent metadata in responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadataResponse {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Agent spec in responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpecResponse {
    pub model: AgentModelResponse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Agent model configuration in responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelResponse {
    pub provider: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Response for listing agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
}

// ============================================================================
// Session Types
// ============================================================================

/// Request to create a new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub agent: String,
}

/// Session status.
///
/// Used in session responses and client-side session handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is active and ready for messages.
    Active,
    /// Session is paused (client disconnected with on_disconnect: pause).
    Paused,
    /// Session is running in background (client disconnected with on_disconnect: continue).
    Running,
    /// Session has completed.
    Completed,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
            SessionStatus::Paused => write!(f, "paused"),
            SessionStatus::Running => write!(f, "running"),
            SessionStatus::Completed => write!(f, "completed"),
        }
    }
}

/// Response for session creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: String,
}

/// Response for getting a single session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSessionResponse {
    pub session_id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Summary of a session in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: String,
}

/// Response for listing sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSummary>,
}

// ============================================================================
// Message Types
// ============================================================================

/// Request to send a message to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

/// A message in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    pub role: String,
    pub content: String,
}

/// Response for getting messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMessagesResponse {
    pub messages: Vec<MessageResponse>,
}

/// Response from sending a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub message_id: String,
    pub role: String,
    pub content: String,
}

// ============================================================================
// Approval Types
// ============================================================================

/// Approval decision from user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Allow this command once.
    AllowOnce,
    /// Allow this command pattern always (saves to policy.local.yaml).
    AllowAlways,
    /// Deny this command.
    Deny,
}

/// Request to approve or deny a pending command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveCommandRequest {
    /// The tool call ID that needs approval.
    pub call_id: String,
    /// The command being approved (for verification).
    pub command: String,
    /// The approval decision.
    pub decision: ApprovalDecision,
}

/// Response from approving a command.
///
/// Can be either a completion (agent finished responding) or another
/// pending approval (agent needs to run another tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum ApproveCommandResponse {
    /// Agent completed its response.
    #[serde(rename = "complete")]
    Complete {
        /// The message ID of the response.
        message_id: String,
        /// The agent's response content.
        content: String,
    },
    /// Another tool needs approval.
    #[serde(rename = "pending_approval")]
    PendingApproval {
        /// The tool call ID that needs approval.
        call_id: String,
        /// The command that needs approval.
        command: String,
    },
}

/// SSE payload for approval_required event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequiredEvent {
    pub call_id: String,
    pub command: String,
}

/// Response when a message requires approval before completing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApprovalResponse {
    /// The session ID.
    pub session_id: String,
    /// The tool call ID that needs approval.
    pub call_id: String,
    /// The command being approved.
    pub command: String,
}

/// Response for getting a single session (extended with pending_approval).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSessionDetailResponse {
    pub session_id: String,
    pub agent: String,
    pub status: SessionStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Pending approval waiting for user decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval: Option<PendingApprovalInfo>,
}

/// Info about a pending approval in session responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApprovalInfo {
    pub call_id: String,
    pub command: String,
    pub expires_at: String,
}
