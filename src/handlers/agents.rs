use crate::agent::AgentStore;
use crate::response;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct AgentsResponse {
    agents: Vec<AgentSummary>,
}

#[derive(Serialize)]
pub struct AgentSummary {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Serialize)]
pub struct AgentDetailResponse {
    api_version: String,
    kind: String,
    metadata: MetadataResponse,
    spec: SpecResponse,
}

#[derive(Serialize)]
pub struct MetadataResponse {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    labels: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct SpecResponse {
    model: ModelResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(Serialize)]
pub struct ModelResponse {
    provider: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
}

pub async fn list_agents(State(store): State<AgentStore>) -> Json<AgentsResponse> {
    let agents: Vec<AgentSummary> = store
        .iter()
        .map(|(_, spec)| AgentSummary {
            name: spec.metadata.name.clone(),
            description: spec.metadata.description.clone(),
            version: spec.metadata.version.clone(),
        })
        .collect();

    Json(AgentsResponse { agents })
}

pub async fn get_agent(State(store): State<AgentStore>, Path(name): Path<String>) -> Response {
    let Some(agent) = store.get(&name) else {
        return response::not_found(format!("Agent '{name}' not found")).into_response();
    };

    let response = AgentDetailResponse {
        api_version: agent.api_version.clone(),
        kind: agent.kind.clone(),
        metadata: MetadataResponse {
            name: agent.metadata.name.clone(),
            description: agent.metadata.description.clone(),
            version: agent.metadata.version.clone(),
            labels: agent.metadata.labels.clone(),
        },
        spec: SpecResponse {
            model: ModelResponse {
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
