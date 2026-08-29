//! Data-mesh-backed state handle for AIVCS.
//!
//! The governed state layer uses `data-mesh-client` when configured and falls
//! back to an in-process store for local execution and tests. Direct database
//! drivers are intentionally not used here.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::Utc;
use data_mesh::Client as MeshClient;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::ci::{CiPipelineSpec, CiRunRecord, CiSnapshot};
use crate::error::{StateError, StorageError};
use crate::schema::{
    AgentRecord, CommitId, CommitRecord, DecisionRecord, EdgeType, GraphEdge,
    MemoryProvenanceRecord, MemoryRecord, SnapshotRecord,
};
use crate::storage_traits::{ContentDigest, ReleaseMetadata, ReleaseRecord, StorageResult};
use crate::Result;

const COLLECTION_COMMITS: &str = "aivcs_commits";
const COLLECTION_SNAPSHOTS: &str = "aivcs_snapshots";
const COLLECTION_AGENTS: &str = "aivcs_agents";
const COLLECTION_MEMORIES: &str = "aivcs_memories";
const COLLECTION_EDGES: &str = "aivcs_graph_edges";
const COLLECTION_RELEASES: &str = "aivcs_releases";
const COLLECTION_CI_SNAPSHOTS: &str = "aivcs_ci_snapshots";
const COLLECTION_CI_PIPELINES: &str = "aivcs_ci_pipelines";
const COLLECTION_CI_RUNS: &str = "aivcs_ci_runs";
const COLLECTION_DECISIONS: &str = "aivcs_decisions";
const COLLECTION_PROVENANCE: &str = "aivcs_memory_provenances";

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// Flat, queryable field injected into every mesh document so a record can be
/// fetched back by its domain key via `query_documents`. The mesh assigns its
/// own opaque document id (unrelated to the idempotency key), so reads cannot
/// use `get_document(collection, key)`; they query on this field instead.
const MESH_KEY_FIELD: &str = "_aivcs_key";

/// Inject [`MESH_KEY_FIELD`] into a serialized document so it is retrievable by
/// `key`. Non-object values (which our records never are) are returned as-is.
fn with_mesh_key(mut value: Value, key: &str) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert(MESH_KEY_FIELD.to_string(), Value::String(key.to_string()));
    }
    value
}

/// Decode a mesh document into `T`, tolerating either a raw payload or an
/// `{ "data": <payload> }` envelope. The injected [`MESH_KEY_FIELD`] is ignored
/// on the way back in (records don't deny unknown fields).
fn doc_into<T: DeserializeOwned>(doc: Value) -> Option<T> {
    if let Ok(value) = serde_json::from_value::<T>(doc.clone()) {
        return Some(value);
    }
    doc.get("data")
        .cloned()
        .and_then(|inner| serde_json::from_value::<T>(inner).ok())
}

/// A `create_document_idempotent` response with `status == "exists"` means the
/// row was already present (a duplicate write), not a fresh insert.
fn created_is_new(status: &str) -> bool {
    status != "exists"
}

/// Deprecated legacy config placeholder.
#[derive(Debug, Clone)]
pub struct CloudConfig {
    pub endpoint: String,
    pub username: String,
    pub password: String,
    pub namespace: String,
    pub database: String,
    pub is_root: bool,
}

impl CloudConfig {
    pub fn new(
        endpoint: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            username: username.into(),
            password: password.into(),
            namespace: "aivcs".to_string(),
            database: "main".to_string(),
            is_root: false,
        }
    }

    pub fn new_root(
        endpoint: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            username: username.into(),
            password: password.into(),
            namespace: "aivcs".to_string(),
            database: "main".to_string(),
            is_root: true,
        }
    }

    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    pub fn with_database(mut self, db: impl Into<String>) -> Self {
        self.database = db.into();
        self
    }

    pub fn with_root(mut self, is_root: bool) -> Self {
        self.is_root = is_root;
        self
    }

    pub fn from_env() -> std::result::Result<Self, String> {
        Err(
            "legacy direct DB access removed — configure DATA_FABRIC_URL and DATA_MESH_TENANT_ID"
                .into(),
        )
    }
}

#[derive(Default)]
struct MemoryStore {
    commits: HashMap<String, CommitRecord>,
    snapshots: HashMap<String, SnapshotRecord>,
    agents: HashMap<String, AgentRecord>,
    memories: HashMap<String, MemoryRecord>,
    edges: Vec<GraphEdge>,
    releases: HashMap<String, Vec<ReleaseRecord>>,
    ci_snapshots: HashMap<String, CiSnapshot>,
    ci_pipelines: HashMap<String, CiPipelineSpec>,
    ci_runs: HashMap<String, CiRunRecord>,
    decisions: HashMap<String, DecisionRecord>,
    provenances: Vec<MemoryProvenanceRecord>,
}

