//! Agent management HTTP handlers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::api::{
    AgentDetailResponse, AgentMetadataResponse, AgentModelResponse, AgentSpecResponse,
    AgentSummary, ListAgentsResponse,
};
use crate::handlers::problem_details;
use crate::server::AppState;

pub async fn list_agents(State(state): State<AppState>) -> Json<ListAgentsResponse> {
    let agents: Vec<AgentSummary> = state
        .services
        .agents
        .snapshot()
        .into_iter()
        .map(|(_, spec)| AgentSummary {
            name: spec.metadata.name.clone(),
            description: spec.metadata.description.clone(),
            version: spec.metadata.version.clone(),
        })
        .collect();

    Json(ListAgentsResponse { agents })
}

pub async fn get_agent(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(agent) = state.services.agents.get(&name) else {
        return problem_details::not_found(format!("agent '{name}' not found")).into_response();
    };

    let response = AgentDetailResponse {
        api_version: agent.api_version.clone(),
        kind: agent.kind.clone(),
        metadata: AgentMetadataResponse {
            name: agent.metadata.name.clone(),
            description: agent.metadata.description.clone(),
            version: agent.metadata.version.clone(),
            labels: agent.metadata.labels.clone(),
        },
        spec: AgentSpecResponse {
            model: AgentModelResponse {
                provider: agent.model.provider.to_string(),
                name: agent.model.name.clone(),
                temperature: agent.model.temperature,
                max_input_tokens: agent.model.max_input_tokens,
                max_output_tokens: agent.model.max_output_tokens,
                base_url: agent.model.base_url.clone(),
            },
            system_prompt: agent.system_prompt.clone(),
            instructions: agent.instructions.clone(),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}
