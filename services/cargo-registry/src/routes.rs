//! HTTP surface for the sparse Cargo registry.
//!
//! Route parity with the retired Cloudflare Worker
//! (`infra-code crossplane/cloudflare/.../sparse-registry-worker.ts`), so
//! consumers see no change across the cutover.

use axum::{
    body::Bytes,
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, put},
    Router,
};
use std::sync::Arc;

use crate::auth::authorized;
use crate::index::{ensure_trailing_newline, index_entry, index_key, is_index_path, PublishMeta};
use crate::store::{Bucket, Store, StoreError};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    /// `None` disables the auth gate. Production always sets it.
    pub token: Option<String>,
    /// Public base URL, used to build `dl`/`api` in `config.json`.
    pub base_url: String,
    /// Reject plaintext by redirecting to https (F9). Off in tests.
    pub require_https: bool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/config.json", get(config_json))
        // axum 0.7 path params are `:name`, not `{name}` (that is 0.8). With the
        // brace form this route never matches and download requests fall through
        // to the index fallback as a 404.
        .route("/api/v1/crates/:name/:version/download", get(download))
        .route("/api/v1/crates/new", put(publish))
        .fallback(index)
        .with_state(state)
}

// ── probes (never auth-gated: kubelet and the ALB have no token) ─────────────

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn readyz(State(st): State<AppState>) -> impl IntoResponse {
    // N1: a registry that cannot reach its bucket must not take traffic — it is
    // on the critical path of every Rust build in the fleet.
    if st.store.healthy().await {
        (StatusCode::OK, "ready")
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "object storage unreachable",
        )
    }
}

// ── guards ───────────────────────────────────────────────────────────────────

