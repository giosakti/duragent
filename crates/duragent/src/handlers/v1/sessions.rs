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
use tracing::{debug, error};
use ulid::Ulid;

use crate::agent::{AgentSpec, OnDisconnect};
use crate::api::{
    ApprovalDecision, ApproveCommandRequest, CreateSessionRequest, CreateSessionResponse,
    GetMessagesResponse, GetSessionResponse, ListSessionsResponse, MessageResponse,
    PendingApprovalResponse, SendMessageRequest, SendMessageResponse, SessionStatus,
    SessionSummary,
};
use crate::context::{ContextBuilder, TokenBudget, load_all_directives};
use crate::handlers::problem_details;
use crate::llm::{ChatRequest, LLMProvider};
use crate::server::AppState;
use crate::session::{
    AccumulatingStream, AgenticResult, ApprovalDecisionType, SessionHandle, StreamConfig,
    resume_agentic_loop, run_agentic_loop,
};
use crate::tools::{ToolDependencies, ToolResult, build_executor};

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
        .services
        .session_registry
        .list()
        .await
        .into_iter()
        .map(|m| SessionSummary {
            session_id: m.id,
            agent: m.agent,
            status: m.status,
            created_at: m.created_at.to_rfc3339(),
        })
        .collect();

    Json(ListSessionsResponse { sessions })
}

/// POST /api/v1/sessions
pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let Some(agent_spec) = state.services.agents.get(&req.agent) else {
        return problem_details::not_found(format!("agent '{}' not found", req.agent))
            .into_response();
    };

    // Create session via registry - actor records SessionStart event automatically
    let handle = match state
        .services
        .session_registry
        .create(
            &req.agent,
            agent_spec.session.on_disconnect,
            None,
            None,
            crate::session::DEFAULT_SILENT_BUFFER_CAP,
            crate::session::actor_message_limit(agent_spec.model.effective_max_input_tokens()),
            agent_spec.session.compaction,
        )
        .await
    {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to create session");
            return problem_details::internal_error("failed to create session").into_response();
        }
    };

    // Get metadata for response
    let metadata = match handle.get_metadata().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to get session metadata");
            return problem_details::internal_error("failed to get session metadata")
                .into_response();
        }
    };

    let response = CreateSessionResponse {
        session_id: metadata.id,
        agent: metadata.agent,
        status: metadata.status,
        created_at: metadata.created_at.to_rfc3339(),
    };

    (StatusCode::CREATED, Json(response)).into_response()
}

