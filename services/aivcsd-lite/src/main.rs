//! `aivcsd-lite` — the lean repository daemon.
//!
//! It is the forge half of `aivcsd` with the CI-execution engine, run store,
//! executor, and poll loop removed: just the repository forge (CAS / commit /
//! branch, plus `POST /api/v1/repos` to init an empty repo), persisted
//! **mesh-first** to the data-mesh. "aivcsd is the thing you need for repos" —
//! this is only that, so the image carries none of the CI-engine overhead.
//!
//! It reuses the shared `forge-cas` and `data-mesh` crates verbatim — no forge
//! logic is duplicated; the difference from `aivcsd` is purely what is *left
//! out* (no `aivcs-runs`, no engine loop).

mod auth;

use std::net::SocketAddr;
use std::sync::Arc;

/// Forge persistence, **fail-closed**: a durable store is required. A forge
/// exists to be durable — silently degrading to in-memory would mint `aivcs://`
/// URIs for repos that vanish on restart while `/readyz` still says ready (the
/// same fail-open shape removed from aivcs-repo).
///
/// Two durable backends, selected by env (checked in priority order):
/// 1. `DATABASE_URL` → `SqlxForgeStore` (PostgreSQL). Postgres only; a SQLite or
///    other DSN is refused at startup (never-use-sqlite). This is the backend for
///    the aivcs-attic-cache scope, backed by its `db.t4g.micro` Postgres.
/// 2. `DATA_MESH_URL` → `DataMeshForgeStore` (the mesh-first default).
///
/// With neither set, startup refuses unless in-memory is *explicitly* opted into
/// for local dev (`AIVCSD_LITE_IN_MEMORY=true`).
///
/// **Both set is a hard error.** The two backends are independent durable stores
/// with no migration and no dual-read between them, so silently preferring one
/// would hide every repo written to the other while `/readyz` stays green — the
/// same fail-open shape this function exists to prevent, merely displaced from
/// "in-memory" to "the wrong durable store". Refuse and make the operator choose.
async fn build_forge() -> anyhow::Result<forge_cas::SharedForge> {
    if std::env::var_os("DATABASE_URL").is_some() && std::env::var_os("DATA_MESH_URL").is_some() {
        anyhow::bail!(
            "both DATABASE_URL (postgres) and DATA_MESH_URL are set — refusing to start. \
             They select DIFFERENT durable forge stores with no migration or dual-read; \
             starting on one would silently hide every repo written to the other. \
             Set exactly one."
        );
    }
    if let Ok(dsn) = std::env::var("DATABASE_URL") {
        let store = forge_cas::SqlxForgeStore::connect(&dsn)
            .await
            .map_err(|e| anyhow::anyhow!("postgres forge bind failed: {e}"))?;
        tracing::info!("forge store: postgres/sqlx (durable)");
        return Ok(Arc::new(store));
    }
    if std::env::var("DATA_MESH_URL").is_ok() {
        let client =
            data_mesh::from_env().map_err(|e| anyhow::anyhow!("data-mesh bind failed: {e}"))?;
        tracing::info!("forge store: data-mesh (durable)");
        return Ok(Arc::new(forge_cas::DataMeshForgeStore::new(client)));
    }
    if std::env::var("AIVCSD_LITE_IN_MEMORY").as_deref() == Ok("true") {
        tracing::warn!("AIVCSD_LITE_IN_MEMORY=true — in-memory forge (NON-DURABLE, dev only)");
        return Ok(forge_cas::new_forge());
    }
    anyhow::bail!(
        "a durable forge backend is required: set DATABASE_URL (postgres) or \
         DATA_MESH_URL. Set AIVCSD_LITE_IN_MEMORY=true only for local dev."
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service_kit::init_tracing();
    let forge = build_forge().await?;

    // Ops probes (/health, /readyz) + the forge HTTP API. No background engine.
    // North-south gate. In-mesh callers are already authenticated by Linkerd and
    // are not asked for a second bearer (SM2) — this is for off-cluster traffic
    // arriving via aivcsd.aivcs.io, which has no mesh identity.
    let auth_cfg = auth::AuthConfig::from_env(Some(forge.clone()));
    if !auth_cfg.is_configured() {
        tracing::warn!(
            env = auth::TOKEN_ENV,
            "no forge write credential configured — writes will be refused with 503"
        );
    }
    let app = service_kit::ops_router("aivcsd-lite", env!("CARGO_PKG_VERSION"))
        .merge(forge_cas::router_with(forge))
        .layer(axum::middleware::from_fn_with_state(
            auth_cfg,
            auth::require_bearer,
        ));

    let addr = SocketAddr::from(([0, 0, 0, 0], service_kit::port_from_env()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "aivcsd-lite forge API active (no CI engine)");
    axum::serve(listener, app).await?;
    Ok(())
}
