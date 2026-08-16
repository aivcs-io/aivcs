//! `aivcsd` — the CI execution engine for `www.aivcs.io` (Issue #4). It is the
//! **producer** side of the `aivcs-runs` [`RunStore`] seam: it claims queued
//! runs, runs them through a [`CiExecutor`], and advances their status.
//! `aivcs-io-api` reads that same store.

use std::net::SocketAddr;
use std::sync::Arc;

use aivcs_runs::{DataMeshRunStore, InMemoryRunStore, RunStore};

use crate::executor::PlaceholderExecutor;

mod executor {
    use aivcs_runs::{Run, RunStatus};
    use async_trait::async_trait;

    /// Terminal outcome of executing a run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExecOutcome {
        Succeeded,
        /// A real executor produces this on failure; `PlaceholderExecutor` never
        /// does, so it is only constructed once a real runner is wired (and in
        /// tests).
        #[allow(dead_code)]
        Failed,
    }

    impl ExecOutcome {
        /// The [`RunStatus`] this outcome settles the run to.
        pub fn status(self) -> RunStatus {
            match self {
                ExecOutcome::Succeeded => RunStatus::Succeeded,
                ExecOutcome::Failed => RunStatus::Failed,
            }
        }
    }

    /// Runs a claimed CI run and reports its outcome. The real CI runner
    /// (container / job dispatch, log capture) implements this; the engine loop
    /// is unaware of how execution happens.
    #[async_trait]
    pub trait CiExecutor: Send + Sync {
        async fn execute(&self, run: &Run) -> ExecOutcome;
    }

    /// Placeholder executor — reports success without running anything. It is the
    /// default until a real runner is wired; swapping it in changes nothing else.
    pub struct PlaceholderExecutor;

    #[async_trait]
    impl CiExecutor for PlaceholderExecutor {
        async fn execute(&self, _run: &Run) -> ExecOutcome {
            ExecOutcome::Succeeded
        }
    }
}

mod engine {
    use aivcs_runs::{RunStore, StoreResult};

    use crate::executor::CiExecutor;

    /// Claim one queued run, execute it, and settle its status from the
    /// executor's outcome. Returns the claimed run id, or `None` when nothing is
    /// queued.
    pub async fn advance_once(
        store: &dyn RunStore,
        executor: &dyn CiExecutor,
    ) -> StoreResult<Option<String>> {
        match store.claim_next_queued().await? {
            Some(run) => {
                let outcome = executor.execute(&run).await;
                store.set_status(&run.run_id, outcome.status()).await?;
                Ok(Some(run.run_id))
            }
            None => Ok(None),
        }
    }
}

/// Select the run store from the environment — durable `data-mesh` store when
/// `DATA_MESH_URL` is set, else the non-durable in-memory store (dev). Mirrors
/// `aivcs-io-api` so both sides bind the same backing.
fn build_store() -> anyhow::Result<Arc<dyn RunStore>> {
    if std::env::var("DATA_MESH_URL").is_ok() {
        let client =
            data_mesh::from_env().map_err(|e| anyhow::anyhow!("data-mesh bind failed: {e}"))?;
        tracing::info!("run store: data-mesh (durable)");
        Ok(Arc::new(DataMeshRunStore::new(client)))
    } else {
        tracing::warn!("DATA_MESH_URL unset — using in-memory run store (non-durable)");
        Ok(Arc::new(InMemoryRunStore::new()))
    }
}

/// Select the forge persistence from the same explicit production contract as
/// the run store. There is no deployment-specific fallback: unset means a
/// deliberately non-durable development process.
fn build_forge() -> anyhow::Result<forge_cas::SharedForge> {
    if std::env::var("DATA_MESH_URL").is_ok() {
        let client =
            data_mesh::from_env().map_err(|e| anyhow::anyhow!("data-mesh bind failed: {e}"))?;
        tracing::info!("forge store: data-mesh (durable)");
        Ok(Arc::new(forge_cas::DataMeshForgeStore::new(client)))
    } else {
        tracing::warn!("DATA_MESH_URL unset — using in-memory forge (non-durable)");
        Ok(forge_cas::new_forge())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    service_kit::init_tracing();
    let store = build_store()?;
    let forge = build_forge()?;

    // Background: the CI execution engine loop — claim a queued run, execute it,
    // settle its status; sleep when the queue is empty.
    tokio::spawn(async move {
        let executor = PlaceholderExecutor;
        loop {
            match engine::advance_once(store.as_ref(), &executor).await {
                Ok(Some(run_id)) => tracing::info!(%run_id, "run completed"),
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                Err(e) => {
                    tracing::error!(error = %e, "engine step failed");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });

    // Foreground: the forge HTTP API (CAS/commit/branch/manifest) + ops probes.
    // Serving `/health` is what makes the ALB/gateway see a healthy target and
    // route `aivcsd.aivcs.io` to this pod (fixes the 503/404 no-backend state).
    let app = service_kit::ops_router("aivcsd", env!("CARGO_PKG_VERSION"))
        .merge(forge_cas::router_with(forge));
    let addr = SocketAddr::from(([0, 0, 0, 0], service_kit::port_from_env()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "aivcsd forge API + engine active");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivcs_runs::{Run, RunStatus};
    use async_trait::async_trait;
    use executor::{CiExecutor, ExecOutcome};

    /// An executor that always fails — proves the outcome flows to the store.
    struct FailingExecutor;

    #[async_trait]
    impl CiExecutor for FailingExecutor {
        async fn execute(&self, _run: &Run) -> ExecOutcome {
            ExecOutcome::Failed
        }
    }

    #[tokio::test]
    async fn advance_claims_and_settles_success() {
        let store = InMemoryRunStore::new();
        let run = store.enqueue("o", "r").await.unwrap();

        let done = engine::advance_once(&store, &PlaceholderExecutor)
            .await
            .unwrap();
        assert_eq!(done.as_deref(), Some(run.run_id.as_str()));
        assert_eq!(
            store.get(&run.run_id).await.unwrap().unwrap().status,
            RunStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn advance_settles_failure_from_executor() {
        let store = InMemoryRunStore::new();
        let run = store.enqueue("o", "r").await.unwrap();

        engine::advance_once(&store, &FailingExecutor)
            .await
            .unwrap();
        assert_eq!(
            store.get(&run.run_id).await.unwrap().unwrap().status,
            RunStatus::Failed
        );
    }

    #[tokio::test]
    async fn advance_is_noop_when_nothing_queued() {
        let store = InMemoryRunStore::new();
        assert!(engine::advance_once(&store, &PlaceholderExecutor)
            .await
            .unwrap()
            .is_none());
    }
}