/// GET /api/v1/sessions/{session_id}
pub async fn get_session(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
) -> impl IntoResponse {
    let Some(handle) = state.services.session_registry.get(&session_id) else {
        return problem_details::not_found("session not found").into_response();
    };

    let metadata = match handle.get_metadata().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to get session metadata");
            return problem_details::internal_error("failed to get session metadata")
                .into_response();
        }
    };

    let response = GetSessionResponse {
        session_id: metadata.id,
        agent: metadata.agent,
        status: metadata.status,
        created_at: metadata.created_at.to_rfc3339(),
        updated_at: Some(metadata.updated_at.to_rfc3339()),
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// DELETE /api/v1/sessions/{session_id}
pub async fn delete_session(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
) -> Response {
    let Some(handle) = state.services.session_registry.get(&session_id) else {
        return problem_details::not_found("session not found").into_response();
    };

    // Mark session as completed so it won't be recovered
    let _ = handle.set_status(SessionStatus::Completed).await;

    // Remove from registry and cache
    state.services.session_registry.remove(&session_id);
    state
        .chat_session_cache
        .remove_by_session_id(&session_id)
        .await;

    // Delete persisted data
    if let Err(e) = state
        .services
        .session_registry
        .store()
        .delete(&session_id)
        .await
    {
        error!(error = %e, "failed to delete session");
        return problem_details::internal_error("failed to delete session").into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// GET /api/v1/sessions/{session_id}/messages
pub async fn get_messages(
    State(state): State<AppState>,
    PathExtract(session_id): PathExtract<String>,
    Query(query): Query<GetMessagesQuery>,
) -> impl IntoResponse {
    let Some(handle) = state.services.session_registry.get(&session_id) else {
        return problem_details::not_found("session not found").into_response();
    };

    let messages = match handle.get_messages().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to get messages");
            return problem_details::internal_error("failed to get messages").into_response();
        }
    };

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

    // Check if agent has tools configured
    if !ctx.agent_spec.tools.is_empty() {
        // Use agentic loop for tool-using agents
        return send_message_agentic(&state, ctx).await;
    }

    // Simple single-turn for agents without tools
    let chat_response = match ctx.provider.chat(ctx.request).await {
        Ok(resp) => resp,
        Err(e) => {
            error!(error = %e, "llm request failed");
            return problem_details::internal_error("llm request failed").into_response();
        }
    };

    let assistant_content = chat_response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default();

    // Persist assistant message via actor
    if let Err(e) = ctx
        .handle
        .add_assistant_message(assistant_content.clone(), chat_response.usage.clone())
        .await
    {
        return problem_details::internal_error(format!(
            "failed to persist assistant message: {}",
            e
        ))
        .into_response();
    }

    let response = SendMessageResponse {
        message_id: format!("{}{}", crate::api::MESSAGE_ID_PREFIX, Ulid::new()),
        role: "assistant".to_string(),
        content: assistant_content,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Handle send_message for agents with tools using the agentic loop.
async fn send_message_agentic(state: &AppState, ctx: ChatContext) -> Response {
    let session_id = ctx.handle.id().to_string();
    let agent_name = ctx.handle.agent().to_string();

    // Load policy from store (picks up runtime changes from AllowAlways)
    let policy = state.services.policy_store.load(&agent_name).await;

    // Create tools and executor
    let deps = ToolDependencies {
        sandbox: state.services.sandbox.clone(),
        agent_dir: ctx.agent_dir.clone(),
        scheduler: None,
        execution_context: None,
    };
    let executor = build_executor(
        &ctx.agent_spec,
        &agent_name,
        &session_id,
        policy,
        deps,
        &state.services.world_memory_path,
    );

    // Run the agentic loop with SessionHandle
    let result = match run_agentic_loop(
        ctx.provider,
        &executor,
        &ctx.agent_spec,
        ctx.request.messages,
        &ctx.handle,
        ctx.tool_refs.as_ref(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "agentic loop failed");
            return problem_details::internal_error("agentic loop failed").into_response();
        }
    };

    handle_agentic_result(&ctx.handle, result, false).await
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

    let stream = match ctx.provider.chat_stream(ctx.request).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "llm request failed");
            return problem_details::internal_error("llm request failed").into_response();
        }
    };

    let message_id = format!("{}{}", crate::api::MESSAGE_ID_PREFIX, Ulid::new());
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
            handle: ctx.handle,
            session_id,
            message_id,
            idle_timeout: Duration::from_secs(state.idle_timeout_seconds),
            cancel_token,
            on_disconnect: ctx.on_disconnect,
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
    // Verify session exists and get handle
    let Some(handle) = state.services.session_registry.get(&session_id) else {
        return problem_details::not_found("session not found").into_response();
    };

    let agent_name = handle.agent().to_string();

    // Load pending approval from actor (actor serializes access, no external lock needed)
    let pending = match handle.get_pending_approval().await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return problem_details::not_found("no pending approval for this session")
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to load pending approval");
            return problem_details::internal_error("failed to load pending approval")
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

    // Record the approval decision event via actor
    if let Err(e) = handle
        .record_approval_decision(req.call_id.clone(), decision)
        .await
    {
        error!(error = %e, "failed to persist approval decision");
        return problem_details::internal_error("failed to persist approval decision")
            .into_response();
    }

    // Get agent spec for tool execution
    let Some(agent_spec) = state.services.agents.get(&agent_name) else {
        return problem_details::internal_error("session references non-existent agent")
            .into_response();
    };

    // If allow_always, save pattern to policy
    if req.decision == ApprovalDecision::AllowAlways
        && let Err(e) = crate::agent::ToolPolicy::add_pattern_and_save(
            state.services.policy_store.as_ref(),
            &agent_name,
            crate::agent::ToolType::Bash,
            &req.command,
            &state.policy_locks,
        )
        .await
    {
        debug!(
            error = %e,
            command = %req.command,
            "Failed to save allow pattern to policy"
        );
    }

    // Load policy from store (picks up runtime changes from AllowAlways)
    // This is loaded AFTER add_pattern_and_save so it includes any newly saved pattern
    let policy = state.services.policy_store.load(&agent_name).await;

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
        let deps = ToolDependencies {
            sandbox: state.services.sandbox.clone(),
            agent_dir: agent_spec.agent_dir.clone(),
            scheduler: None,
            execution_context: None,
        };
        let executor = build_executor(
            &agent_spec,
            &agent_name,
            &session_id,
            policy.clone(),
            deps,
            &state.services.world_memory_path,
        );

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

    // Clear pending approval via actor
    if let Err(e) = handle.clear_pending_approval().await {
        debug!(error = %e, "Failed to clear pending approval");
    }

    // Get provider for resuming the loop
    let Some(provider) = state
        .services
        .providers
        .get(
            &agent_spec.model.provider,
            agent_spec.model.base_url.as_deref(),
        )
        .await
    else {
        return problem_details::internal_error("provider not configured").into_response();
    };

    // Create executor for resume (uses same policy loaded above)
    let deps = ToolDependencies {
        sandbox: state.services.sandbox.clone(),
        agent_dir: agent_spec.agent_dir.clone(),
        scheduler: None,
        execution_context: None,
    };
    let executor = build_executor(
        &agent_spec,
        &agent_name,
        &session_id,
        policy,
        deps,
        &state.services.world_memory_path,
    );

    // Set session to Running before resuming (accurate status during execution)
    if let Err(e) = handle.set_status(SessionStatus::Running).await {
        debug!(error = %e, "Failed to set session status to Running");
    }

    // Extract tool_refs from agent spec (consistent with run path)
    let tool_refs = ContextBuilder::new()
        .from_agent_spec(&agent_spec)
        .build()
        .tool_refs;

    // Resume the agentic loop with SessionHandle
    let result = match resume_agentic_loop(
        provider,
        &executor,
        &agent_spec,
        pending,
        tool_result,
        &handle,
        tool_refs.as_ref(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Reset to Active on error
            let _ = handle.set_status(SessionStatus::Active).await;
            error!(error = %e, "agentic loop failed");
            return problem_details::internal_error("agentic loop failed").into_response();
        }
    };

    // Handle result using shared helper
    handle_agentic_result(&handle, result, true).await
}

