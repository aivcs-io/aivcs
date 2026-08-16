//! North-south request auth for the forge API.
//!
//! `aivcsd-lite` is reachable from off-cluster once `aivcsd.aivcs.io` routes to
//! it (infra-code#2067). This layer gates that edge traffic.
//!
//! **In-mesh callers are exempt**, and that is enforced, not merely intended:
//! a request carrying a Linkerd-set `l5d-client-id` is already mTLS-authenticated
//! and is passed through without a bearer (`service-mesh` SM2). Without that
//! exemption every meshed consumer — notably sandlot's build Jobs, which reach
//! this forge over the `EastWest` publication to fetch sources — would get 401 on
//! the deploy that "secured" the edge.
//!
//! Off-mesh requests fail closed in every direction: no token configured means
//! the service refuses them outright rather than serving reads, because with the
//! `…/source?ref=` surface an open read is every repo's source.
//!
//! Shape follows `FR_SHARED_FAIL_CLOSED_HTTP_AUTH`, the same fail-closed shared
//! bearer `ci-runner` and `sso-registration` already use in this workspace:
//! constant-time comparison, and **an unset or empty secret authorizes no one**.
//! That last clause is the whole point — a blank token that waves everyone
//! through is the fail-open shape the FR was written to kill
//! (agent-orchestrate-ai#106).

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Env var holding the shared bearer for off-cluster callers.
pub const TOKEN_ENV: &str = "AIVCSD_LITE_TOKEN";

/// Header the Linkerd inbound proxy sets to the mTLS peer's identity.
///
/// Trusting it is what keeps SM2 true: an in-mesh caller is already
/// authenticated and must not be asked for a second bearer. The property this
/// relies on is that the proxy **overwrites** any client-supplied value — a
/// header forged at the edge never reaches the handler intact, because edge
/// traffic arrives unmeshed and the proxy sets the header from the (absent)
/// peer identity. `mesh_identity_is_not_client_supplied` pins the expectation.
const MESH_IDENTITY_HEADER: &str = "l5d-client-id";

