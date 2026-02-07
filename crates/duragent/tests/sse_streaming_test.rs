//! Integration tests for SSE streaming endpoint.
//!
//! Tests the `/api/v1/sessions/{session_id}/stream` endpoint behavior.
//!
//! Note: Full SSE streaming tests with mock LLM providers would require
//! exposing AgentSpec and related types publicly, or adding test helpers
//! to the library. These tests focus on error cases and behavior that
//! can be verified with the existing test infrastructure.

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use duragent::agent::OnDisconnect;
use duragent::llm::{Role, StreamEvent, Usage};
use duragent::server;

mod common;
use common::test_app;

// ============================================================================
// SSE Event Parsing Helper
// ============================================================================

/// Parse SSE events from response body.
fn parse_sse_events(body: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let mut current_event = String::new();
    let mut current_data = String::new();

    for line in body.lines() {
        if let Some(event_name) = line.strip_prefix("event:") {
            current_event = event_name.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            current_data = data.trim().to_string();
        } else if line.is_empty() && !current_event.is_empty() {
            events.push((current_event.clone(), current_data.clone()));
            current_event.clear();
            current_data.clear();
        }
    }

    // Handle last event if no trailing newline
    if !current_event.is_empty() {
        events.push((current_event, current_data));
    }

    events
}

// ============================================================================
// HTTP Error Case Tests
// ============================================================================

/// Test that SSE endpoint returns 404 for non-existent session.
#[tokio::test]
async fn stream_session_not_found() {
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

    assert_eq!(response.status(), 404);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 404);
}

/// Test that SSE endpoint returns problem+json content-type for errors.
#[tokio::test]
async fn stream_session_error_content_type() {
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

    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/problem+json"
    );
}

/// Test that SSE endpoint returns 400 for invalid JSON body.
#[tokio::test]
async fn stream_session_invalid_json() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::post("/api/v1/sessions/some-session/stream")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"invalid json"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // axum returns 400 for JSON parse errors
    assert!(response.status().is_client_error());
}

/// Test that SSE endpoint returns 400 for missing content field.
#[tokio::test]
async fn stream_session_missing_content() {
    let app = test_app().await;

    let response = app
        .oneshot(
            Request::post("/api/v1/sessions/some-session/stream")
                .header("content-type", "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // axum returns 422 for missing required fields
    assert!(response.status().is_client_error());
}

// ============================================================================
// StreamEvent Type Tests
// ============================================================================

/// Test StreamEvent::Token variant.
#[test]
fn stream_event_token_format() {
    let token = StreamEvent::Token("Hello".to_string());
    match token {
        StreamEvent::Token(content) => assert_eq!(content, "Hello"),
        _ => panic!("Expected Token variant"),
    }
}

/// Test StreamEvent::Done with usage statistics.
#[test]
fn stream_event_done_with_usage() {
    let usage = Usage {
        prompt_tokens: 10,
        completion_tokens: 20,
        total_tokens: 30,
    };
    let done = StreamEvent::Done { usage: Some(usage) };
    match done {
        StreamEvent::Done { usage: Some(u) } => {
            assert_eq!(u.prompt_tokens, 10);
            assert_eq!(u.completion_tokens, 20);
            assert_eq!(u.total_tokens, 30);
        }
        _ => panic!("Expected Done variant with usage"),
    }
}

/// Test StreamEvent::Done without usage statistics.
#[test]
fn stream_event_done_without_usage() {
    let done = StreamEvent::Done { usage: None };
    assert!(matches!(done, StreamEvent::Done { usage: None }));
}

/// Test StreamEvent::Cancelled variant.
#[test]
fn stream_event_cancelled() {
    let cancelled = StreamEvent::Cancelled;
    assert!(matches!(cancelled, StreamEvent::Cancelled));
}

// ============================================================================
// SSE Event Parsing Tests
// ============================================================================

#[test]
fn parse_sse_events_start_and_token() {
    let body = "event: start\ndata: {}\n\nevent: token\ndata: {\"content\":\"Hello\"}\n\n";
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "start");
    assert_eq!(events[0].1, "{}");
    assert_eq!(events[1].0, "token");
    assert_eq!(events[1].1, r#"{"content":"Hello"}"#);
}

#[test]
fn parse_sse_events_done_with_message_id() {
    let body = "event: done\ndata: {\"message_id\":\"msg_abc123\",\"usage\":null}\n\n";
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "done");
    assert!(events[0].1.contains("msg_abc123"));
}

#[test]
fn parse_sse_events_error() {
    let body = "event: error\ndata: {\"message\":\"Stream idle timeout\"}\n\n";
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "error");
    assert!(events[0].1.contains("timeout"));
}

