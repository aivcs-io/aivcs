//! Sovereign Cargo sparse registry — `registry.aivcs.io`.
//!
//! Rust + OCI on EKS, replacing the Cloudflare Worker. Object storage is AWS S3
//! (us-east-2), not R2, so no part of the registry depends on Cloudflare compute
//! or storage. Cloudflare stays in front for DNS, TLS, WAF and cache only.
//!
//! Why this exists rather than a patched Worker: the Worker's HTTP Basic support
//! merged 2026-08-03 and was never deployed, because Worker delivery sits outside
//! Flux, the OCI rail and `deploy_gate` — nothing reported the gap, and every
//! hermetic Nix build in the fleet 401'd for four days. Here, merge means
//! deployed, like every other service.
//!
//! Design: code-governance `docs/architecture/TDD_RETIRE_CLOUDFLARE_WORKERS.md`.

mod auth;
mod index;
mod repair;
mod routes;
mod store;

#[cfg(test)]
#[path = "routes_tests.rs"]
mod routes_tests;

use anyhow::Context;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let index_bucket =
        std::env::var("REGISTRY_INDEX_BUCKET").context("REGISTRY_INDEX_BUCKET is required")?;
    let crates_bucket =
        std::env::var("REGISTRY_CRATES_BUCKET").context("REGISTRY_CRATES_BUCKET is required")?;
    let base_url = std::env::var("REGISTRY_BASE_URL")
        .unwrap_or_else(|_| "https://registry.aivcs.io".to_string());

    // Fail closed. An unset token would serve every private crate to anyone who
    // finds the origin, so require an explicit opt-out instead of defaulting to
    // open — the same posture as the Worker's MCP_AUTH_REQUIRED.
    let auth_disabled = std::env::var("REGISTRY_AUTH_DISABLED").as_deref() == Ok("true");
    let token = match std::env::var("REGISTRY_AUTH_TOKEN") {
        Ok(t) if !t.is_empty() => Some(t),
        _ if auth_disabled => {
            tracing::warn!("REGISTRY_AUTH_DISABLED=true — serving unauthenticated");
            None
        }
        _ => anyhow::bail!(
            "REGISTRY_AUTH_TOKEN is required (set REGISTRY_AUTH_DISABLED=true to run open)"
        ),
    };

    let cfg = aws_config::load_from_env().await;
    let s3 = aws_sdk_s3::Client::new(&cfg);
    let store = Arc::new(store::S3Store::new(s3, index_bucket, crates_bucket));

    // `regenerate-index` rebuilds malformed index records from the published
    // artifacts. Repair cannot go through publish — versions are immutable and
    // a re-publish is a 409 — so it is a maintenance command, not a route: it
    // must never be reachable over HTTP.
    //
    // Dry by default; `--apply` is required to write.
    if std::env::args().nth(1).as_deref() == Some("regenerate-index") {
        let apply = std::env::args().any(|a| a == "--apply");
        let report = repair::run(store.as_ref(), &base_url, apply).await?;
        tracing::info!(
            apply,
            records = report.records,
            already_valid = report.already_valid,
            repaired = report.repaired.len(),
            failed = report.failed.len(),
            "index regeneration complete"
        );
        for id in &report.repaired {
            tracing::info!(crate_version = %id, applied = apply, "record rebuilt");
        }
        for (id, why) in &report.failed {
            tracing::error!(crate_version = %id, reason = %why, "record left unrepaired");
        }
        // A partial repair is a failed repair: exit non-zero so an operator
        // cannot read a clean exit as "the index is now valid".
        if !report.failed.is_empty() {
            anyhow::bail!("{} record(s) could not be rebuilt", report.failed.len());
        }
        return Ok(());
    }

    let state = routes::AppState {
        store,
        token,
        base_url,
        // F9 — plaintext gets a 301, never a served response. Basic auth carries
        // the token as recoverable base64, so TLS is load-bearing here.
        require_https: std::env::var("REGISTRY_REQUIRE_HTTPS").as_deref() != Ok("false"),
    };

    let app = routes::router(state);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "cargo-registry listening");
    axum::serve(listener, app).await?;
    Ok(())
}
