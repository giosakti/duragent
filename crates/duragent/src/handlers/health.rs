use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::server::AppState;

pub async fn livez() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

#[derive(Serialize)]
pub struct ReadyzResponse {
    pub status: String,
    pub workspace_hash: String,
}

pub async fn readyz(State(state): State<AppState>) -> Json<ReadyzResponse> {
    Json(ReadyzResponse {
        status: "ok".to_string(),
        workspace_hash: state.workspace_hash.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_livez() {
        let (status, body) = livez().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}