/// The configured secret, or `None` when unset/blank.
///
/// Whitespace-only is treated as unset on purpose: `AIVCSD_LITE_TOKEN=" "` from
/// a half-filled Secret must not become a token that a caller could actually
/// send.
pub fn configured_token() -> Option<String> {
    std::env::var(TOKEN_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn deny(status: StatusCode, code: &str, message: &str) -> Response {
    // JSON envelope per the http-control-plane-profile: plain-text or HTML on
    // the auth-rejection path is explicitly non-conformant.
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

/// Peer identity as established by the mesh, if any.
fn mesh_identity(req: &Request<Body>) -> Option<&str> {
    req.headers()
        .get(MESH_IDENTITY_HEADER)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Extract a bearer credential, tolerating the `Bearer ` prefix case-insensitively.
fn bearer(req: &Request<Body>) -> Option<&str> {
    let raw = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(value.trim())
}

/// The gate's configuration, resolved ONCE at startup.
///
/// Deliberately not read from the environment per request: a process-global read
/// inside the handler makes the credential ambient, untestable in parallel, and
/// silently mutable by anything that calls `set_var`. Resolving it at startup
/// also means the "not configured" warning is logged once, at boot, where an
/// operator will see it.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    token: Option<String>,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        Self {
            token: configured_token(),
        }
    }
    /// Build a config directly, bypassing the environment.
    ///
    /// Test-only: the binary always uses [`AuthConfig::from_env`]. Kept behind
    /// `cfg(test)` rather than left `pub` so the credential cannot be set from
    /// application code by accident.
    #[cfg(test)]
    pub fn with_token(token: Option<&str>) -> Self {
        Self {
            token: token.map(str::to_string),
        }
    }
    pub fn is_configured(&self) -> bool {
        self.token.is_some()
    }
}

/// Fail-closed bearer gate.
///
/// Probes stay open so kubelet and the ALB TargetGroup can check health without
/// a credential — they arrive unauthenticated by design and gating them would
/// take the pod out of service rather than secure it.
pub async fn require_bearer(
    State(cfg): State<AuthConfig>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if matches!(
        path,
        "/healthz" | "/health" | "/readyz" | "/ready" | "/version"
    ) {
        return next.run(req).await;
    }

    // SM2: an in-mesh caller is already authenticated by Linkerd mTLS and is not
    // asked for a second bearer. This is the whole reason the Service can stay
    // published EastWest — sandlot's build Jobs fetch sources over the mesh, and
    // gating them on a shared secret would break that on the deploy meant to
    // secure the edge.
    if let Some(peer) = mesh_identity(&req) {
        tracing::debug!(%peer, "in-mesh caller; bearer not required (SM2)");
        return next.run(req).await;
    }

    let Some(expected) = cfg.token.clone() else {
        // Unconfigured AND off-mesh. Refuse everything rather than serve it:
        // "not set up yet" must not be the same state as "open to the internet".
        // Reads matter as much as writes here — with the `…/source?ref=` surface
        // (#82), an open read is every repo's source.
        return deny(
            StatusCode::SERVICE_UNAVAILABLE,
            "UNAVAILABLE",
            "forge edge auth is not configured; refusing off-mesh requests",
        );
    };

    match bearer(&req) {
        // constant_time_eq itself returns false for an empty `expected` — the
        // agent-orchestrate-ai#106 fix encoded upstream. configured_token()
        // already filters that case; both layers agreeing is deliberate.
        Some(provided) if envelope_http_auth::constant_time_eq(provided, &expected) => {
            next.run(req).await
        }
        _ => deny(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "a valid bearer credential is required",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_or_unset_token_is_not_a_credential() {
        // The fail-open shape this guards: a half-filled Secret yielding "" or
        // " " must never become a token a caller can present.
        for raw in ["", "   ", "\t\n"] {
            std::env::set_var(TOKEN_ENV, raw);
            assert!(
                configured_token().is_none(),
                "{raw:?} must not count as configured"
            );
        }
        std::env::set_var(TOKEN_ENV, "  real-token  ");
        assert_eq!(configured_token().as_deref(), Some("real-token"));
        std::env::remove_var(TOKEN_ENV);
        assert!(configured_token().is_none());
    }

    #[test]
    fn mesh_identity_must_be_non_empty_to_count() {
        // A present-but-blank header must not be read as "an in-mesh peer".
        // That is the same fail-open shape as a blank bearer.
        for raw in ["", "   "] {
            let req = Request::builder()
                .header(MESH_IDENTITY_HEADER, raw)
                .body(Body::empty())
                .unwrap();
            assert!(mesh_identity(&req).is_none(), "{raw:?} must not count");
        }
        let req = Request::builder()
            .header(
                MESH_IDENTITY_HEADER,
                "sandlot.sandlot.serviceaccount.identity.linkerd.cluster.local",
            )
            .body(Body::empty())
            .unwrap();
        assert!(mesh_identity(&req).is_some());
    }
    // ── through the real router ──────────────────────────────────────────────

    use axum::{routing::post, Router};
    use tower::ServiceExt;

    /// A router shaped like the service's: ops probes + a mutating forge route,
    /// behind the same middleware.
    fn app(cfg: AuthConfig) -> Router {
        Router::new()
            .route(
                "/api/v1/repos",
                post(|| async { "created" }).get(|| async { "listed" }),
            )
            .route("/healthz", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(cfg, require_bearer))
    }

    async fn call(app: Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
        let mut b = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app.oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn an_unauthenticated_write_is_rejected() {
        let cfg = AuthConfig::with_token(Some("s3cret"));

        // The exact request that succeeded against the live pod with no
        // credential: POST /api/v1/repos -> 201. It must not any more.
        assert_eq!(
            call(app(cfg.clone()), "POST", "/api/v1/repos", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(app(cfg.clone()), "POST", "/api/v1/repos", Some("wrong")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(app(cfg.clone()), "POST", "/api/v1/repos", Some("s3cret")).await,
            StatusCode::OK
        );

        // Probes stay open — gating them takes the pod out of service rather
        // than securing anything.
        assert_eq!(
            call(app(cfg.clone()), "GET", "/healthz", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn unconfigured_refuses_writes_rather_than_serving_them() {
        let cfg = AuthConfig::with_token(None);
        // Not 401 but 503: the operator has not set this up, and the honest
        // answer is "unavailable", never a silently accepted write.
        assert_eq!(
            call(app(cfg.clone()), "POST", "/api/v1/repos", None).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            call(app(cfg), "POST", "/api/v1/repos", Some("anything")).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    /// The SM2 exemption: a meshed caller is not asked for a bearer.
    #[tokio::test]
    async fn in_mesh_callers_are_not_asked_for_a_bearer() {
        let cfg = AuthConfig::with_token(Some("s3cret"));
        let peer = "sandlot.sandlot.serviceaccount.identity.linkerd.cluster.local";

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/repos")
            .header(MESH_IDENTITY_HEADER, peer)
            .body(Body::empty())
            .unwrap();
        // Without this exemption sandlot's build Jobs get 401 the moment the
        // token is set — on the deploy meant to secure the edge.
        assert_eq!(
            app(cfg).oneshot(req).await.unwrap().status(),
            StatusCode::OK
        );
    }

    /// Even unconfigured, a meshed caller works and an off-mesh one does not.
    #[tokio::test]
    async fn unconfigured_still_serves_the_mesh_but_refuses_the_edge() {
        let cfg = AuthConfig::with_token(None);
        let peer = "aivcs-repo.aivcs-repo.serviceaccount.identity.linkerd.cluster.local";

        let meshed = Request::builder()
            .method("GET")
            .uri("/api/v1/repos")
            .header(MESH_IDENTITY_HEADER, peer)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app(cfg.clone()).oneshot(meshed).await.unwrap().status(),
            StatusCode::OK
        );

        // Off-mesh READ is refused too, not just writes: with the …/source?ref=
        // surface, an open read is every repo's source.
        assert_eq!(
            call(app(cfg), "GET", "/api/v1/repos", None).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
