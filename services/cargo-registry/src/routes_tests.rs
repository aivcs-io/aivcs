//! Route-level contract tests against an in-memory store.

use super::routes::{router, AppState};
use super::store::{Bucket, MemoryStore, Store};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

fn state(store: Arc<MemoryStore>, require_https: bool) -> AppState {
    AppState {
        store,
        token: Some(TOKEN.to_string()),
        base_url: "https://registry.aivcs.io".to_string(),
        require_https,
    }
}

fn basic_header() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("token:{TOKEN}"))
    )
}

async fn get(
    app: axum::Router,
    uri: &str,
    auth: Option<&str>,
    proto: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method("GET").uri(uri);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    if let Some(p) = proto {
        req = req.header("x-forwarded-proto", p);
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn probes_are_open_and_readyz_follows_storage() {
    let store = Arc::new(MemoryStore::new());
    let app = router(state(store.clone(), false));

    let (s, _) = get(app.clone(), "/healthz", None, None).await;
    assert_eq!(s, StatusCode::OK, "healthz must not require a token");
    let (s, _) = get(app.clone(), "/readyz", None, None).await;
    assert_eq!(s, StatusCode::OK);

    // N1: unreachable storage must take the pod out of service, not 401.
    store
        .unavailable
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (s, _) = get(app, "/readyz", None, None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn basic_auth_is_accepted_this_is_the_nix_contract() {
    // The regression that cost the fleet four days: Nix authenticates FOD
    // fetches via netrc, which curl sends as HTTP Basic. Bearer-only 401s every
    // hermetic build even with a valid token.
    let store = Arc::new(MemoryStore::new());
    store
        .put(
            Bucket::Index,
            "da/ta/data-mesh-client",
            b"{\"name\":\"x\"}\n".to_vec(),
        )
        .await
        .unwrap();
    let app = router(state(store, false));

    for (label, hdr) in [
        ("basic", basic_header()),
        ("bearer", format!("Bearer {TOKEN}")),
        ("raw", TOKEN.to_string()),
    ] {
        let (s, _) = get(app.clone(), "/config.json", Some(&hdr), None).await;
        assert_eq!(s, StatusCode::OK, "{label} must authenticate");
    }

    let (s, _) = get(app, "/config.json", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no credential must 401");
}

#[tokio::test]
async fn index_read_forces_trailing_newline() {
    // F5 — cargo rejects the whole index if the last record is not
    // newline-terminated. Objects predating that fix exist.
    let store = Arc::new(MemoryStore::new());
    store
        .put(
            Bucket::Index,
            "da/ta/data-mesh-client",
            b"{\"name\":\"a\"}".to_vec(),
        )
        .await
        .unwrap();
    let app = router(state(store, false));

    let (s, body) = get(app, "/da/ta/data-mesh-client", Some(&basic_header()), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        body.last(),
        Some(&b'\n'),
        "index must be newline-terminated"
    );
}

#[tokio::test]
async fn missing_index_and_crate_are_404_not_500() {
    let store = Arc::new(MemoryStore::new());
    let app = router(state(store, false));

    let (s, _) = get(app.clone(), "/zz/zz/nope", Some(&basic_header()), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = get(
        app,
        "/api/v1/crates/nope/1.0.0/download",
        Some(&basic_header()),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn plaintext_is_redirected_before_the_token_is_examined() {
    // F9 — a netrc entry is per-machine, not per-scheme, so a plaintext request
    // would have carried the token in recoverable base64.
    let store = Arc::new(MemoryStore::new());
    let app = router(state(store, true));

    let (s, _) = get(app.clone(), "/config.json", None, Some("http")).await;
    assert_eq!(
        s,
        StatusCode::MOVED_PERMANENTLY,
        "plaintext must redirect, not 401 — the redirect must win over auth"
    );

    let (s, _) = get(app, "/config.json", Some(&basic_header()), Some("https")).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn publish_is_immutable_and_indexes_the_crate() {
    let store = Arc::new(MemoryStore::new());
    let app = router(state(store.clone(), false));

    // F7 — u32le json len, json, u32le crate len, crate bytes.
    // Carries a crates.io dep and publish-only bookkeeping, so the index record
    // this produces is checked against a realistic payload rather than a stub.
    let meta = br#"{"name":"demo-crate","vers":"0.1.0","description":"ignored","license":"Apache-2.0","deps":[{"name":"serde","version_req":"^1","features":["derive"],"optional":false,"default_features":true,"target":null,"kind":"normal","registry":null}]}"#;
    let krate = b"fake-tarball-bytes";
    let mut body = Vec::new();
    body.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    body.extend_from_slice(meta);
    body.extend_from_slice(&(krate.len() as u32).to_le_bytes());
    body.extend_from_slice(krate);

    let put = |b: Vec<u8>| {
        Request::builder()
            .method("PUT")
            .uri("/api/v1/crates/new")
            .header("authorization", basic_header())
            .body(Body::from(b))
            .unwrap()
    };

    let res = app.clone().oneshot(put(body.clone())).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "first publish succeeds");

    // Downloadable afterwards.
    let (s, got) = get(
        app.clone(),
        "/api/v1/crates/demo-crate/0.1.0/download",
        Some(&basic_header()),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got, krate);

    // Indexed under the sparse prefix layout, newline-terminated.
    let idx = store.get(Bucket::Index, "de/mo/demo-crate").await.unwrap();
    assert_eq!(idx.last(), Some(&b'\n'));

    // The record must be an *index* entry, not the publish payload echoed back.
    // Writing the payload verbatim shipped entries with `version_req` and no
    // `cksum`, which cargo rejects — assert the shape through the HTTP path.
    let entry: serde_json::Value =
        serde_json::from_slice(idx.strip_suffix(b"\n").unwrap()).expect("index line is json");
    assert_eq!(
        entry["cksum"],
        // SHA-256 of b"fake-tarball-bytes".
        serde_json::json!(sha256_hex(krate)),
        "cksum is the digest of the stored artifact"
    );
    assert_eq!(entry["deps"][0]["req"], "^1", "index uses `req`");
    assert!(
        entry["deps"][0].get("version_req").is_none(),
        "publish-only `version_req` must not survive into the index"
    );
    assert_eq!(
        entry["deps"][0]["registry"], "https://github.com/rust-lang/crates.io-index",
        "a null publish registry means crates.io"
    );
    assert_eq!(entry["yanked"], false);
    assert!(
        entry.get("v").is_none(),
        "`v` is omitted so rebuilt records match the valid ones already stored"
    );
    assert!(
        entry.get("description").is_none() && entry.get("license").is_none(),
        "registry bookkeeping must not leak into the index"
    );

    // F6 — republishing the same version must not silently replace bytes;
    // consumers pin a checksum in Cargo.lock against them.
    let res = app.oneshot(put(body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn malformed_publish_is_rejected() {
    let store = Arc::new(MemoryStore::new());
    let app = router(state(store, false));
    for bad in [vec![], vec![1, 2, 3], vec![255, 255, 255, 255, 1]] {
        let req = Request::builder()
            .method("PUT")
            .uri("/api/v1/crates/new")
            .header("authorization", basic_header())
            .body(Body::from(bad))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
}

/// Local digest helper so the expected `cksum` is computed independently of the
/// production path rather than copied from its output.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
