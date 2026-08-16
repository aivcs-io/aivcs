//! `aivcs-runs` — the shared CI-run domain model + storage seam for the
//! `www.aivcs.io` backend split (Issue #4).
//!
//! `aivcs-io-api` reads and enqueues runs through the [`RunStore`] trait; the
//! `aivcsd` engine produces and advances them through the same trait, so the two
//! processes share one boundary and no HTTP handler changes when the backing
//! store changes.
//!
//! - **[`InMemoryRunStore`]** — durable for the process lifetime; backs tests and
//!   single-replica dev.
//! - **[`DataMeshRunStore`]** — durable, cross-process, backed by the `data-mesh`
//!   facade (`ci_executions` over data-fabric). Selected in production via
//!   `DATA_MESH_URL`.
//!
//! `RunStore` is `async` and **fallible** (`Result`) so the network-backed
//! implementation fits without special-casing callers.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Error from a [`RunStore`] operation. In-memory stores never produce one;
/// network-backed stores surface backend failures here.
#[derive(Debug, thiserror::Error)]
pub enum RunStoreError {
    /// The backing store (e.g. data-mesh / data-fabric) failed.
    #[error("run store backend error: {0}")]
    Backend(String),
}

/// Result alias for [`RunStore`] operations.
pub type StoreResult<T> = std::result::Result<T, RunStoreError>;

/// Lifecycle state of a CI run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Requested, not yet picked up by the engine.
    Queued,
    /// Claimed by the engine and executing.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed with a failure.
    Failed,
}

/// A single CI run for a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub run_id: String,
    pub owner: String,
    pub repo: String,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
}

/// Storage + lifecycle seam for CI runs.
///
/// `async` (network backing) and fallible (`StoreResult`) so the Phase 1b
/// `data-mesh`-backed implementation fits without changing the contract.
#[async_trait]
pub trait RunStore: Send + Sync {
    /// Enqueue a new run for `owner/repo`, returned in [`RunStatus::Queued`].
    async fn enqueue(&self, owner: &str, repo: &str) -> StoreResult<Run>;

    /// All runs for `owner/repo`, most-recent first (one entry per run).
    async fn list_by_repo(&self, owner: &str, repo: &str) -> StoreResult<Vec<Run>>;

    /// All runs across every repo, most-recent first (one entry per run) — the
    /// flat feed the `www.aivcs.io` Runs tab consumes at `GET /v1/executions`.
    async fn list_all(&self) -> StoreResult<Vec<Run>>;

    /// A single run by id, if it exists.
    async fn get(&self, run_id: &str) -> StoreResult<Option<Run>>;

    /// Claim the oldest [`RunStatus::Queued`] run and move it to
    /// [`RunStatus::Running`] (the `aivcsd` engine seam). Returns `None` when
    /// nothing is queued.
    async fn claim_next_queued(&self) -> StoreResult<Option<Run>>;

    /// Set a run's status; returns `false` if the run id is unknown.
    async fn set_status(&self, run_id: &str, status: RunStatus) -> StoreResult<bool>;
}

fn new_run(owner: &str, repo: &str) -> Run {
    Run {
        run_id: format!("run_{}", Uuid::new_v4()),
        owner: owner.to_string(),
        repo: repo.to_string(),
        status: RunStatus::Queued,
        created_at: Utc::now(),
    }
}

/// In-memory [`RunStore`] — durable only for the process lifetime. Backs tests
/// and single-replica dev; **not** the production store (see
/// [`DataMeshRunStore`]).
#[derive(Default)]
pub struct InMemoryRunStore {
    runs: Mutex<Vec<Run>>,
}