#[test]
fn parse_sse_events_cancelled() {
    let body = "event: cancelled\ndata: {}\n\n";
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "cancelled");
    assert_eq!(events[0].1, "{}");
}

#[test]
fn parse_sse_events_multiple() {
    let body = concat!(
        "event: start\ndata: {}\n\n",
        "event: token\ndata: {\"content\":\"Hello\"}\n\n",
        "event: token\ndata: {\"content\":\" world\"}\n\n",
        "event: done\ndata: {\"message_id\":\"msg_123\"}\n\n"
    );
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 4);
    assert_eq!(events[0].0, "start");
    assert_eq!(events[1].0, "token");
    assert_eq!(events[2].0, "token");
    assert_eq!(events[3].0, "done");
}

#[test]
fn parse_sse_events_empty() {
    let body = "";
    let events = parse_sse_events(body);
    assert!(events.is_empty());
}

#[test]
fn parse_sse_events_keep_alive() {
    // Keep-alive comments are ignored
    let body = ": keep-alive\n\nevent: start\ndata: {}\n\n";
    let events = parse_sse_events(body);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "start");
}

// ============================================================================
// OnDisconnect Serialization Tests
// ============================================================================

#[test]
fn on_disconnect_pause_serialization() {
    assert_eq!(
        serde_json::to_string(&OnDisconnect::Pause).unwrap(),
        "\"pause\""
    );
}

#[test]
fn on_disconnect_continue_serialization() {
    assert_eq!(
        serde_json::to_string(&OnDisconnect::Continue).unwrap(),
        "\"continue\""
    );
}

#[test]
fn on_disconnect_pause_deserialization() {
    let pause: OnDisconnect = serde_json::from_str("\"pause\"").unwrap();
    assert_eq!(pause, OnDisconnect::Pause);
}

#[test]
fn on_disconnect_continue_deserialization() {
    let cont: OnDisconnect = serde_json::from_str("\"continue\"").unwrap();
    assert_eq!(cont, OnDisconnect::Continue);
}

// ============================================================================
// Usage Statistics Tests
// ============================================================================

#[test]
fn usage_serialization() {
    let usage = Usage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    };
    let json = serde_json::to_string(&usage).unwrap();

    assert!(json.contains("\"prompt_tokens\":100"));
    assert!(json.contains("\"completion_tokens\":50"));
    assert!(json.contains("\"total_tokens\":150"));
}

#[test]
fn usage_deserialization() {
    let json = r#"{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}"#;
    let usage: Usage = serde_json::from_str(json).unwrap();

    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(usage.total_tokens, 150);
}

// ============================================================================
// Message Role Tests
// ============================================================================

#[test]
fn role_serialization() {
    assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        "\"assistant\""
    );
}

#[test]
fn role_deserialization() {
    assert_eq!(
        serde_json::from_str::<Role>("\"system\"").unwrap(),
        Role::System
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"user\"").unwrap(),
        Role::User
    );
    assert_eq!(
        serde_json::from_str::<Role>("\"assistant\"").unwrap(),
        Role::Assistant
    );
}

// ============================================================================
// AppState Configuration Tests
// ============================================================================

/// Test that AppState can be created with custom timeout values.
#[tokio::test]
async fn app_state_with_custom_timeouts() {
    let mut state = common::test_app_state().await;
    state.idle_timeout_seconds = 120;
    state.keep_alive_interval_seconds = 30;

    let app = server::build_app(state, 600);

    // Verify the app is functional
    let response = app
        .oneshot(Request::get("/livez").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}