// ============================================================================
// Implementation Details
// ============================================================================

/// Errors that can occur when sending a message.
#[derive(Debug)]
enum SendMessageError {
    SessionNotFound,
    AgentNotFound,
    PersistFailed,
    ProviderNotConfigured,
}

impl IntoResponse for SendMessageError {
    fn into_response(self) -> Response {
        match self {
            Self::SessionNotFound => problem_details::not_found("session not found"),
            Self::AgentNotFound => {
                problem_details::internal_error("session references non-existent agent")
            }
            Self::PersistFailed => {
                problem_details::internal_error("failed to persist session data")
            }
            Self::ProviderNotConfigured => {
                problem_details::internal_error("provider not configured")
            }
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
    handle: SessionHandle,
    tool_refs: Option<std::collections::HashSet<String>>,
}

/// Prepare chat context for LLM request.
///
/// Validates session and agent, adds user message, builds structured context,
/// and returns the ChatRequest with the provider and agent configuration.
async fn prepare_chat_context(
    state: &AppState,
    session_id: &str,
    user_content: String,
) -> Result<ChatContext, SendMessageError> {
    let Some(handle) = state.services.session_registry.get(session_id) else {
        return Err(SendMessageError::SessionNotFound);
    };

    let agent_name = handle.agent().to_string();
    let Some(agent) = state.services.agents.get(&agent_name) else {
        return Err(SendMessageError::AgentNotFound);
    };

    // Persist user message via actor
    if let Err(e) = handle.add_user_message(user_content).await {
        error!(error = %e, "failed to persist user message");
        return Err(SendMessageError::PersistFailed);
    }

    let history = match handle.get_messages().await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "failed to get messages");
            return Err(SendMessageError::PersistFailed);
        }
    };

    let Some(provider) = state
        .services
        .providers
        .get(&agent.model.provider, agent.model.base_url.as_deref())
        .await
    else {
        return Err(SendMessageError::ProviderNotConfigured);
    };

    // Build structured context from agent spec and history
    let directives =
        load_all_directives(&state.services.workspace_directives_path, &agent.agent_dir);
    let structured_context = ContextBuilder::new()
        .from_agent_spec(&agent)
        .with_messages(history)
        .with_directives(directives)
        .build();

    // Capture tool_refs before rendering (for passing to agentic loop)
    let tool_refs = structured_context.tool_refs.clone();

    // Render to ChatRequest with budget (tools handled separately by agentic loop via executor)
    let budget = TokenBudget {
        max_input_tokens: agent.model.effective_max_input_tokens(),
        max_output_tokens: agent.model.max_output_tokens.unwrap_or(4096),
        max_history_tokens: agent.session.context.max_history_tokens,
    };
    let chat_request = structured_context.render_with_budget(
        &agent.model.name,
        agent.model.temperature,
        agent.model.max_output_tokens,
        vec![], // Tools come from ToolExecutor in agentic loop
        &budget,
    );

    let agent_dir = agent.agent_dir.clone();
    let agent_spec = agent.clone();

    Ok(ChatContext {
        request: chat_request,
        provider,
        on_disconnect: agent.session.on_disconnect,
        agent_spec,
        agent_dir,
        handle,
        tool_refs,
    })
}

