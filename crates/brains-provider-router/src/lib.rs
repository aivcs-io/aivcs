use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use dashmap::DashMap;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, warn};

/// Active backend provider target
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderTarget {
    #[serde(rename = "llmkube")]
    LlmKube,
    #[serde(rename = "dgx")]
    Dgx,
}

impl std::fmt::Display for ProviderTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderTarget::LlmKube => write!(f, "llmkube"),
            ProviderTarget::Dgx => write!(f, "dgx"),
        }
    }
}

/// Provider health and latency profile
#[derive(Debug)]
pub struct ProviderHealth {
    pub name: String,
    pub endpoint_url: String,
    pub is_healthy: AtomicBool,
    pub consecutive_failures: AtomicUsize,
    pub total_requests: AtomicU64,
    pub total_tokens_streamed: AtomicU64,
    pub last_latency_ms: AtomicU64,
}

impl ProviderHealth {
    pub fn new(name: &str, endpoint_url: &str) -> Self {
        Self {
            name: name.to_string(),
            endpoint_url: endpoint_url.to_string(),
            is_healthy: AtomicBool::new(true),
            consecutive_failures: AtomicUsize::new(0),
            total_requests: AtomicU64::new(0),
            total_tokens_streamed: AtomicU64::new(0),
            last_latency_ms: AtomicU64::new(0),
        }
    }

    pub fn record_success(&self, latency_ms: u64, tokens: u64) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.is_healthy.store(true, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_tokens_streamed.fetch_add(tokens, Ordering::Relaxed);
        self.last_latency_ms.store(latency_ms, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        let fails = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if fails >= 3 {
            self.is_healthy.store(false, Ordering::Relaxed);
            warn!(provider = %self.name, failures = fails, "Provider circuit breaker tripped: marked UNHEALTHY");
        }
    }
}

/// Shared application state for Brains Provider Router
#[derive(Clone)]
pub struct RouterState {
    pub client: Client,
    pub llmkube: Arc<ProviderHealth>,
    pub dgx: Arc<ProviderHealth>,
    pub token_rate_history: Arc<DashMap<String, u64>>,
}

impl RouterState {
    pub fn new(llmkube_url: &str, dgx_url: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(64)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            client,
            llmkube: Arc::new(ProviderHealth::new("llmkube", llmkube_url)),
            dgx: Arc::new(ProviderHealth::new("dgx", dgx_url)),
            token_rate_history: Arc::new(DashMap::new()),
        }
    }

