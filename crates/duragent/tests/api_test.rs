//! Integration tests for the HTTP API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

mod common;

use common::test_app;

// ============================================================================
// Health Endpoints
// ============================================================================

#[tokio::test]
async fn test_livez() {
    let app = test_app().await;

    let response = app
        .oneshot(Request::get("/livez").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn test_readyz() {
    let app = test_app().await;

    let response = app
        .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn test_version() {
    let app = test_app().await;

    let response = app
        .oneshot(Request::get("/version").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json.get("version").is_some());
}

// ============================================================================
// Agents API
// ============================================================================

#[tokio::test]
async fn test_list_agents_empty() {
    let app = test_app().await;

    let response = app
        .oneshot(Request::get("/api/v1/agents").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["agents"], serde_json::json!([]));
}

#[tokio::test]
async fn test_get_agent_not_found() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::get("/api/v1/agents/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], 404);
    assert!(json["detail"].as_str().unwrap().contains("not found"));
}

// ============================================================================
// Sessions API
// ============================================================================

#[tokio::test]
async fn test_create_session_agent_not_found() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::post("/api/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"agent": "nonexistent"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], 404);
    assert!(json["detail"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_get_session_not_found() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::get("/api/v1/sessions/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], 404);
}

#[tokio::test]
async fn test_stream_session_not_found() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::post("/api/v1/sessions/nonexistent/stream")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content": "hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], 404);
}

// ============================================================================
// Error Responses
// ============================================================================

#[tokio::test]
async fn test_problem_details_format() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::get("/api/v1/agents/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // RFC 7807 required fields
    assert!(json.get("type").is_some());
    assert!(json.get("title").is_some());
    assert!(json.get("status").is_some());
}
