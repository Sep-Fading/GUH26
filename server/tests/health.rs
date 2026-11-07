use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use miniclaw_server::app;
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok() {
    let request = Request::builder()
        .method("GET")
        .uri("/api/health")
        .header(header::HOST, "127.0.0.1:8787")
        .body(Body::empty())
        .expect("valid health request");

    let response = app().oneshot(request).await.expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");

    let bytes = to_bytes(response.into_body(), 4096)
        .await
        .expect("health response within 4 KiB");
    let body: Value =
        serde_json::from_slice(&bytes).expect("valid JSON response");

    assert_eq!(body["backend"], "ok");
    // The page words itself for the machine the backend runs on.
    assert_eq!(body["platform"], std::env::consts::OS);
    assert!(matches!(body["platform"].as_str(), Some("macos" | "windows" | "linux")));
}