    /// Select optimal provider based on explicit header, capability heuristics, and circuit health
    pub fn select_provider(
        &self,
        headers: &HeaderMap,
        body_json: Option<&serde_json::Value>,
    ) -> (ProviderTarget, String) {
        // 1. Explicit Header Override
        if let Some(header_val) = headers.get("x-brain-provider") {
            if let Ok(val_str) = header_val.to_str() {
                if val_str.eq_ignore_ascii_case("dgx") {
                    if self.dgx.is_healthy.load(Ordering::Relaxed) {
                        return (ProviderTarget::Dgx, self.dgx.endpoint_url.clone());
                    } else {
                        warn!("Explicit DGX requested but unhealthy; falling back to LLMKube");
                    }
                } else if val_str.eq_ignore_ascii_case("llmkube") {
                    if self.llmkube.is_healthy.load(Ordering::Relaxed) {
                        return (ProviderTarget::LlmKube, self.llmkube.endpoint_url.clone());
                    } else {
                        warn!("Explicit LLMKube requested but unhealthy; falling back to DGX");
                    }
                }
            }
        }

        // 2. Capability Heuristics from payload
        if let Some(json) = body_json {
            let model = json.get("model").and_then(|m| m.as_str()).unwrap_or("");
            let max_tokens = json.get("max_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            let prompt_len = json.get("messages")
                .and_then(|m| m.as_array())
                .map(|arr| arr.len())
                .unwrap_or(1);

            // Heavy reasoning / long context -> DGX
            let requires_dgx = model.contains("70b")
                || model.contains("deepseek-r1")
                || max_tokens > 4096
                || prompt_len > 20;

            if requires_dgx && self.dgx.is_healthy.load(Ordering::Relaxed) {
                return (ProviderTarget::Dgx, self.dgx.endpoint_url.clone());
            }
        }

        // 3. Default to in-cluster LLMKube if healthy, else DGX
        if self.llmkube.is_healthy.load(Ordering::Relaxed) {
            (ProviderTarget::LlmKube, self.llmkube.endpoint_url.clone())
        } else {
            (ProviderTarget::Dgx, self.dgx.endpoint_url.clone())
        }
    }
}

/// Build the Axum application router
pub fn app(state: Arc<RouterState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/api/v1/router/metrics", get(router_metrics_handler))
        .route("/v1/models", get(models_proxy_handler))
        .route("/v1/chat/completions", post(chat_completions_proxy_handler))
        .route("/v1/completions", post(completions_proxy_handler))
        .route("/v1/embeddings", post(embeddings_proxy_handler))
        .with_state(state)
}

/// Liveness / Readiness probe
async fn healthz_handler(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let llmkube_ok = state.llmkube.is_healthy.load(Ordering::Relaxed);
    let dgx_ok = state.dgx.is_healthy.load(Ordering::Relaxed);

    if !llmkube_ok && !dgx_ok {
        (StatusCode::SERVICE_UNAVAILABLE, "ALL_PROVIDERS_UNHEALTHY")
    } else {
        (StatusCode::OK, "OK")
    }
}

/// Telemetry metrics endpoint
#[derive(Serialize)]
pub struct ProviderMetricView {
    pub name: String,
    pub endpoint: String,
    pub is_healthy: bool,
    pub consecutive_failures: usize,
    pub total_requests: u64,
    pub total_tokens_streamed: u64,
    pub last_latency_ms: u64,
}

#[derive(Serialize)]
pub struct RouterMetricsResponse {
    pub service: &'static str,
    pub active_providers: Vec<ProviderMetricView>,
}

async fn router_metrics_handler(State(state): State<Arc<RouterState>>) -> Json<RouterMetricsResponse> {
    Json(RouterMetricsResponse {
        service: "brains-provider-router",
        active_providers: vec![
            ProviderMetricView {
                name: state.llmkube.name.clone(),
                endpoint: state.llmkube.endpoint_url.clone(),
                is_healthy: state.llmkube.is_healthy.load(Ordering::Relaxed),
                consecutive_failures: state.llmkube.consecutive_failures.load(Ordering::Relaxed),
                total_requests: state.llmkube.total_requests.load(Ordering::Relaxed),
                total_tokens_streamed: state.llmkube.total_tokens_streamed.load(Ordering::Relaxed),
                last_latency_ms: state.llmkube.last_latency_ms.load(Ordering::Relaxed),
            },
            ProviderMetricView {
                name: state.dgx.name.clone(),
                endpoint: state.dgx.endpoint_url.clone(),
                is_healthy: state.dgx.is_healthy.load(Ordering::Relaxed),
                consecutive_failures: state.dgx.consecutive_failures.load(Ordering::Relaxed),
                total_requests: state.dgx.total_requests.load(Ordering::Relaxed),
                total_tokens_streamed: state.dgx.total_tokens_streamed.load(Ordering::Relaxed),
                last_latency_ms: state.dgx.last_latency_ms.load(Ordering::Relaxed),
            },
        ],
    })
}

/// Forward `/v1/models` request
async fn models_proxy_handler(
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let (target, base_url) = state.select_provider(&headers, None);
    let target_url = format!("{}/v1/models", base_url.trim_end_matches('/'));

    let resp = state
        .client
        .get(&target_url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let body = resp.bytes().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert("x-served-by", HeaderValue::from_str(&target.to_string()).unwrap());
    Ok(response)
}

/// Core Forwarder for `/v1/chat/completions` with streaming SSE support
async fn chat_completions_proxy_handler(
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Response, (StatusCode, String)> {
    let start_time = Instant::now();
    let is_streaming = payload.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let (target, base_url) = state.select_provider(&headers, Some(&payload));
    let target_url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));

    let mut req_builder = state.client.post(&target_url).json(&payload);

    // Forward auth headers if present
    if let Some(auth_header) = headers.get(header::AUTHORIZATION) {
        req_builder = req_builder.header(header::AUTHORIZATION, auth_header);
    }
    if let Some(tenant) = headers.get("x-tenant-id") {
        req_builder = req_builder.header("x-tenant-id", tenant);
    }

    let upstream_resp = match req_builder.send().await {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            error!(provider = %target, status = %status, err = %err_text, "Upstream returned error status");
            if target == ProviderTarget::LlmKube {
                state.llmkube.record_failure();
            } else {
                state.dgx.record_failure();
            }
            return Err((StatusCode::BAD_GATEWAY, format!("Upstream {} error: {}", target, err_text)));
        }
        Err(e) => {
            error!(provider = %target, err = %e, "Failed to reach upstream provider");
            if target == ProviderTarget::LlmKube {
                state.llmkube.record_failure();
            } else {
                state.dgx.record_failure();
            }
            return Err((StatusCode::BAD_GATEWAY, format!("Network error reaching {}: {}", target, e)));
        }
    };

    let provider_ref = if target == ProviderTarget::LlmKube {
        state.llmkube.clone()
    } else {
        state.dgx.clone()
    };

    if is_streaming {
        let stream = upstream_resp.bytes_stream().map(move |chunk_res| {
            match chunk_res {
                Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
                Err(err) => Err(std::io::Error::new(std::io::ErrorKind::Other, err)),
            }
        });

        provider_ref.record_success(start_time.elapsed().as_millis() as u64, 1);

        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        response.headers_mut().insert("x-served-by", HeaderValue::from_str(&target.to_string()).unwrap());
        Ok(response)
    } else {
        let body_bytes = upstream_resp.bytes().await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed reading body: {}", e))
        })?;

        let latency_ms = start_time.elapsed().as_millis() as u64;
        provider_ref.record_success(latency_ms, 1);

        let mut response = Response::new(Body::from(body_bytes));
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response.headers_mut().insert("x-served-by", HeaderValue::from_str(&target.to_string()).unwrap());
        response.headers_mut().insert("x-latency-ms", HeaderValue::from_str(&latency_ms.to_string()).unwrap());
        Ok(response)
    }
}

/// Forward `/v1/completions` request
async fn completions_proxy_handler(
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Response, (StatusCode, String)> {
    let (target, base_url) = state.select_provider(&headers, Some(&payload));
    let target_url = format!("{}/v1/completions", base_url.trim_end_matches('/'));

    let resp = state
        .client
        .post(&target_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let body = resp.bytes().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert("x-served-by", HeaderValue::from_str(&target.to_string()).unwrap());
    Ok(response)
}

/// Forward `/v1/embeddings` request
async fn embeddings_proxy_handler(
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Response, (StatusCode, String)> {
    let (target, base_url) = state.select_provider(&headers, Some(&payload));
    let target_url = format!("{}/v1/embeddings", base_url.trim_end_matches('/'));

    let resp = state
        .client
        .post(&target_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)))?;

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let body = resp.bytes().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert("x-served-by", HeaderValue::from_str(&target.to_string()).unwrap());
    Ok(response)
}