/// AIVCS persistence handle.
///
/// Cheap to clone: clones share the same in-process store (`Arc<Mutex<…>>`) and
/// mesh client, matching the previous handle's `Clone` semantics that
/// downstream crates (e.g. `aivcs-cli`) rely on.
#[derive(Clone)]
pub struct SurrealHandle {
    store: Arc<Mutex<MemoryStore>>,
    mesh: Option<MeshClient>,
}

impl SurrealHandle {
    fn new(mesh: Option<MeshClient>) -> Self {
        Self {
            store: Arc::new(Mutex::new(MemoryStore::default())),
            mesh,
        }
    }

    /// Idempotently persist `doc` under `key`, returning whether a *new* row was
    /// created (`false` means it already existed). Callers with a uniqueness
    /// contract turn `false` into an "already exists" error.
    async fn mesh_create<T: Serialize>(
        &self,
        mesh: &MeshClient,
        collection: &str,
        key: &str,
        doc: &T,
    ) -> Result<bool> {
        let value = with_mesh_key(serde_json::to_value(doc)?, key);
        let created = mesh
            .create_document_idempotent(collection, key, &value)
            .await
            .map_err(|error| StateError::Query(error.to_string()))?;
        Ok(created_is_new(&created.status))
    }

    /// Fetch a single record by its domain `key`.
    async fn mesh_get<T: DeserializeOwned>(
        &self,
        mesh: &MeshClient,
        collection: &str,
        key: &str,
    ) -> Result<Option<T>> {
        let docs = mesh
            .query_documents(collection, MESH_KEY_FIELD, key, Some(1))
            .await
            .map_err(|error| StateError::Query(error.to_string()))?;
        Ok(docs.into_iter().next().and_then(doc_into))
    }

    /// Fetch every record in `collection` (for filtered/list reads that filter
    /// and sort in memory). `limit` bounds the server page; `None` uses the
    /// server default.
    async fn mesh_all<T: DeserializeOwned>(
        &self,
        mesh: &MeshClient,
        collection: &str,
        limit: Option<u32>,
    ) -> Result<Vec<T>> {
        let docs = mesh
            .list_documents(collection, limit)
            .await
            .map_err(|error| StateError::Query(error.to_string()))?;
        Ok(docs.into_iter().filter_map(doc_into).collect())
    }

    /// Best-effort durability write for the mutable/deletable records that the
    /// create/query/list-only mesh facade cannot yet fully represent (memories,
    /// decisions). The in-process store stays authoritative for these;
    /// this persists a copy so a durable-read path can be added once the facade
    /// grows update/delete (tracked as a follow-up). No-op when the mesh is
    /// unconfigured.
    async fn mesh_write_through<T: Serialize>(
        &self,
        collection: &str,
        key: &str,
        doc: &T,
    ) -> Result<()> {
        if let Some(mesh) = &self.mesh {
            self.mesh_create(mesh, collection, key, doc).await?;
        }
        Ok(())
    }

    async fn init_schema(&self) -> Result<()> {
        debug!("Initializing AIVCS in-process schema");
        Ok(())
    }

    /// Connect to the in-process store.
    #[instrument(skip_all)]
    pub async fn setup_db() -> Result<Self> {
        info!("AIVCS state: in-process store");
        let handle = Self::new(None);
        handle.init_schema().await?;
        Ok(handle)
    }

    /// Legacy direct DB entrypoint.
    #[instrument(skip(_config))]
    pub async fn setup_cloud(_config: CloudConfig) -> Result<Self> {
        Err(StateError::Connection(
            "legacy direct DB access removed — configure DATA_FABRIC_URL and DATA_MESH_TENANT_ID"
                .into(),
        ))
    }

    /// Connect using environment variables.
    #[instrument(skip_all)]
    pub async fn setup_from_env() -> Result<Self> {
        match MeshClient::from_env() {
            Ok(mesh) => {
                info!(
                    base_url = %mesh.base_url(),
                    tenant = %mesh.tenant_id(),
                    "AIVCS state: data-mesh write-through enabled"
                );
                let handle = Self::new(Some(mesh));
                handle.init_schema().await?;
                Ok(handle)
            }
            Err(error) => {
                debug!(%error, "data-mesh not configured; using in-process store");
                Self::setup_db().await
            }
        }
    }

    // ========== Commit Operations ==========

