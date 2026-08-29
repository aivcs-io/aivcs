use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use brains_provider_router::{app, ProviderTarget, RouterState};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_healthz_and_metrics_endpoints() {
    let state = Arc::new(RouterState::new(
        "http://127.0.0.1:9001",
        "http://127.0.0.1:9002",
    ));
    let router = app(state);

    // 1. Healthz
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // 2. Metrics
    let resp_metrics = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/router/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_metrics.status(), StatusCode::OK);
}

#[test]
fn test_provider_selection_logic() {
    let state = RouterState::new(
        "http://brains-provider-llmkube.brains.svc:8000",
        "http://brains-provider-dgx.brains.svc:8000",
    );

    // 1. Default -> LLMKube
    let mut headers = header::HeaderMap::new();
    let (target, url) = state.select_provider(&headers, None);
    assert_eq!(target, ProviderTarget::LlmKube);
    assert!(url.contains("llmkube"));

    // 2. Explicit Header -> DGX
    headers.insert("x-brain-provider", "dgx".parse().unwrap());
    let (target_dgx, url_dgx) = state.select_provider(&headers, None);
    assert_eq!(target_dgx, ProviderTarget::Dgx);
    assert!(url_dgx.contains("dgx"));

    // 3. Capability Heuristic: DeepSeek-R1 or 70B model -> DGX
    let empty_headers = header::HeaderMap::new();
    let heavy_payload = json!({
        "model": "deepseek-r1-671b",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let (target_model, _) = state.select_provider(&empty_headers, Some(&heavy_payload));
    assert_eq!(target_model, ProviderTarget::Dgx);

    // 4. Fallback when DGX fails circuit breaker
    state.dgx.record_failure();
    state.dgx.record_failure();
    state.dgx.record_failure(); // Tripped!
    assert!(!state.dgx.is_healthy.load(std::sync::atomic::Ordering::Relaxed));

    // Requesting DGX explicitly should gracefully fall back to LLMKube
    let (fallback_target, _) = state.select_provider(&headers, Some(&heavy_payload));
    assert_eq!(fallback_target, ProviderTarget::LlmKube);
}