impl InMemoryRunStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn enqueue(&self, owner: &str, repo: &str) -> StoreResult<Run> {
        let run = new_run(owner, repo);
        self.runs.lock().unwrap().push(run.clone());
        Ok(run)
    }

    async fn list_by_repo(&self, owner: &str, repo: &str) -> StoreResult<Vec<Run>> {
        let mut out: Vec<Run> = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.owner == owner && r.repo == repo)
            .cloned()
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(out)
    }

    async fn list_all(&self) -> StoreResult<Vec<Run>> {
        let mut out: Vec<Run> = self.runs.lock().unwrap().clone();
        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(out)
    }

    async fn get(&self, run_id: &str) -> StoreResult<Option<Run>> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.run_id == run_id)
            .cloned())
    }

    async fn claim_next_queued(&self) -> StoreResult<Option<Run>> {
        let mut runs = self.runs.lock().unwrap();
        let idx = runs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status == RunStatus::Queued)
            .min_by(|(_, a), (_, b)| a.created_at.cmp(&b.created_at))
            .map(|(i, _)| i);
        match idx {
            Some(i) => {
                runs[i].status = RunStatus::Running;
                Ok(Some(runs[i].clone()))
            }
            None => Ok(None),
        }
    }

    async fn set_status(&self, run_id: &str, status: RunStatus) -> StoreResult<bool> {
        let mut runs = self.runs.lock().unwrap();
        if let Some(r) = runs.iter_mut().find(|r| r.run_id == run_id) {
            r.status = status;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Durable [`RunStore`] backed by the `data-mesh` facade (`ci_executions`
/// collection over data-fabric).
///
/// The data-mesh client is **append-only** (no in-place update), so runs are
/// **event-sourced**: `enqueue` and every `set_status` append a document for the
/// run; the server returns newest-first, so reads take the newest event per
/// `run_id` as the current state. The original `created_at` is preserved across
/// status events. Each stored document carries a `repo_key` (`"owner/repo"`)
/// field so a repo's runs can be fetched with a single query.
pub struct DataMeshRunStore {
    client: data_mesh::Client,
    collection: String,
}

impl DataMeshRunStore {
    /// The data-mesh collection CI runs live in.
    pub const COLLECTION: &'static str = "ci_executions";

    pub fn new(client: data_mesh::Client) -> Self {
        Self {
            client,
            collection: Self::COLLECTION.to_string(),
        }
    }

    fn repo_key(owner: &str, repo: &str) -> String {
        format!("{owner}/{repo}")
    }

    /// Serialize a run into the stored document, adding the `repo_key` query key.
    fn to_doc(run: &Run) -> serde_json::Value {
        let mut doc = serde_json::to_value(run).expect("Run serializes");
        doc["repo_key"] = serde_json::Value::String(Self::repo_key(&run.owner, &run.repo));
        doc
    }

    async fn append(&self, run: &Run) -> StoreResult<()> {
        self.client
            .create_document(&self.collection, &Self::to_doc(run))
            .await
            .map_err(backend_err)?;
        Ok(())
    }
}

fn backend_err(e: data_mesh::Error) -> RunStoreError {
    RunStoreError::Backend(e.to_string())
}

fn run_from(doc: serde_json::Value) -> Option<Run> {
    data_mesh::from_document::<Run>(doc).ok()
}

#[async_trait]
impl RunStore for DataMeshRunStore {
    async fn enqueue(&self, owner: &str, repo: &str) -> StoreResult<Run> {
        let run = new_run(owner, repo);
        self.append(&run).await?;
        Ok(run)
    }

    async fn list_by_repo(&self, owner: &str, repo: &str) -> StoreResult<Vec<Run>> {
        let key = Self::repo_key(owner, repo);
        let docs = self
            .client
            .query_documents(&self.collection, "repo_key", &key, Some(500))
            .await
            .map_err(backend_err)?;
        // Docs are newest-first; fold to the newest event per run_id.
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for run in docs.into_iter().filter_map(run_from) {
            if seen.insert(run.run_id.clone()) {
                out.push(run);
            }
        }
        Ok(out)
    }

    async fn list_all(&self) -> StoreResult<Vec<Run>> {
        let docs = self
            .client
            .list_documents(&self.collection, Some(1000))
            .await
            .map_err(backend_err)?;
        // Docs newest-first; fold to the newest event per run_id.
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for run in docs.into_iter().filter_map(run_from) {
            if seen.insert(run.run_id.clone()) {
                out.push(run);
            }
        }
        Ok(out)
    }

    async fn get(&self, run_id: &str) -> StoreResult<Option<Run>> {
        // Newest event for this run_id is the current state.
        let docs = self
            .client
            .query_documents(&self.collection, "run_id", run_id, Some(1))
            .await
            .map_err(backend_err)?;
        Ok(docs.into_iter().next().and_then(run_from))
    }

    async fn claim_next_queued(&self) -> StoreResult<Option<Run>> {
        // Queued events, newest-first; reverse for oldest-first (FIFO).
        let docs = self
            .client
            .query_documents(&self.collection, "status", "queued", Some(100))
            .await
            .map_err(backend_err)?;
        let mut candidates: Vec<Run> = docs.into_iter().filter_map(run_from).collect();
        candidates.reverse();

        // Confirm each candidate's *current* latest status is still queued (a
        // later event may have advanced it), then claim it atomically: the
        // Running-event write is **idempotent** under a per-run claim key, so
        // concurrent engines dedup to a single claim. If the write comes back
        // `exists`, another engine already claimed this run — skip it.
        let mut seen = std::collections::HashSet::new();
        for cand in candidates {
            if !seen.insert(cand.run_id.clone()) {
                continue;
            }
            let Some(current) = self.get(&cand.run_id).await? else {
                continue;
            };
            if current.status != RunStatus::Queued {
                continue;
            }
            let running = Run {
                status: RunStatus::Running,
                ..current
            };
            let created = self
                .client
                .create_document_idempotent(
                    &self.collection,
                    &format!("claim:{}", running.run_id),
                    &Self::to_doc(&running),
                )
                .await
                .map_err(backend_err)?;
            if created.status == "exists" {
                // Lost the race to another engine — try the next candidate.
                continue;
            }
            return Ok(Some(running));
        }
        Ok(None)
    }

    async fn set_status(&self, run_id: &str, status: RunStatus) -> StoreResult<bool> {
        // Append a new event carrying the new status, preserving created_at.
        match self.get(run_id).await? {
            Some(current) => {
                let updated = Run { status, ..current };
                self.append(&updated).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enqueue_then_list_returns_the_run() {
        let store = InMemoryRunStore::new();
        let run = store.enqueue("lornu-ai", "aivcs.io").await.unwrap();
        assert_eq!(run.status, RunStatus::Queued);
        let listed = store.list_by_repo("lornu-ai", "aivcs.io").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].run_id, run.run_id);
    }

    #[tokio::test]
    async fn list_is_scoped_to_repo() {
        let store = InMemoryRunStore::new();
        store.enqueue("lornu-ai", "aivcs.io").await.unwrap();
        assert!(store
            .list_by_repo("lornu-ai", "other")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn claim_advances_queued_to_running_then_settles() {
        let store = InMemoryRunStore::new();
        let r = store.enqueue("o", "r").await.unwrap();
        let claimed = store
            .claim_next_queued()
            .await
            .unwrap()
            .expect("a queued run");
        assert_eq!(claimed.run_id, r.run_id);
        assert_eq!(claimed.status, RunStatus::Running);
        assert!(store.claim_next_queued().await.unwrap().is_none());
        assert!(store
            .set_status(&r.run_id, RunStatus::Succeeded)
            .await
            .unwrap());
        assert_eq!(
            store.get(&r.run_id).await.unwrap().unwrap().status,
            RunStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn set_status_on_unknown_run_is_false() {
        let store = InMemoryRunStore::new();
        assert!(!store.set_status("nope", RunStatus::Failed).await.unwrap());
    }
}