/// F9 — the registry is HTTPS-only.
///
/// TLS terminates at Cloudflare and again at the ALB, so the pod always sees
/// plain HTTP; `X-Forwarded-Proto` is the only signal of what the client used.
/// A netrc entry is per-machine, not per-scheme, so a client that reaches us
/// over `http://` would have sent the token in recoverable base64 — redirect
/// instead of serving.
fn https_redirect(headers: &HeaderMap, uri: &Uri, state: &AppState) -> Option<Response> {
    if !state.require_https {
        return None;
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    if proto.eq_ignore_ascii_case("https") {
        return None;
    }
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let location = format!("{}{}", state.base_url.trim_end_matches('/'), path);
    Some(
        (
            StatusCode::MOVED_PERMANENTLY,
            [(header::LOCATION, location)],
        )
            .into_response(),
    )
}

fn auth_reject(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    let Some(token) = state.token.as_deref() else {
        return None; // gate disabled
    };
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if authorized(provided, token) {
        None
    } else {
        Some((StatusCode::UNAUTHORIZED, "Unauthorized").into_response())
    }
}

/// Both guards, in order: transport first, then credentials. Redirecting before
/// authenticating means a plaintext request never has its token examined.
fn guard(headers: &HeaderMap, uri: &Uri, state: &AppState) -> Option<Response> {
    https_redirect(headers, uri, state).or_else(|| auth_reject(headers, state))
}

// ── routes ───────────────────────────────────────────────────────────────────

async fn config_json(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Some(r) = guard(&headers, &uri, &st) {
        return r;
    }
    let base = st.base_url.trim_end_matches('/');
    let body = serde_json::json!({
        "dl": format!("{base}/api/v1/crates"),
        "api": base,
        "auth-required": st.token.is_some(),
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

async fn download(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, version)): Path<(String, String)>,
) -> Response {
    if let Some(r) = guard(&headers, &uri, &st) {
        return r;
    }
    let key = crate_key(&name, &version);
    match st.store.get(Bucket::Crates, &key).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(StoreError::NotFound) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(e) => storage_error(e),
    }
}

async fn index(State(st): State<AppState>, req: Request) -> Response {
    let headers = req.headers().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();

    if !is_index_path(&path) {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    if let Some(r) = guard(&headers, &uri, &st) {
        return r;
    }

    // The request path IS the object key for the sparse index.
    let key = path.trim_start_matches('/');
    match st.store.get(Bucket::Index, key).await {
        Ok(body) => {
            // F5 — cargo rejects the whole index if any line, including the
            // last, is not newline-terminated. Objects written before this was
            // enforced may lack it, so normalise rather than trust.
            let body = ensure_trailing_newline(body);
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                body,
            )
                .into_response()
        }
        Err(StoreError::NotFound) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(e) => storage_error(e),
    }
}

async fn publish(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(r) = guard(&headers, &uri, &st) {
        return r;
    }

    // F7 — preserve the wire framing exactly: u32le json len, json, u32le
    // crate len, crate bytes. Existing publish tooling depends on it.
    let Some((meta, crate_bytes)) = parse_publish(&body) else {
        return (StatusCode::BAD_REQUEST, "malformed publish payload").into_response();
    };

    let key = crate_key(&meta.name, &meta.vers);

    // F6 — published artifacts are immutable. Consumers pin a checksum in
    // Cargo.lock, so silently replacing bytes under a version would break every
    // lockfile that pinned the old hash.
    match st.store.exists(Bucket::Crates, &key).await {
        Ok(true) => {
            return (
                StatusCode::CONFLICT,
                format!("{} {} already published", meta.name, meta.vers),
            )
                .into_response()
        }
        Ok(false) => {}
        Err(e) => return storage_error(e),
    }

    // Built before the upload moves the bytes: `cksum` is the SHA-256 of the
    // exact artifact being stored.
    let entry = index_entry(&meta, &crate_bytes, Some(&st.base_url));

    if let Err(e) = st.store.put(Bucket::Crates, &key, crate_bytes).await {
        return storage_error(e);
    }

    // Append the index record, keeping the ndjson invariant.
    //
    // The publish payload is NOT an index record — it spells the requirement
    // `version_req` (the index reads `req`), carries no `cksum`, and omits the
    // crates.io URL on external deps. Writing it verbatim produced entries
    // cargo cannot resolve, so build the record explicitly.
    let idx_key = index_key(&meta.name);
    let mut existing = match st.store.get(Bucket::Index, &idx_key).await {
        Ok(b) => b,
        Err(StoreError::NotFound) => Vec::new(),
        Err(e) => return storage_error(e),
    };
    if !existing.is_empty() && existing.last() != Some(&b'\n') {
        existing.push(b'\n');
    }
    match serde_json::to_vec(&entry) {
        Ok(line) => existing.extend_from_slice(&line),
        Err(e) => {
            tracing::error!(error = %e, crate_name = %meta.name, "serialising index entry");
            return (StatusCode::INTERNAL_SERVER_ERROR, "index encode failed").into_response();
        }
    }
    existing.push(b'\n');
    if let Err(e) = st.store.put(Bucket::Index, &idx_key, existing).await {
        return storage_error(e);
    }

    (
        StatusCode::OK,
        axum::Json(serde_json::json!({"warnings": {}})),
    )
        .into_response()
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn crate_key(name: &str, version: &str) -> String {
    format!("{}/{}", name.to_ascii_lowercase(), version)
}

fn json_len(body: &[u8]) -> Option<usize> {
    if body.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize)
}

fn parse_publish(body: &[u8]) -> Option<(PublishMeta, Vec<u8>)> {
    let jlen = json_len(body)?;
    let json_end = 4usize.checked_add(jlen)?;
    if body.len() < json_end + 4 {
        return None;
    }
    let meta: PublishMeta = serde_json::from_slice(&body[4..json_end]).ok()?;
    let clen = u32::from_le_bytes([
        body[json_end],
        body[json_end + 1],
        body[json_end + 2],
        body[json_end + 3],
    ]) as usize;
    let crate_start = json_end + 4;
    let crate_end = crate_start.checked_add(clen)?;
    if body.len() < crate_end {
        return None;
    }
    Some((meta, body[crate_start..crate_end].to_vec()))
}

fn storage_error(e: StoreError) -> Response {
    tracing::error!(error = %e, "object storage error");
    (StatusCode::BAD_GATEWAY, "object storage error").into_response()
}