/// Handle the result from an agentic loop (initial or resume).
///
/// For Complete: persists the message and returns 200 with the response.
/// For AwaitingApproval: saves pending state, sets Paused, returns 202.
async fn handle_agentic_result(
    handle: &SessionHandle,
    result: AgenticResult,
    is_resume: bool,
) -> Response {
    let session_id = handle.id();

    match result {
        AgenticResult::Complete { content, usage, .. } => {
            // Persist the final assistant message via actor
            if let Err(e) = handle.add_assistant_message(content.clone(), usage).await {
                error!(error = %e, "failed to persist assistant message");
                return problem_details::internal_error("failed to persist assistant message")
                    .into_response();
            }

            // On resume, set back to Active (was Running during execution)
            if is_resume {
                let _ = handle.set_status(SessionStatus::Active).await;
            }

            let message_id = format!("{}{}", crate::api::MESSAGE_ID_PREFIX, Ulid::new());

            // Return different response types based on context
            if is_resume {
                // For approve_command, return ApproveCommandResponse
                let response = crate::api::ApproveCommandResponse::Complete {
                    message_id,
                    content,
                };
                (StatusCode::OK, Json(response)).into_response()
            } else {
                // For send_message, return SendMessageResponse
                let response = SendMessageResponse {
                    message_id,
                    role: "assistant".to_string(),
                    content,
                };
                (StatusCode::OK, Json(response)).into_response()
            }
        }

        AgenticResult::AwaitingApproval { pending, .. } => {
            // Save pending approval via actor (immediately persists for crash safety)
            if let Err(e) = handle.set_pending_approval(pending.clone()).await {
                error!(error = %e, "failed to save pending approval");
                return problem_details::internal_error("failed to save pending approval")
                    .into_response();
            }

            // Set session to Paused
            let _ = handle.set_status(SessionStatus::Paused).await;

            // Return different response types based on context
            if is_resume {
                // For approve_command, return ApproveCommandResponse
                let response = crate::api::ApproveCommandResponse::PendingApproval {
                    call_id: pending.call_id,
                    command: pending.command,
                };
                (StatusCode::ACCEPTED, Json(response)).into_response()
            } else {
                // For send_message, return PendingApprovalResponse
                let response = PendingApprovalResponse {
                    session_id: session_id.to_string(),
                    call_id: pending.call_id,
                    command: pending.command,
                };
                (StatusCode::ACCEPTED, Json(response)).into_response()
            }
        }
    }
}
