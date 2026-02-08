//! HTTP client library for duragent server.
//!
//! Provides `AgentClient` for interacting with an duragent server over HTTP.
//! Used by CLI commands to communicate with local or remote servers.

mod error;
mod stream;

pub use crate::api::{
    AgentDetailResponse, AgentMetadataResponse, AgentModelResponse, AgentSpecResponse,
    AgentSummary, ApprovalDecision, ApproveCommandRequest, ApproveCommandResponse,
    CreateSessionRequest, GetMessagesResponse, GetSessionResponse, ListAgentsResponse,
    ListSessionsResponse, MessageResponse, SendMessageRequest, SendMessageResponse, SessionStatus,
    SessionSummary,
};
pub use error::{ClientError, Result};
pub use stream::ClientStreamEvent;

use reqwest::Client;
use serde::Deserialize;

/// Response from the /readyz health check endpoint.
#[derive(Debug, Deserialize)]
pub struct ReadyzResponse {
    pub status: String,
    #[serde(default)]
    pub workspace_hash: String,
}

/// HTTP client for duragent server.
#[derive(Debug, Clone)]
pub struct AgentClient {
    base_url: String,
    http: Client,
}

impl AgentClient {
    /// Create a new client pointing to the given base URL.
    ///
    /// Example: `AgentClient::new("http://localhost:8080")`
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: Client::new(),
        }
    }

    /// Check if the server is healthy.
    ///
    /// Calls GET /readyz and returns the readyz response with workspace hash.
    pub async fn health(&self) -> Result<ReadyzResponse> {
        let url = format!("{}/readyz", self.base_url);
        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(ClientError::ServerUnhealthy {
                status: response.status().as_u16(),
            });
        }

        Ok(response.json().await?)
    }

    // ----------------------------------------------------------------------------
    // Agents
    // ----------------------------------------------------------------------------

    /// List all available agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentSummary>> {
        let url = format!("{}/api/v1/agents", self.base_url);
        let response = self.http.get(&url).send().await?;

        if response.status().is_success() {
            let body: ListAgentsResponse = response.json().await?;
            Ok(body.agents)
        } else {
            Err(self.parse_error(response).await)
        }
    }

    /// Get details of a specific agent.
    pub async fn get_agent(&self, name: &str) -> Result<AgentDetailResponse> {
        let url = format!("{}/api/v1/agents/{}", self.base_url, name);
        let response = self.http.get(&url).send().await?;
        self.json_response(response).await
    }

    // ----------------------------------------------------------------------------
    // Sessions
    // ----------------------------------------------------------------------------

    /// List all sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let response = self.http.get(&url).send().await?;

        if response.status().is_success() {
            let body: ListSessionsResponse = response.json().await?;
            Ok(body.sessions)
        } else {
            Err(self.parse_error(response).await)
        }
    }

    /// Create a new session for an agent.
    pub async fn create_session(&self, agent: &str) -> Result<GetSessionResponse> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let body = CreateSessionRequest {
            agent: agent.to_string(),
        };

        let response = self.http.post(&url).json(&body).send().await?;
        self.json_response(response).await
    }

    /// Get details of a specific session.
    pub async fn get_session(&self, session_id: &str) -> Result<GetSessionResponse> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let response = self.http.get(&url).send().await?;
        self.json_response(response).await
    }

    /// Get messages for a session.
    pub async fn get_messages(
        &self,
        session_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<MessageResponse>> {
        let mut url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        if let Some(limit) = limit {
            url.push_str(&format!("?limit={}", limit));
        }

        let response = self.http.get(&url).send().await?;

        if response.status().is_success() {
            let body: GetMessagesResponse = response.json().await?;
            Ok(body.messages)
        } else {
            Err(self.parse_error(response).await)
        }
    }

    /// Send a message and get a response (non-streaming).
    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<SendMessageResponse> {
        let url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        let body = SendMessageRequest {
            content: content.to_string(),
        };

        let response = self.http.post(&url).json(&body).send().await?;
        self.json_response(response).await
    }

    /// Send a message and stream the response via SSE.
    pub async fn stream_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<impl futures::Stream<Item = Result<ClientStreamEvent>>> {
        let url = format!("{}/api/v1/sessions/{}/stream", self.base_url, session_id);
        let body = SendMessageRequest {
            content: content.to_string(),
        };

        let response = self.http.post(&url).json(&body).send().await?;

        if response.status().is_success() {
            Ok(stream::into_event_stream(response))
        } else {
            Err(self.parse_error(response).await)
        }
    }

    /// Approve or deny a pending tool execution.
    pub async fn approve_command(
        &self,
        session_id: &str,
        call_id: &str,
        command: &str,
        decision: ApprovalDecision,
    ) -> Result<ApproveCommandResponse> {
        let url = format!("{}/api/v1/sessions/{}/approve", self.base_url, session_id);
        let body = ApproveCommandRequest {
            call_id: call_id.to_string(),
            command: command.to_string(),
            decision,
        };

        let response = self.http.post(&url).json(&body).send().await?;
        self.json_response(response).await
    }

    // ----------------------------------------------------------------------------
    // Admin
    // ----------------------------------------------------------------------------

    /// Request server shutdown.
    ///
    /// Calls POST /api/admin/v1/shutdown to trigger graceful server shutdown.
    pub async fn shutdown(&self) -> Result<()> {
        let url = format!("{}/api/admin/v1/shutdown", self.base_url);
        let response = self.http.post(&url).send().await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(self.parse_error(response).await)
        }
    }

    // ----------------------------------------------------------------------------
    // Helpers
    // ----------------------------------------------------------------------------

    /// Parse an error response into a ClientError.
    async fn parse_error(&self, response: reqwest::Response) -> ClientError {
        let status = response.status().as_u16();

        // Try to parse as problem+json
        if let Ok(problem) = response.json::<ProblemDetails>().await {
            ClientError::ApiError {
                status,
                message: problem.detail.unwrap_or(problem.title),
            }
        } else {
            ClientError::ApiError {
                status,
                message: format!("HTTP {}", status),
            }
        }
    }

    /// Parse a successful JSON response or convert error response.
    async fn json_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        if response.status().is_success() {
            Ok(response.json().await?)
        } else {
            Err(self.parse_error(response).await)
        }
    }
}

/// RFC 7807 Problem Details response.
#[derive(Deserialize)]
struct ProblemDetails {
    title: String,
    detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_new_trims_trailing_slash() {
        let client = AgentClient::new("http://localhost:8080/");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn client_new_preserves_url_without_slash() {
        let client = AgentClient::new("http://localhost:8080");
        assert_eq!(client.base_url, "http://localhost:8080");
    }
}
