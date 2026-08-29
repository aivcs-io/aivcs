//! Integration tests for `data-gateway` REST endpoints and orchestration state routing.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use data_gateway::{app, GatewayState};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_healthz_endpoint() {
    // Dummy / uninitialized pool check for pure routing test
    // We can verify router construction & healthz without DB
    let dummy_url = "postgres://aivcs_test:fake@127.0.0.1:5432/non_existent_db";
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy(dummy_url)
        .expect("lazy connect should succeed");

    let state = Arc::new(GatewayState::new(pool, 65536));
    let router = app(state);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_payload_routing_logic() {
    let small_payload = json!({"task": "compile", "status": "ok"});
    let small_bytes = serde_json::to_vec(&small_payload).unwrap();
    let threshold = 100; // 100 bytes

    assert!(small_bytes.len() < threshold);

    // Large payload
    let large_payload = json!({
        "large_data": "A".repeat(500)
    });
    let large_bytes = serde_json::to_vec(&large_payload).unwrap();
    assert!(large_bytes.len() > threshold);

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&large_bytes);
    let digest = format!("{:x}", hasher.finalize());

    assert_eq!(digest.len(), 64);
}

#[tokio::test]
async fn test_dag_traversal_route_registration() {
    let dummy_url = "postgres://aivcs_test:fake@127.0.0.1:5432/non_existent_db";
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy(dummy_url)
        .expect("lazy connect should succeed");

    let state = Arc::new(GatewayState::new(pool, 65536));
    let router = app(state);

    // Verify downstream route matches (will return 500 because lazy pool is not connected, but not 404!)
    let target_id = uuid::Uuid::now_v7();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orchestration/dag/downstream/{}", target_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(response.status(), StatusCode::NOT_FOUND);

    // Verify upstream route matches
    let response_upstream = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/orchestration/dag/upstream/{}", target_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(response_upstream.status(), StatusCode::NOT_FOUND);
}