    #[instrument(skip(self, record), fields(commit_id = %record.commit_id.hash))]
    pub async fn save_commit(&self, record: &CommitRecord) -> Result<CommitRecord> {
        let mut stored = record.clone();
        let key = stored.commit_id.hash.clone();
        stored.id.get_or_insert_with(|| format!("commit:{key}"));

        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_COMMITS, &key, &stored)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "Commit already exists: {key}"
                )));
            }
            return Ok(stored);
        }

        let mut store = self.store.lock().await;
        if store.commits.contains_key(&key) {
            return Err(StateError::Transaction(format!(
                "Commit already exists: {key}"
            )));
        }
        store.commits.insert(key.clone(), stored.clone());
        Ok(stored)
    }

    #[instrument(skip(self, memories, commit, snapshot))]
    pub async fn save_merge_commit(
        &self,
        memories: &[MemoryRecord],
        commit: &CommitRecord,
        parent_a: &str,
        parent_b: &str,
        snapshot: &SnapshotRecord,
    ) -> Result<()> {
        debug!(
            "Saving merge commit {} with {} memories",
            commit.commit_id.short(),
            memories.len()
        );

        for memory in memories {
            self.save_memory(memory).await?;
        }

        self.save_commit(commit).await?;
        self.save_commit_graph_edge_typed(&commit.commit_id.hash, parent_a, EdgeType::Merge)
            .await?;
        self.save_commit_graph_edge_typed(&commit.commit_id.hash, parent_b, EdgeType::Merge)
            .await?;
        self.save_snapshot(&commit.commit_id, snapshot.state.clone())
            .await?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn get_commit(&self, commit_hash: &str) -> Result<Option<CommitRecord>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_get(mesh, COLLECTION_COMMITS, commit_hash).await;
        }
        Ok(self.store.lock().await.commits.get(commit_hash).cloned())
    }

    // ========== Snapshot Operations ==========

    #[instrument(skip(self, commit_id, state))]
    pub async fn save_snapshot(
        &self,
        commit_id: &CommitId,
        state: serde_json::Value,
    ) -> Result<()> {
        let mut stored = SnapshotRecord::new(&commit_id.hash, state);
        let key = commit_id.hash.clone();
        stored.id.get_or_insert_with(|| format!("snapshot:{key}"));

        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_SNAPSHOTS, &key, &stored)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "Snapshot already exists for commit {key}"
                )));
            }
        } else {
            let mut store = self.store.lock().await;
            if store.snapshots.contains_key(&key) {
                return Err(StateError::Transaction(format!(
                    "Snapshot already exists for commit {key}"
                )));
            }
            store.snapshots.insert(key.clone(), stored.clone());
        }

        info!(
            "Snapshot saved: {} ({} bytes)",
            commit_id.short(),
            stored.size_bytes
        );
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn load_snapshot(&self, commit_id: &str) -> Result<SnapshotRecord> {
        if let Some(mesh) = &self.mesh {
            return self
                .mesh_get(mesh, COLLECTION_SNAPSHOTS, commit_id)
                .await?
                .ok_or_else(|| StateError::CommitNotFound(commit_id.to_string()));
        }
        self.store
            .lock()
            .await
            .snapshots
            .get(commit_id)
            .cloned()
            .ok_or_else(|| StateError::CommitNotFound(commit_id.to_string()))
    }

    // ========== Graph Edge Operations ==========

    #[instrument(skip(self))]
    pub async fn save_commit_graph_edge(&self, child_id: &str, parent_id: &str) -> Result<()> {
        self.save_commit_graph_edge_typed(child_id, parent_id, EdgeType::Normal)
            .await
    }

    #[instrument(skip(self))]
    pub async fn save_commit_graph_edge_typed(
        &self,
        child_id: &str,
        parent_id: &str,
        edge_type: EdgeType,
    ) -> Result<()> {
        debug!(
            "Saving graph edge: {} -> {} ({:?})",
            parent_id, child_id, edge_type
        );

        let edge = match edge_type {
            EdgeType::Normal => GraphEdge::new(child_id, parent_id),
            EdgeType::Merge => GraphEdge::merge(child_id, parent_id),
            EdgeType::Fork => GraphEdge {
                child_id: child_id.to_string(),
                parent_id: parent_id.to_string(),
                edge_type: EdgeType::Fork,
                created_at: Utc::now(),
            },
        };

        // Stable key (not the enum's Debug repr) so it survives EdgeType changes.
        let edge_kind = match edge.edge_type {
            EdgeType::Normal => "normal",
            EdgeType::Merge => "merge",
            EdgeType::Fork => "fork",
        };
        let key = format!("{child_id}:{parent_id}:{edge_kind}");

        if let Some(mesh) = &self.mesh {
            self.mesh_create(mesh, COLLECTION_EDGES, &key, &edge)
                .await?;
        } else {
            self.store.lock().await.edges.push(edge.clone());
        }

        info!("Graph edge saved: {} -> {}", parent_id, child_id);
        Ok(())
    }

    /// All graph edges (mesh-backed reads filter these in memory).
    async fn all_edges(&self) -> Result<Vec<GraphEdge>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_all(mesh, COLLECTION_EDGES, None).await;
        }
        Ok(self.store.lock().await.edges.clone())
    }

    #[instrument(skip(self))]
    pub async fn get_parent(&self, child_id: &str) -> Result<Option<String>> {
        Ok(self
            .all_edges()
            .await?
            .into_iter()
            .find(|edge| edge.child_id == child_id)
            .map(|edge| edge.parent_id))
    }

    #[instrument(skip(self))]
    pub async fn get_parents(&self, child_id: &str) -> Result<Vec<String>> {
        Ok(self
            .all_edges()
            .await?
            .into_iter()
            .filter(|edge| edge.child_id == child_id)
            .map(|edge| edge.parent_id)
            .collect())
    }

    #[instrument(skip(self))]
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<String>> {
        Ok(self
            .all_edges()
            .await?
            .into_iter()
            .filter(|edge| edge.parent_id == parent_id)
            .map(|edge| edge.child_id)
            .collect())
    }

    // ========== Agent Operations ==========

    #[instrument(skip(self, record), fields(agent_name = %record.name))]
    pub async fn register_agent(&self, record: &AgentRecord) -> Result<AgentRecord> {
        let mut stored = record.clone();
        let key = stored.agent_id.to_string();
        stored.id.get_or_insert_with(|| format!("agent:{key}"));

        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_AGENTS, &key, &stored)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "Agent already exists: {key}"
                )));
            }
            return Ok(stored);
        }

        let mut store = self.store.lock().await;
        if store.agents.contains_key(&key) {
            return Err(StateError::Transaction(format!(
                "Agent already exists: {key}"
            )));
        }
        store.agents.insert(key.clone(), stored.clone());
        Ok(stored)
    }

    #[instrument(skip(self))]
    pub async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_get(mesh, COLLECTION_AGENTS, agent_id).await;
        }
        Ok(self.store.lock().await.agents.get(agent_id).cloned())
    }

    // ========== Memory Operations ==========

    #[instrument(skip(self, record), fields(key = %record.key))]
    pub async fn save_memory(&self, record: &MemoryRecord) -> Result<MemoryRecord> {
        let mut stored = record.clone();
        let key = format!("{}:{}", stored.commit_id, stored.key);
        stored.id.get_or_insert_with(|| format!("memory:{key}"));

        self.store
            .lock()
            .await
            .memories
            .insert(key.clone(), stored.clone());
        self.mesh_write_through(COLLECTION_MEMORIES, &key, &stored)
            .await?;
        Ok(stored)
    }

    #[instrument(skip(self))]
    pub async fn get_memories(&self, commit_id: &str) -> Result<Vec<MemoryRecord>> {
        let mut memories: Vec<_> = self
            .store
            .lock()
            .await
            .memories
            .values()
            .filter(|memory| memory.commit_id == commit_id)
            .cloned()
            .collect();
        memories.sort_by_key(|memory| memory.created_at);
        Ok(memories)
    }

    #[instrument(skip(self))]
    pub async fn delete_memory(&self, memory_key: &str) -> Result<bool> {
        let mut store = self.store.lock().await;
        let before = store.memories.len();
        store.memories.retain(|_, memory| memory.key != memory_key);
        Ok(before != store.memories.len())
    }

    #[instrument(skip(self))]
    pub async fn delete_memory_by_id(&self, memory_id: &str) -> Result<bool> {
        let stripped = memory_id.strip_prefix("memory:").unwrap_or(memory_id);
        let mut store = self.store.lock().await;
        if store.memories.remove(memory_id).is_some() || store.memories.remove(stripped).is_some() {
            return Ok(true);
        }
        let key_to_remove = store.memories.iter().find_map(|(k, v)| {
            if v.id.as_deref() == Some(memory_id) || v.id.as_deref() == Some(stripped) {
                Some(k.clone())
            } else {
                None
            }
        });
        if let Some(k) = key_to_remove {
            store.memories.remove(&k);
            return Ok(true);
        }
        Ok(false)
    }

    // ========== Release Registry Operations ==========

    #[instrument(skip(self, spec_digest, metadata), fields(name = %name, digest = %spec_digest))]
    pub async fn release_promote(
        &self,
        name: &str,
        spec_digest: &ContentDigest,
        metadata: ReleaseMetadata,
    ) -> StorageResult<ReleaseRecord> {
        let record = ReleaseRecord {
            name: name.to_string(),
            spec_digest: spec_digest.clone(),
            metadata,
            created_at: Utc::now(),
        };

        if let Some(mesh) = &self.mesh {
            // Releases are an append-only history: each promote (and rollback,
            // which re-promotes) adds a row. A key unique per event appends
            // rather than dedups, so the full history is preserved.
            let key = format!(
                "{name}:{}:{}",
                record.spec_digest,
                record.created_at.to_rfc3339()
            );
            self.mesh_create(mesh, COLLECTION_RELEASES, &key, &record)
                .await
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            return Ok(record);
        }

        self.store
            .lock()
            .await
            .releases
            .entry(name.to_string())
            .or_default()
            .push(record.clone());
        Ok(record)
    }

    #[instrument(skip(self), fields(name = %name))]
    pub async fn release_rollback(&self, name: &str) -> StorageResult<ReleaseRecord> {
        let history = self.release_history(name).await?;
        if history.is_empty() {
            return Err(StorageError::ReleaseNotFound {
                name: name.to_string(),
            });
        }
        if history.len() < 2 {
            return Err(StorageError::NoPreviousRelease {
                name: name.to_string(),
            });
        }

        let previous = &history[1];
        self.release_promote(name, &previous.spec_digest, previous.metadata.clone())
            .await
    }

    #[instrument(skip(self), fields(name = %name))]
    pub async fn release_current(&self, name: &str) -> StorageResult<Option<ReleaseRecord>> {
        Ok(self.release_history(name).await?.into_iter().next())
    }

    #[instrument(skip(self), fields(name = %name))]
    pub async fn release_history(&self, name: &str) -> StorageResult<Vec<ReleaseRecord>> {
        let mut history = if let Some(mesh) = &self.mesh {
            let mut all: Vec<ReleaseRecord> = self
                .mesh_all(mesh, COLLECTION_RELEASES, None)
                .await
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            all.retain(|record| record.name == name);
            all
        } else {
            self.store
                .lock()
                .await
                .releases
                .get(name)
                .cloned()
                .unwrap_or_default()
        };
        history.sort_by_key(|record| std::cmp::Reverse(record.created_at));
        Ok(history)
    }

    // ========== CI Operations ==========

    #[instrument(skip(self, snapshot))]
    pub async fn save_ci_snapshot(&self, snapshot: &CiSnapshot) -> Result<String> {
        let digest = snapshot.digest();

        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_CI_SNAPSHOTS, &digest, snapshot)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "CI snapshot already exists: {digest}"
                )));
            }
            return Ok(digest);
        }

        let mut store = self.store.lock().await;
        if store.ci_snapshots.contains_key(&digest) {
            return Err(StateError::Transaction(format!(
                "CI snapshot already exists: {digest}"
            )));
        }
        store.ci_snapshots.insert(digest.clone(), snapshot.clone());
        Ok(digest)
    }

    #[instrument(skip(self))]
    pub async fn load_ci_snapshot(&self, digest: &str) -> Result<Option<CiSnapshot>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_get(mesh, COLLECTION_CI_SNAPSHOTS, digest).await;
        }
        Ok(self.store.lock().await.ci_snapshots.get(digest).cloned())
    }

    #[instrument(skip(self, pipeline))]
    pub async fn save_ci_pipeline(&self, pipeline: &CiPipelineSpec) -> Result<String> {
        let digest = pipeline.digest();

        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_CI_PIPELINES, &digest, pipeline)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "CI pipeline already exists: {digest}"
                )));
            }
            return Ok(digest);
        }

        let mut store = self.store.lock().await;
        if store.ci_pipelines.contains_key(&digest) {
            return Err(StateError::Transaction(format!(
                "CI pipeline already exists: {digest}"
            )));
        }
        store.ci_pipelines.insert(digest.clone(), pipeline.clone());
        Ok(digest)
    }

    #[instrument(skip(self))]
    pub async fn load_ci_pipeline(&self, digest: &str) -> Result<Option<CiPipelineSpec>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_get(mesh, COLLECTION_CI_PIPELINES, digest).await;
        }
        Ok(self.store.lock().await.ci_pipelines.get(digest).cloned())
    }

    #[instrument(skip(self, run), fields(run_id = %run.run_id))]
    pub async fn save_ci_run(&self, run: &CiRunRecord) -> Result<CiRunRecord> {
        if let Some(mesh) = &self.mesh {
            if !self
                .mesh_create(mesh, COLLECTION_CI_RUNS, &run.run_id, run)
                .await?
            {
                return Err(StateError::Transaction(format!(
                    "CI run already exists: {}",
                    run.run_id
                )));
            }
            return Ok(run.clone());
        }

        let mut store = self.store.lock().await;
        if store.ci_runs.contains_key(&run.run_id) {
            return Err(StateError::Transaction(format!(
                "CI run already exists: {}",
                run.run_id
            )));
        }
        store.ci_runs.insert(run.run_id.clone(), run.clone());
        Ok(run.clone())
    }

    #[instrument(skip(self))]
    pub async fn get_ci_run(&self, run_id: &str) -> Result<Option<CiRunRecord>> {
        if let Some(mesh) = &self.mesh {
            return self.mesh_get(mesh, COLLECTION_CI_RUNS, run_id).await;
        }
        Ok(self.store.lock().await.ci_runs.get(run_id).cloned())
    }

    #[instrument(skip(self))]
    pub async fn list_ci_runs_by_snapshot(
        &self,
        snapshot_digest: &str,
    ) -> Result<Vec<CiRunRecord>> {
        let mut runs: Vec<CiRunRecord> = if let Some(mesh) = &self.mesh {
            self.mesh_all(mesh, COLLECTION_CI_RUNS, None).await?
        } else {
            self.store.lock().await.ci_runs.values().cloned().collect()
        };
        runs.retain(|run| run.snapshot_digest == snapshot_digest);
        runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(runs)
    }

    // ========== Decision and Provenance Operations ==========

    #[instrument(skip(self, record))]
    pub async fn save_decision(&self, record: &DecisionRecord) -> Result<DecisionRecord> {
        let mut stored = record.clone();
        let key = stored.decision_id.clone();
        stored.id.get_or_insert_with(|| format!("decision:{key}"));

        let mut store = self.store.lock().await;
        if store.decisions.contains_key(&key) {
            return Err(StateError::Transaction(format!(
                "Decision already exists: {key}"
            )));
        }
        store.decisions.insert(key.clone(), stored.clone());
        drop(store);

        self.mesh_write_through(COLLECTION_DECISIONS, &key, &stored)
            .await?;
        Ok(stored)
    }

    #[instrument(skip(self))]
    pub async fn get_decision(&self, decision_id: &str) -> Result<Option<DecisionRecord>> {
        Ok(self.store.lock().await.decisions.get(decision_id).cloned())
    }

    #[instrument(skip(self))]
    pub async fn update_decision_outcome(
        &self,
        decision_id: &str,
        outcome_json: String,
    ) -> Result<DecisionRecord> {
        let mut store = self.store.lock().await;
        let updated = {
            let decision = store.decisions.get_mut(decision_id).ok_or_else(|| {
                StateError::Transaction("Decision not found for update".to_string())
            })?;
            decision.outcome = Some(outcome_json);
            decision.outcome_at = Some(Utc::now());
            decision.clone()
        };
        drop(store);

        self.mesh_write_through(COLLECTION_DECISIONS, decision_id, &updated)
            .await?;
        Ok(updated)
    }

    #[instrument(skip(self))]
    pub async fn get_decision_history(
        &self,
        task: &str,
        limit: usize,
    ) -> Result<Vec<DecisionRecord>> {
        let mut decisions: Vec<_> = self
            .store
            .lock()
            .await
            .decisions
            .values()
            .filter(|decision| decision.task == task)
            .cloned()
            .collect();
        decisions.sort_by_key(|decision| std::cmp::Reverse(decision.timestamp));
        decisions.truncate(limit);
        Ok(decisions)
    }

    #[instrument(skip(self, record))]
    pub async fn save_provenance(
        &self,
        record: &MemoryProvenanceRecord,
    ) -> Result<MemoryProvenanceRecord> {
        let mut stored = record.clone();
        let key = stored.id.get_or_insert_with(|| new_id("prov")).clone();

        if let Some(mesh) = &self.mesh {
            self.mesh_create(mesh, COLLECTION_PROVENANCE, &key, &stored)
                .await?;
            return Ok(stored);
        }

        self.store.lock().await.provenances.push(stored.clone());
        Ok(stored)
    }

    #[instrument(skip(self))]
    pub async fn get_provenance(&self, memory_id: &str) -> Result<Vec<MemoryProvenanceRecord>> {
        let mut provenances: Vec<MemoryProvenanceRecord> = if let Some(mesh) = &self.mesh {
            self.mesh_all(mesh, COLLECTION_PROVENANCE, None).await?
        } else {
            self.store.lock().await.provenances.clone()
        };
        provenances.retain(|record| record.memory_id == memory_id);
        provenances.sort_by_key(|record| record.created_at);
        Ok(provenances)
    }

    // ========== History Operations ==========

    #[instrument(skip(self))]
    pub async fn get_commit_history(
        &self,
        start_commit: &str,
        limit: usize,
    ) -> Result<Vec<CommitRecord>> {
        let mut history = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        queue.push_back(start_commit.to_string());
        visited.insert(start_commit.to_string());

        while let Some(commit_hash) = queue.pop_front() {
            if history.len() >= limit {
                break;
            }

            if let Some(commit) = self.get_commit(&commit_hash).await? {
                let mut parents = self.get_parents(&commit_hash).await?;
                if parents.is_empty() {
                    if let Some(parent) = commit.parent_ids.first() {
                        parents.push(parent.clone());
                    }
                }

                for parent in parents {
                    if visited.insert(parent.clone()) {
                        queue.push_back(parent);
                    }
                }

                history.push(commit);
            }
        }

        Ok(history)
    }

    #[instrument(skip(self))]
    pub async fn get_reasoning_trace(&self, commit_id: &str) -> Result<Vec<SnapshotRecord>> {
        let history = self.get_commit_history(commit_id, 100).await?;
        let mut trace = Vec::new();

        for commit in history {
            if let Ok(snapshot) = self.load_snapshot(&commit.commit_id.hash).await {
                trace.push(snapshot);
            }
        }

        Ok(trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ---- mesh document helpers (the wire-shape-sensitive bits) ----

    #[test]
    fn with_mesh_key_injects_a_queryable_key() {
        let value = serde_json::json!({ "hash": "abc" });
        let keyed = with_mesh_key(value, "abc");
        assert_eq!(keyed[MESH_KEY_FIELD], "abc");
        assert_eq!(keyed["hash"], "abc");
    }

    #[test]
    fn doc_into_reads_a_raw_payload() {
        // The mesh may return the payload directly.
        let doc = serde_json::json!({ "name": "main", "n": 1 });
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct T {
            name: String,
            n: u8,
        }
        assert_eq!(
            doc_into::<T>(doc),
            Some(T {
                name: "main".into(),
                n: 1
            })
        );
    }

    #[test]
    fn doc_into_unwraps_a_data_envelope() {
        // …or wrapped under `data` alongside a server-assigned id.
        let doc = serde_json::json!({ "id": "srv-1", "data": { "name": "main", "n": 2 } });
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct T {
            name: String,
            n: u8,
        }
        assert_eq!(
            doc_into::<T>(doc),
            Some(T {
                name: "main".into(),
                n: 2
            })
        );
    }

    #[test]
    fn doc_into_ignores_the_injected_key_on_the_way_back() {
        // A record round-trips even though the stored doc carries MESH_KEY_FIELD.
        let stored = with_mesh_key(serde_json::json!({ "name": "main", "n": 3 }), "main");
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct T {
            name: String,
            n: u8,
        }
        assert_eq!(
            doc_into::<T>(stored),
            Some(T {
                name: "main".into(),
                n: 3
            })
        );
    }

    #[test]
    fn created_status_exists_means_duplicate() {
        assert!(created_is_new("created"));
        assert!(!created_is_new("exists"));
    }

    #[tokio::test]
    async fn test_in_memory_connection_and_schema_creation() {
        let handle = SurrealHandle::setup_db().await;
        assert!(handle.is_ok(), "Failed to connect: {:?}", handle.err());
    }

    #[tokio::test]
    async fn test_snapshot_is_atomic_and_retrievable() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let state = serde_json::json!({
            "agent_name": "test-agent",
            "step": 1,
            "variables": {"x": 42, "y": "hello"}
        });

        let commit_id = CommitId::from_state(serde_json::to_vec(&state).unwrap().as_slice());
        handle
            .save_snapshot(&commit_id, state.clone())
            .await
            .unwrap();
        let loaded = handle.load_snapshot(&commit_id.hash).await.unwrap();

        assert_eq!(loaded.commit_id, commit_id.hash);
        assert_eq!(loaded.state, state);
    }

    #[tokio::test]
    async fn test_parent_child_edge_is_created() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let parent_id = "parent-commit-hash";
        let child_id = "child-commit-hash";

        handle
            .save_commit_graph_edge(child_id, parent_id)
            .await
            .unwrap();

        let parent = handle.get_parent(child_id).await.unwrap();
        assert_eq!(parent, Some(parent_id.to_string()));

        let children = handle.get_children(parent_id).await.unwrap();
        assert!(children.contains(&child_id.to_string()));
    }

    #[tokio::test]
    async fn test_commit_record_operations() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let commit_id = CommitId::from_state(b"test state");
        let commit = CommitRecord::new(commit_id.clone(), vec![], "Initial commit", "test-agent");

        let saved = handle.save_commit(&commit).await.unwrap();
        assert_eq!(saved.commit_id.hash, commit_id.hash);

        let loaded = handle.get_commit(&commit_id.hash).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().message, "Initial commit");
    }

    #[tokio::test]
    async fn test_get_trace_for_commit_id_returns_correct_cot() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let state_0 = serde_json::json!({"step": 0, "thought": "Starting exploration"});
        let state_1 = serde_json::json!({"step": 1, "thought": "Trying strategy A"});
        let state_2 = serde_json::json!({"step": 2, "thought": "Strategy A failed, pivoting"});
        let state_3 = serde_json::json!({"step": 3, "thought": "Strategy B succeeded"});

        let id_0 = CommitId::from_state(b"state-0");
        let id_1 = CommitId::from_state(b"state-1");
        let id_2 = CommitId::from_state(b"state-2");
        let id_3 = CommitId::from_state(b"state-3");

        handle.save_snapshot(&id_0, state_0.clone()).await.unwrap();
        handle.save_snapshot(&id_1, state_1.clone()).await.unwrap();
        handle.save_snapshot(&id_2, state_2.clone()).await.unwrap();
        handle.save_snapshot(&id_3, state_3.clone()).await.unwrap();

        let commit_0 = CommitRecord::new(id_0.clone(), vec![], "Step 0", "agent");
        let commit_1 = CommitRecord::new(id_1.clone(), vec![id_0.hash.clone()], "Step 1", "agent");
        let commit_2 = CommitRecord::new(id_2.clone(), vec![id_1.hash.clone()], "Step 2", "agent");
        let commit_3 = CommitRecord::new(id_3.clone(), vec![id_2.hash.clone()], "Step 3", "agent");

        handle.save_commit(&commit_0).await.unwrap();
        handle.save_commit(&commit_1).await.unwrap();
        handle.save_commit(&commit_2).await.unwrap();
        handle.save_commit(&commit_3).await.unwrap();

        let trace = handle.get_reasoning_trace(&id_3.hash).await.unwrap();
        assert_eq!(trace.len(), 4, "Trace should contain all 4 commits");
        assert_eq!(trace[0].state["step"], 3);
        assert_eq!(trace[1].state["step"], 2);
        assert_eq!(trace[2].state["step"], 1);
        assert_eq!(trace[3].state["step"], 0);
        assert_eq!(trace[0].state["thought"], "Strategy B succeeded");
        assert_eq!(trace[1].state["thought"], "Strategy A failed, pivoting");
        assert_eq!(trace[2].state["thought"], "Trying strategy A");
        assert_eq!(trace[3].state["thought"], "Starting exploration");
    }

    #[tokio::test]
    async fn test_ci_records_roundtrip() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let snapshot = CiSnapshot {
            repo_sha: "abc123".to_string(),
            workspace_hash: "work-1".to_string(),
            local_ci_config_hash: "cfg-1".to_string(),
            env_hash: "env-1".to_string(),
        };
        let snapshot_digest = handle.save_ci_snapshot(&snapshot).await.unwrap();
        let loaded_snapshot = handle.load_ci_snapshot(&snapshot_digest).await.unwrap();
        assert_eq!(loaded_snapshot, Some(snapshot.clone()));

        let pipeline = CiPipelineSpec {
            name: "default".to_string(),
            steps: vec![crate::ci::CiStepSpec {
                name: "test".to_string(),
                command: crate::ci::CiCommand {
                    program: "cargo".to_string(),
                    args: vec!["test".to_string()],
                    env: BTreeMap::new(),
                    cwd: None,
                },
                timeout_secs: Some(300),
                allow_failure: false,
            }],
        };
        let pipeline_digest = handle.save_ci_pipeline(&pipeline).await.unwrap();
        let loaded_pipeline = handle.load_ci_pipeline(&pipeline_digest).await.unwrap();
        assert_eq!(loaded_pipeline, Some(pipeline.clone()));

        let run = CiRunRecord::queued(&snapshot_digest, &pipeline_digest);
        let saved_run = handle.save_ci_run(&run).await.unwrap();
        let loaded_run = handle.get_ci_run(&saved_run.run_id).await.unwrap();
        assert_eq!(loaded_run, Some(saved_run.clone()));

        let runs = handle
            .list_ci_runs_by_snapshot(&snapshot_digest)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, saved_run.run_id);
    }

    #[tokio::test]
    async fn test_release_fields_roundtrip() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let metadata = ReleaseMetadata {
            version_label: Some("v1.2.3".to_string()),
            promoted_by: "test-user".to_string(),
            notes: Some("Release notes here".to_string()),
        };
        let digest = ContentDigest::from_bytes(b"spec-data");

        let release = handle
            .release_promote("test-agent", &digest, metadata.clone())
            .await
            .unwrap();

        assert_eq!(release.name, "test-agent");
        assert_eq!(release.metadata.version_label, Some("v1.2.3".to_string()));
        assert_eq!(release.metadata.promoted_by, "test-user");
        assert_eq!(
            release.metadata.notes,
            Some("Release notes here".to_string())
        );

        let current = handle.release_current("test-agent").await.unwrap().unwrap();
        assert_eq!(current.spec_digest, digest);

        let history = handle.release_history("test-agent").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].metadata.version_label,
            Some("v1.2.3".to_string())
        );
        assert_eq!(history[0].metadata.promoted_by, "test-user");
    }
}
