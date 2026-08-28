//! `service-kit` — the shared bootstrap every `apps-middle-ware` Rust service
//! builds on, so services re-implement as little as possible (mirrors the role
//! of `@web-apps/build` in the frontend monorepo).
//!
//! Phase 0 provides the operational surface (`/health`, `/version`), env-driven
//! config, and tracing init. The offering facades (`data-mesh`, `secrets`,
//! `identity`) land as sibling crates as services need them — each wired to its
//! standard-offering client, never to a source directly.

use axum::{routing::get, Json, Router};
use serde_json::json;

/// The standard operational endpoints every service exposes:
/// `GET /health` and its alias `GET /healthz` (liveness/readiness probe
/// target), plus `GET /version`.
///
/// Both `/health` and `/healthz` are served because the fleet's edge probes
/// (ALB/Gateway health checks and the public `*.aivcs.io/healthz` convention
/// used by www/splash/design/future) hit `/healthz`, while the in-cluster k8s
/// probes historically hit `/health`. Serving both keeps every service
/// answerable on whichever path a checker uses — `aivcs-io-api` 404'd on
/// `api.aivcs.io/healthz` because it exposed only `/health`.
///
/// Compose service routes onto this so probes and version reporting are
/// identical across the fleet: `ops_router(name, ver).merge(my_routes)`.
pub fn ops_router(service_name: &'static str, version: &'static str) -> Router {
    // Non-capturing, so `Copy` — the same handler is registered on both the
    // canonical `/health` and the `/healthz` alias.
    let health = || async { Json(json!({ "status": "healthy" })) };
    Router::new()
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route(
            "/version",
            get(
                move || async move { Json(json!({ "service": service_name, "version": version })) },
            ),
        )
}

/// Bind port from `PORT`, defaulting to `8080` (the fleet convention).
pub fn port_from_env() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080)
}

/// Initialize tracing once, honoring `RUST_LOG` (defaults to `info`). Safe to
/// call from `main`; a no-op if a subscriber is already installed.
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt; // oneshot

    async fn body_json(res: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let res = ops_router("svc", "0.0.0")
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["status"], "healthy");
    }

    #[tokio::test]
    async fn healthz_alias_returns_ok() {
        // The public `*.aivcs.io/healthz` convention must resolve, not 404.
        let res = ops_router("svc", "0.0.0")
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["status"], "healthy");
    }

    #[tokio::test]
    async fn version_reports_service_and_version() {
        let res = ops_router("hello", "1.2.3")
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["service"], "hello");
        assert_eq!(v["version"], "1.2.3");
    }
}
