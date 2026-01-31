//! Session management HTTP handlers.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as PathExtract, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use uuid::Uuid;

use crate::agent::{AgentSpec, OnDisconnect};
use crate::api::{
    ApprovalDecision, ApproveCommandRequest, CreateSessionRequest, CreateSessionResponse,
    GetMessagesResponse, GetSessionResponse, ListSessionsResponse, MessageResponse,
    PendingApprovalResponse, SendMessageRequest, SendMessageResponse, SessionStatus,
    SessionSummary,
};
use crate::handlers::problem_details;
use crate::llm::{ChatRequest, LLMProvider, Message, Role};
use crate::server::AppState;
use crate::session::{
    AccumulatingStream, AgenticResult, ApprovalDecisionType, EventContext, SessionContext,
    SessionEventPayload, StreamConfig, build_chat_request, build_system_message,
    clear_pending_approval, commit_event, get_pending_approval, persist_assistant_message,
    record_event, resume_agentic_loop, run_agentic_loop, set_pending_approval,
};
use crate::tools::{ToolExecutor, ToolResult};

// ============================================================================
// Query Types
// ============================================================================

#[derive(Deserialize)]
pub struct GetMessagesQuery {
    limit: Option<u32>,
}

// ============================================================================
// Handlers
// ============================================================================

/// GET /api/v1/sessions
pub async fn list_sessions(State(state): State<AppState>) -> Json<ListSessionsResponse> {
    let sessions: Vec<SessionSummary> = state
        .sessions
        .list()
        .await
        .into_iter()
        .map(|s| SessionSummary {
            session_id: s.id,
            agent: s.agent,
            status: s.status,
            created_at: s.created_at.to_rfc3339(),
        })
        .collect();

    Json(ListSessionsResponse { sessions })
}

