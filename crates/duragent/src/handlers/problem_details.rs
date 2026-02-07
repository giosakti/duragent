//! RFC 7807 Problem Details for HTTP error responses.

use axum::Json;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// URN-style identifiers for RFC 7807 `type`.
pub const TYPE_BAD_REQUEST: &str = "urn:duragent:problem:bad-request";
pub const TYPE_INTERNAL_ERROR: &str = "urn:duragent:problem:internal-error";
pub const TYPE_NOT_FOUND: &str = "urn:duragent:problem:not-found";

/// RFC 7807 Problem Details response
#[derive(Debug, Serialize)]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ProblemDetails {
    #[must_use]
    pub fn new(status: StatusCode, title: impl Into<String>) -> Self {
        Self {
            r#type: "about:blank".to_string(),
            title: title.into(),
            status: status.as_u16(),
            detail: None,
            instance: None,
        }
    }

    #[must_use]
    pub fn with_type(mut self, type_uri: impl Into<String>) -> Self {
        self.r#type = type_uri.into();
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    #[allow(dead_code)] // public API, used in tests
    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        let mut pd = self;
        let status = StatusCode::from_u16(pd.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        pd.status = status.as_u16();

        (
            status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            )],
            Json(pd),
        )
            .into_response()
    }
}

/// Common error responses
#[must_use]
pub fn bad_request(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(StatusCode::BAD_REQUEST, "Bad Request")
        .with_type(TYPE_BAD_REQUEST)
        .with_detail(detail)
}

#[must_use]
pub fn internal_error(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
        .with_type(TYPE_INTERNAL_ERROR)
        .with_detail(detail)
}

#[must_use]
pub fn not_found(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(StatusCode::NOT_FOUND, "Not Found")
        .with_type(TYPE_NOT_FOUND)
        .with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_problem_details_new() {
        let pd = ProblemDetails::new(StatusCode::BAD_REQUEST, "Bad Request");
        assert_eq!(pd.r#type, "about:blank");
        assert_eq!(pd.title, "Bad Request");
        assert_eq!(pd.status, 400);
        assert!(pd.detail.is_none());
        assert!(pd.instance.is_none());
    }

    #[test]
    fn test_problem_details_with_type() {
        let pd = ProblemDetails::new(StatusCode::BAD_REQUEST, "Bad Request")
            .with_type("urn:example:error");
        assert_eq!(pd.r#type, "urn:example:error");
    }

    #[test]
    fn test_problem_details_with_detail() {
        let pd = ProblemDetails::new(StatusCode::BAD_REQUEST, "Bad Request")
            .with_detail("Something went wrong");
        assert_eq!(pd.detail, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_problem_details_with_instance() {
        let pd = ProblemDetails::new(StatusCode::BAD_REQUEST, "Bad Request")
            .with_instance("/api/v1/resource/123");
        assert_eq!(pd.instance, Some("/api/v1/resource/123".to_string()));
    }

    #[test]
    fn test_problem_details_builder_chain() {
        let pd = ProblemDetails::new(StatusCode::NOT_FOUND, "Not Found")
            .with_type(TYPE_NOT_FOUND)
            .with_detail("Resource not found")
            .with_instance("/api/v1/agents/unknown");

        assert_eq!(pd.r#type, TYPE_NOT_FOUND);
        assert_eq!(pd.title, "Not Found");
        assert_eq!(pd.status, 404);
        assert_eq!(pd.detail, Some("Resource not found".to_string()));
        assert_eq!(pd.instance, Some("/api/v1/agents/unknown".to_string()));
    }

    #[test]
    fn test_bad_request_helper() {
        let pd = bad_request("Invalid input");
        assert_eq!(pd.r#type, TYPE_BAD_REQUEST);
        assert_eq!(pd.title, "Bad Request");
        assert_eq!(pd.status, 400);
        assert_eq!(pd.detail, Some("Invalid input".to_string()));
    }

    #[test]
    fn test_internal_error_helper() {
        let pd = internal_error("Database connection failed");
        assert_eq!(pd.r#type, TYPE_INTERNAL_ERROR);
        assert_eq!(pd.title, "Internal Server Error");
        assert_eq!(pd.status, 500);
        assert_eq!(pd.detail, Some("Database connection failed".to_string()));
    }

    #[test]
    fn test_not_found_helper() {
        let pd = not_found("Agent not found");
        assert_eq!(pd.r#type, TYPE_NOT_FOUND);
        assert_eq!(pd.title, "Not Found");
        assert_eq!(pd.status, 404);
        assert_eq!(pd.detail, Some("Agent not found".to_string()));
    }

    #[tokio::test]
    async fn test_problem_details_into_response_contract() {
        use http_body_util::BodyExt;

        let resp = bad_request("Invalid input")
            .with_instance("/api/v1/agents")
            .into_response();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(v["type"], TYPE_BAD_REQUEST);
        assert_eq!(v["title"], "Bad Request");
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], "Invalid input");
        assert_eq!(v["instance"], "/api/v1/agents");
    }
}