/// POST /api/v1/sessions
pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let Some(agent_spec) = state.agents.get(&req.agent) else {
        return problem_details::not_found(format!("agent '{}' not found", req.agent))
            .into_response();
    };

    let session = state.sessions.create(&req.agent).await;
    let ctx = SessionContext {
        sessions: &state.sessions,
        sessions_path: &state.sessions_path,
        session_id: &session.id,
        agent: &session.agent,
        created_at: session.created_at,
        status: SessionStatus::Active,
        on_disconnect: agent_spec.session.on_disconnect,
        gateway: None,
        gateway_chat_id: None,
        pending_approval: None,
    };
    if let Err(e) = commit_event(
        &ctx,
        SessionEventPayload::SessionStart {
            session_id: session.id.clone(),
            agent: session.agent.clone(),
        },
    )
    .await
    {
        return problem_details::internal_error(format!("failed to persist session: {}", e))
            .into_response();
    }

    let response = CreateSessionResponse {
        session_id: session.id.clone(),
        agent: session.agent.clone(),
        status: session.status,
        created_at: session.created_at.to_rfc3339(),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// GET /api/v1/sessions/{session_id}
pub async fn get_session(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
) -> impl IntoResponse {
    let Some(session) = state.sessions.get(&session_id).await else {
        return problem_details::not_found("session not found").into_response();
    };

    let response = GetSessionResponse {
        session_id: session.id.clone(),
        agent: session.agent.clone(),
        status: session.status,
        created_at: session.created_at.to_rfc3339(),
        updated_at: Some(session.updated_at.to_rfc3339()),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// GET /api/v1/sessions/{session_id}/messages
pub async fn get_messages(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
    Query(query): Query<GetMessagesQuery>,
) -> impl IntoResponse {
    // First check if session exists
    if state.sessions.get(&session_id).await.is_none() {
        return problem_details::not_found("session not found").into_response();
    }

    let messages = state
        .sessions
        .get_messages(&session_id)
        .await
        .unwrap_or_default();

    let iter = messages.into_iter().map(|m| MessageResponse {
        role: m.role.to_string(),
        content: m.content.unwrap_or_default(),
    });
    let messages: Vec<_> = match query.limit {
        Some(limit) => iter.take(limit as usize).collect(),
        None => iter.collect(),
    };

    (StatusCode::OK, Json(GetMessagesResponse { messages })).into_response()
}

/// POST /api/v1/sessions/{session_id}/messages
pub async fn send_message(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let ctx = match prepare_chat_context(&state, &session_id, req.content).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    // Get session info for agent name
    let session = match state.sessions.get(&session_id).await {
        Some(s) => s,
        None => return problem_details::not_found("session not found").into_response(),
    };

    // Check if agent has tools configured
    if !ctx.agent_spec.tools.is_empty() {
        // Use agentic loop for tool-using agents
        return send_message_agentic(&state, &session_id, &session.agent, ctx).await;
    }

    // Simple single-turn for agents without tools
    let chat_response = match ctx.provider.chat(ctx.request).await {
        Ok(resp) => resp,
        Err(e) => {
            return problem_details::internal_error(format!("llm request failed: {}", e))
                .into_response();
        }
    };

    let assistant_content = chat_response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default();

    // Persist assistant message to store and event log
    if let Err(e) = persist_assistant_message(
        &state.sessions,
        &state.sessions_path,
        &session_id,
        &session.agent,
        assistant_content.clone(),
        chat_response.usage.clone(),
    )
    .await
    {
        return problem_details::internal_error(format!(
            "failed to persist assistant message: {}",
            e
        ))
        .into_response();
    }

    let response = SendMessageResponse {
        message_id: format!(
            "{}{}",
            crate::api::MESSAGE_ID_PREFIX,
            Uuid::new_v4().simple()
        ),
        role: "assistant".to_string(),
        content: assistant_content,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Handle send_message for agents with tools using the agentic loop.
async fn send_message_agentic(
    state: &AppState,
    session_id: &str,
    agent_name: &str,
    ctx: ChatContext,
) -> Response {
    // Create tool executor with policy
    let executor = ToolExecutor::new(
        ctx.agent_spec.tools.clone(),
        state.sandbox.clone(),
        ctx.agent_dir.clone(),
        ctx.agent_spec.policy.clone(),
        agent_name.to_string(),
    )
    .with_session_id(session_id.to_string());

    // Run the agentic loop
    let event_ctx = EventContext {
        sessions: state.sessions.clone(),
        sessions_path: state.sessions_path.clone(),
        session_id: session_id.to_string(),
    };
    let result = match run_agentic_loop(
        ctx.provider,
        &executor,
        &ctx.agent_spec,
        ctx.request.messages,
        &event_ctx,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return problem_details::internal_error(format!("agentic loop failed: {}", e))
                .into_response();
        }
    };

    handle_agentic_result(state, session_id, agent_name, result, false).await
}

/// POST /api/v1/sessions/{session_id}/stream
///
/// SSE endpoint for streaming chat completions.
///
/// Request body: `{"content": "..."}`
///
/// Events emitted:
/// - `start`: `{}` — signals streaming has begun
/// - `token`: `{"content": "..."}` — streamed content chunks
/// - `done`: `{"message_id": "msg_...", "usage": {...}}` — stream complete with message ID
/// - `cancelled`: `{}` — stream was cancelled (client disconnected)
/// - `error`: `{"message": "..."}` — on error (timeout, LLM failure)
///
/// Cancellation behavior:
/// - When the client disconnects, the in-flight LLM request is cancelled
/// - Any accumulated content is saved to the session
/// - If agent has `on_disconnect: pause`, session is paused and snapshot is written
/// - If agent has `on_disconnect: continue`, LLM continues in background, events are logged
pub async fn stream_session(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let ctx = match prepare_chat_context(&state, &session_id, req.content).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };

    // Get session info for snapshot (we need created_at and agent)
    let session = match state.sessions.get(&session_id).await {
        Some(s) => s,
        None => return problem_details::not_found("session not found").into_response(),
    };

    let stream = match ctx.provider.chat_stream(ctx.request).await {
        Ok(s) => s,
        Err(e) => {
            return problem_details::internal_error(format!("llm request failed: {}", e))
                .into_response();
        }
    };

    let message_id = format!(
        "{}{}",
        crate::api::MESSAGE_ID_PREFIX,
        Uuid::new_v4().simple()
    );
    let cancel_token = CancellationToken::new();

    debug!(
        session_id = %session_id,
        message_id = %message_id,
        on_disconnect = ?ctx.on_disconnect,
        "Starting SSE stream"
    );

    let sse_stream = AccumulatingStream::new(
        stream,
        StreamConfig {
            sessions: state.sessions.clone(),
            session_id,
            agent: session.agent.clone(),
            created_at: session.created_at,
            message_id,
            idle_timeout: Duration::from_secs(state.idle_timeout_seconds),
            cancel_token,
            on_disconnect: ctx.on_disconnect,
            sessions_path: state.sessions_path.clone(),
            background_tasks: state.background_tasks.clone(),
        },
    );

    let keep_alive = KeepAlive::new()
        .interval(Duration::from_secs(state.keep_alive_interval_seconds))
        .text("keep-alive");

    Sse::new(sse_stream).keep_alive(keep_alive).into_response()
}

/// POST /api/v1/sessions/{session_id}/approve
///
/// Approve or deny a pending tool execution.
///
/// If approved, executes the tool and resumes the agentic loop.
/// Returns the final response or a new pending approval if another tool needs approval.
pub async fn approve_command(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
    Json(req): Json<ApproveCommandRequest>,
) -> impl IntoResponse {
    // Verify session exists
    let Some(session) = state.sessions.get(&session_id).await else {
        return problem_details::not_found("session not found").into_response();
    };

    // Load pending approval from snapshot
    let pending = match get_pending_approval(&state.sessions_path, &session_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return problem_details::not_found("no pending approval for this session")
                .into_response();
        }
        Err(e) => {
            return problem_details::internal_error(format!(
                "failed to load pending approval: {}",
                e
            ))
            .into_response();
        }
    };

    // Validate call_id matches
    if pending.call_id != req.call_id {
        return problem_details::bad_request(format!(
            "call_id mismatch: expected '{}', got '{}'",
            pending.call_id, req.call_id
        ))
        .into_response();
    }

    // Map API decision to internal type
    let decision = match req.decision {
        ApprovalDecision::AllowOnce => ApprovalDecisionType::AllowOnce,
        ApprovalDecision::AllowAlways => ApprovalDecisionType::AllowAlways,
        ApprovalDecision::Deny => ApprovalDecisionType::Deny,
    };

    // Record the approval decision event
    if let Err(e) = record_event(
        &state.sessions,
        &state.sessions_path,
        &session_id,
        SessionEventPayload::ApprovalDecision {
            call_id: req.call_id.clone(),
            decision,
        },
    )
    .await
    {
        return problem_details::internal_error(format!(
            "failed to persist approval decision: {}",
            e
        ))
        .into_response();
    }

    // Get agent spec for tool execution
    let Some(agent_spec) = state.agents.get(&session.agent) else {
        return problem_details::internal_error("session references non-existent agent")
            .into_response();
    };

    // If allow_always, save pattern to policy.local.yaml with locking
    if req.decision == ApprovalDecision::AllowAlways {
        // Get or create lock for this agent
        let lock = state
            .policy_locks
            .entry(session.agent.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();

        // Hold lock while reading, modifying, and writing
        let _guard = lock.lock().await;
        let mut policy = agent_spec.policy.clone();
        policy.reload_local(&agent_spec.agent_dir).await;
        policy.add_allow_pattern(crate::agent::ToolType::Bash, &req.command);
        if let Err(e) = policy.save_local(&agent_spec.agent_dir).await {
            debug!(
                error = %e,
                command = %req.command,
                "Failed to save allow pattern to policy.local.yaml"
            );
        }
    }

    // Determine tool result based on decision
    let tool_result = if req.decision == ApprovalDecision::Deny {
        // Denial - create a rejection message for the LLM
        ToolResult {
            success: false,
            content: format!(
                "Command '{}' was denied by the user. Please try a different approach.",
                req.command
            ),
        }
    } else {
        // Approved - execute the tool
        let executor = ToolExecutor::new(
            agent_spec.tools.clone(),
            state.sandbox.clone(),
            agent_spec.agent_dir.clone(),
            agent_spec.policy.clone(),
            session.agent.clone(),
        )
        .with_session_id(session_id.clone());

        // Build tool call from pending approval
        let tool_call = crate::llm::ToolCall {
            id: pending.call_id.clone(),
            tool_type: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: pending.tool_name.clone(),
                arguments: pending.arguments.to_string(),
            },
        };

        // Execute with policy bypassed (already approved)
        match executor.execute_bypassing_policy(&tool_call).await {
            Ok(result) => result,
            Err(e) => ToolResult {
                success: false,
                content: format!("Tool execution failed: {}", e),
            },
        }
    };

    // Clear pending approval
    if let Err(e) = clear_pending_approval(&state.sessions, &state.sessions_path, &session_id).await
    {
        debug!(error = %e, "Failed to clear pending approval");
    }

    // Get provider for resuming the loop
    let Some(provider) = state.providers.get(
        &agent_spec.model.provider,
        agent_spec.model.base_url.as_deref(),
    ) else {
        return problem_details::internal_error(format!(
            "provider '{}' not configured",
            agent_spec.model.provider
        ))
        .into_response();
    };

    // Create executor for resume
    let executor = ToolExecutor::new(
        agent_spec.tools.clone(),
        state.sandbox.clone(),
        agent_spec.agent_dir.clone(),
        agent_spec.policy.clone(),
        session.agent.clone(),
    )
    .with_session_id(session_id.clone());

    // Set session to Running before resuming (accurate status during execution)
    if let Err(e) = state
        .sessions
        .set_status(&session_id, SessionStatus::Running)
        .await
    {
        debug!(error = %e, "Failed to set session status to Running");
    }

    // Resume the agentic loop
    let event_ctx = EventContext {
        sessions: state.sessions.clone(),
        sessions_path: state.sessions_path.clone(),
        session_id: session_id.clone(),
    };
    let result = match resume_agentic_loop(
        provider,
        &executor,
        agent_spec,
        pending,
        tool_result,
        &event_ctx,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Reset to Active on error
            let _ = state
                .sessions
                .set_status(&session_id, SessionStatus::Active)
                .await;
            return problem_details::internal_error(format!("agentic loop failed: {}", e))
                .into_response();
        }
    };

    // Handle result using shared helper
    handle_agentic_result(&state, &session_id, &session.agent, result, true).await
}

// ============================================================================
// Implementation Details
// ============================================================================

/// Errors that can occur when sending a message.
#[derive(Debug)]
enum SendMessageError {
    SessionNotFound,
    AgentNotFound,
    MessageAddFailed,
    PersistFailed(String),
    ProviderNotConfigured(String),
}

impl IntoResponse for SendMessageError {
    fn into_response(self) -> Response {
        match self {
            Self::SessionNotFound => problem_details::not_found("session not found"),
            Self::AgentNotFound => {
                problem_details::internal_error("session references non-existent agent")
            }
            Self::MessageAddFailed => {
                problem_details::internal_error("failed to add message to session")
            }
            Self::PersistFailed(msg) => problem_details::internal_error(msg),
            Self::ProviderNotConfigured(provider) => problem_details::internal_error(format!(
                "provider '{}' not configured, check API key environment variable",
                provider
            )),
        }
        .into_response()
    }
}

/// Prepared context for LLM chat, including request, provider, and agent config.
struct ChatContext {
    request: ChatRequest,
    provider: Arc<dyn LLMProvider>,
    on_disconnect: OnDisconnect,
    agent_spec: Arc<AgentSpec>,
    agent_dir: std::path::PathBuf,
}

/// Prepare chat context for LLM request.
///
/// Validates session and agent, adds user message, builds system prompt and history,
/// and returns the ChatRequest with the provider and agent configuration.
async fn prepare_chat_context(
    state: &AppState,
    session_id: &str,
    user_content: String,
) -> Result<ChatContext, SendMessageError> {
    let Some(session) = state.sessions.get(session_id).await else {
        return Err(SendMessageError::SessionNotFound);
    };

    let Some(agent) = state.agents.get(&session.agent) else {
        return Err(SendMessageError::AgentNotFound);
    };

    let content_for_event = user_content.clone();
    let user_message = Message::text(Role::User, user_content);
    if state
        .sessions
        .add_message(session_id, user_message)
        .await
        .is_err()
    {
        return Err(SendMessageError::MessageAddFailed);
    }

    if let Err(e) = record_event(
        &state.sessions,
        &state.sessions_path,
        session_id,
        SessionEventPayload::UserMessage {
            content: content_for_event,
        },
    )
    .await
    {
        return Err(SendMessageError::PersistFailed(format!(
            "Failed to persist user message: {}",
            e
        )));
    }

    let history = state
        .sessions
        .get_messages(session_id)
        .await
        .unwrap_or_default();
    let system_message = build_system_message(agent);

    let Some(provider) = state
        .providers
        .get(&agent.model.provider, agent.model.base_url.as_deref())
    else {
        return Err(SendMessageError::ProviderNotConfigured(
            agent.model.provider.to_string(),
        ));
    };

    let chat_request = build_chat_request(
        &agent.model.name,
        system_message.as_deref(),
        &history,
        agent.model.temperature,
        agent.model.max_output_tokens,
    );

    let agent_dir = agent.agent_dir.clone();
    let agent_spec = Arc::new(agent.clone());

    Ok(ChatContext {
        request: chat_request,
        provider,
        on_disconnect: agent.session.on_disconnect,
        agent_spec,
        agent_dir,
    })
}

/// Handle the result from an agentic loop (initial or resume).
///
/// For Complete: persists the message and returns 200 with the response.
/// For AwaitingApproval: saves pending state, sets Paused, returns 202.
async fn handle_agentic_result(
    state: &AppState,
    session_id: &str,
    agent_name: &str,
    result: AgenticResult,
    is_resume: bool,
) -> Response {
    match result {
        AgenticResult::Complete { content, usage, .. } => {
            // Persist the final assistant message
            if let Err(e) = persist_assistant_message(
                &state.sessions,
                &state.sessions_path,
                session_id,
                agent_name,
                content.clone(),
                usage,
            )
            .await
            {
                return problem_details::internal_error(format!(
                    "failed to persist assistant message: {}",
                    e
                ))
                .into_response();
            }

            // On resume, set back to Active (was Running during execution)
            if is_resume {
                let _ = state
                    .sessions
                    .set_status(session_id, SessionStatus::Active)
                    .await;
            }

            let message_id = format!(
                "{}{}",
                crate::api::MESSAGE_ID_PREFIX,
                Uuid::new_v4().simple()
            );
            let response = SendMessageResponse {
                message_id,
                role: "assistant".to_string(),
                content,
            };
            (StatusCode::OK, Json(response)).into_response()
        }

        AgenticResult::AwaitingApproval { pending, .. } => {
            // Save pending approval to snapshot
            if let Err(e) =
                set_pending_approval(&state.sessions, &state.sessions_path, session_id, &pending)
                    .await
            {
                return problem_details::internal_error(format!(
                    "failed to save pending approval: {}",
                    e
                ))
                .into_response();
            }

            // Set session to Paused
            let _ = state
                .sessions
                .set_status(session_id, SessionStatus::Paused)
                .await;

            let response = PendingApprovalResponse {
                session_id: session_id.to_string(),
                call_id: pending.call_id,
                command: pending.command,
            };
            (StatusCode::ACCEPTED, Json(response)).into_response()
        }
    }
}
