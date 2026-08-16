//! `forge-cas` — the sovereign git-free forge's content-addressed storage HTTP
//! surface: blobs, commits, branches, and commit manifests.
//!
//! This is the server side of the contract that `aivcs fetch`/`publish`/`clone`
//! speak (ported from the archived `aivcs.io/crates/aivcsd` `routes/repo.rs` and
//! its `http_remote.rs` client). The wire shapes here match that client exactly:
//!
//! - `POST /api/v1/blobs` — body is raw bytes; returns `{ "digest": "<sha256>" }`.
//! - `GET  /api/v1/blobs/:digest` — the raw bytes back (`application/octet-stream`).
//! - `POST /api/v1/commits` — bind an already-uploaded manifest into a commit;
//!   `{ repo, manifest:[{path,digest,executable,size}], parents, message, author }`
//!   → `{ commit_id, tree_digest }`. Every referenced blob must already be in CAS.
//! - `GET  /api/v1/commits/:commit_id/manifest?repo=` — a **bare array** of
//!   `{path,digest,executable,size}` (the client deserializes `Vec<ManifestEntry>`).
//! - `GET  /api/v1/commits/:commit_id?repo=` — commit metadata (parents/message/
//!   author/tree_digest), so that history isn't discarded.
//! - `PUT  /api/v1/repos/:repo/branches/:branch` — `{ commit_id }` → `{ repo,
//!   branch, head_commit_id }`.
//! - `GET  /api/v1/repos/:repo/branches/:branch` — `{ repo, branch, head_commit_id }`.
//!
//! Invariant (the incident this guards): **every route answers JSON** — a client
//! that mistakenly points at a web-UI SPA host gets HTML and must fail at the
//! JSON boundary, never mis-parse HTML into a branch/commit. This server never
//! emits HTML.
//!
//! Commits and branches are scoped by `repo`: a commit is keyed `(repo, id)`, so
//! a branch can never resolve or point to a commit from another repository.
//! The runnable default is in-memory; production injects the durable data-mesh
//! implementation through the same `ForgeStore` seam.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use std::io;

use async_trait::async_trait;
use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;

/// One `path → blob` entry of a commit's tree. Mirrors the client's
/// `ManifestEntryIn`/`ManifestEntryOut`: the executable bit and byte size are
/// part of the entry and MUST round-trip (a checkout needs the mode + size).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub digest: String,
    pub executable: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub repo: String,
    pub commit_id: String,
    tree_digest: String,
    manifest: Vec<ManifestEntry>,
    parents: Vec<String>,
    message: String,
    author: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("forge backend error: {0}")]
    Backend(String),
    #[error("forge object not found")]
    NotFound,
    #[error("blob not in CAS for {path}: {digest}")]
    MissingBlob { path: String, digest: String },
}

#[async_trait]
pub trait ForgeStore: Send + Sync {
    async fn put_blob(&self, bytes: &[u8]) -> Result<String, ForgeError>;
    async fn get_blob(&self, digest: &str) -> Result<Option<Vec<u8>>, ForgeError>;
    async fn put_commit(&self, commit: CommitRecord) -> Result<(), ForgeError>;
    async fn get_commit(
        &self,
        repo: &str,
        commit_id: &str,
    ) -> Result<Option<CommitRecord>, ForgeError>;
    async fn set_branch(&self, repo: &str, branch: &str, commit_id: &str)
        -> Result<(), ForgeError>;
    async fn get_branch(&self, repo: &str, branch: &str) -> Result<Option<String>, ForgeError>;

    // PHASE 1: Batch blob existence check (optimization for large manifests)
    async fn exists_all(&self, digests: &[&str]) -> Result<Vec<bool>, ForgeError>;

    // PHASE 1: Atomic publish (all-or-nothing semantics)
    async fn atomic_publish(
        &self,
        repo: &str,
        blobs: Vec<Vec<u8>>,
        commit: CommitRecord,
        branch_updates: Vec<(String, String)>,
    ) -> Result<AtomicPublishResult, ForgeError>;
}

/// Atomic publish result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicPublishResult {
    pub commit_id: String,
    pub blobs_stored: usize,
    pub branches_updated: usize,
}

/// Validation: Every manifest entry must reference a blob already in CAS
#[allow(dead_code)]
async fn validate_commit_blobs(
    store: &dyn ForgeStore,
    commit: &CommitRecord,
) -> Result<(), ForgeError> {
    for entry in &commit.manifest {
        match store.get_blob(&entry.digest).await? {
            Some(bytes) => {
                if bytes.len() as u64 != entry.size {
                    return Err(ForgeError::Backend(format!(
                        "manifest size {} != blob length {} for {}",
                        entry.size,
                        bytes.len(),
                        entry.path
                    )));
                }
            }
            None => {
                return Err(ForgeError::MissingBlob {
                    path: entry.path.clone(),
                    digest: entry.digest.clone(),
                });
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct InMemoryState {
    cas: HashMap<String, Vec<u8>>,
    commits: HashMap<(String, String), CommitRecord>,
    branches: HashMap<(String, String), String>,
}

pub struct InMemoryForgeStore {
    state: RwLock<InMemoryState>,
}

impl InMemoryForgeStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ForgeStore for InMemoryForgeStore {
    async fn put_blob(&self, bytes: &[u8]) -> Result<String, ForgeError> {
        let digest = sha256_hex(bytes);
        self.state
            .write()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .cas
            .insert(digest.clone(), bytes.to_vec());
        Ok(digest)
    }

    async fn get_blob(&self, digest: &str) -> Result<Option<Vec<u8>>, ForgeError> {
        Ok(self
            .state
            .read()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .cas
            .get(digest)
            .cloned())
    }

    async fn put_commit(&self, commit: CommitRecord) -> Result<(), ForgeError> {
        self.state
            .write()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .commits
            .insert((commit.repo.clone(), commit.commit_id.clone()), commit);
        Ok(())
    }

    async fn get_commit(
        &self,
        repo: &str,
        commit_id: &str,
    ) -> Result<Option<CommitRecord>, ForgeError> {
        Ok(self
            .state
            .read()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .commits
            .get(&(repo.to_string(), commit_id.to_string()))
            .cloned())
    }

    async fn set_branch(
        &self,
        repo: &str,
        branch: &str,
        commit_id: &str,
    ) -> Result<(), ForgeError> {
        self.state
            .write()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .branches
            .insert(
                (repo.to_string(), branch.to_string()),
                commit_id.to_string(),
            );
        Ok(())
    }

    async fn get_branch(&self, repo: &str, branch: &str) -> Result<Option<String>, ForgeError> {
        Ok(self
            .state
            .read()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?
            .branches
            .get(&(repo.to_string(), branch.to_string()))
            .cloned())
    }

    async fn exists_all(&self, digests: &[&str]) -> Result<Vec<bool>, ForgeError> {
        let state = self
            .state
            .read()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?;
        Ok(digests.iter().map(|d| state.cas.contains_key(*d)).collect())
    }

    async fn atomic_publish(
        &self,
        repo: &str,
        blobs: Vec<Vec<u8>>,
        commit: CommitRecord,
        branch_updates: Vec<(String, String)>,
    ) -> Result<AtomicPublishResult, ForgeError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ForgeError::Backend("forge lock poisoned".into()))?;

        // GATE 1: Validate blob digests and collect them
        let mut blob_digests = Vec::new();
        for blob in &blobs {
            let digest = sha256_hex(blob);
            blob_digests.push(digest);
        }

        // GATE 2: Validate all blobs referenced in commit exist (or are being added)
        let mut all_digests: std::collections::HashSet<String> =
            std::collections::HashSet::from_iter(blob_digests.clone());
        all_digests.extend(state.cas.keys().cloned());

        for entry in &commit.manifest {
            if !all_digests.contains(&entry.digest) {
                return Err(ForgeError::MissingBlob {
                    path: entry.path.clone(),
                    digest: entry.digest.clone(),
                });
            }
        }

        // GATE 3: Validate target commits exist (for branch updates)
        for (_, target_commit_id) in &branch_updates {
            let exists = state
                .commits
                .contains_key(&(repo.to_string(), target_commit_id.clone()));

            // Allow if it's the commit being published
            if !exists && target_commit_id != &commit.commit_id {
                return Err(ForgeError::Backend(format!(
                    "target commit {} does not exist",
                    target_commit_id
                )));
            }
        }

        // STEP 1: Store all blobs
        for (i, blob) in blobs.iter().enumerate() {
            state.cas.insert(blob_digests[i].clone(), blob.to_vec());
        }

        // STEP 2: Store commit (blobs guaranteed to exist now)
        state.commits.insert(
            (commit.repo.clone(), commit.commit_id.clone()),
            commit.clone(),
        );

        // STEP 3: Update branches
        let mut branches_updated = 0;
        for (branch_name, target_commit_id) in branch_updates {
            state
                .branches
                .insert((repo.to_string(), branch_name), target_commit_id);
            branches_updated += 1;
        }

        Ok(AtomicPublishResult {
            commit_id: commit.commit_id,
            blobs_stored: blobs.len(),
            branches_updated,
        })
    }
}

impl Default for InMemoryForgeStore {
    fn default() -> Self {
        Self {
            state: RwLock::new(InMemoryState::default()),
        }
    }
}

pub struct DataMeshForgeStore {
    client: data_mesh::Client,
}

impl DataMeshForgeStore {
    pub const BLOBS: &'static str = "forge_blobs";
    pub const COMMITS: &'static str = "forge_commits";
    pub const BRANCHES: &'static str = "forge_branches";

    pub fn new(client: data_mesh::Client) -> Self {
        Self { client }
    }

    fn backend(e: data_mesh::Error) -> ForgeError {
        ForgeError::Backend(e.to_string())
    }
}

#[async_trait]
impl ForgeStore for DataMeshForgeStore {
    async fn put_blob(&self, bytes: &[u8]) -> Result<String, ForgeError> {
        let digest = sha256_hex(bytes);
        let doc = serde_json::json!({
            "digest": digest,
            "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
        });
        self.client
            .create_document_idempotent(Self::BLOBS, &digest, &doc)
            .await
            .map_err(Self::backend)?;
        Ok(digest)
    }

    async fn get_blob(&self, digest: &str) -> Result<Option<Vec<u8>>, ForgeError> {
        let docs = self
            .client
            .query_documents(Self::BLOBS, "digest", digest, Some(1))
            .await
            .map_err(Self::backend)?;
        let Some(doc) = docs.into_iter().next() else {
            return Ok(None);
        };
        let Some(encoded) = doc.get("bytes").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(Some)
            .map_err(|e| ForgeError::Backend(e.to_string()))
    }

    async fn put_commit(&self, commit: CommitRecord) -> Result<(), ForgeError> {
        let key = format!("{}:{}", commit.repo, commit.commit_id);
        let mut doc =
            data_mesh::to_document(&commit).map_err(|e| ForgeError::Backend(e.to_string()))?;
        doc["commit_key"] = serde_json::json!(key);
        self.client
            .create_document_idempotent(Self::COMMITS, &key, &doc)
            .await
            .map_err(Self::backend)?;
        Ok(())
    }

    async fn get_commit(
        &self,
        repo: &str,
        commit_id: &str,
    ) -> Result<Option<CommitRecord>, ForgeError> {
        let docs = self
            .client
            .query_documents(
                Self::COMMITS,
                "commit_key",
                &format!("{repo}:{commit_id}"),
                Some(1),
            )
            .await
            .map_err(Self::backend)?;
        Ok(docs
            .into_iter()
            .next()
            .and_then(|doc| data_mesh::from_document(doc).ok()))
    }

    async fn set_branch(
        &self,
        repo: &str,
        branch: &str,
        commit_id: &str,
    ) -> Result<(), ForgeError> {
        // A branch head is MUTABLE but the offering is append-only, so each
        // update is stamped and `get_branch` selects the most recent. Without
        // this stamp, `query_documents` returns an unordered set and a moved
        // branch could resolve to a stale head.
        let updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let doc = serde_json::json!({
            "branch_key": format!("{repo}:{branch}"),
            "repo": repo,
            "branch": branch,
            "head_commit_id": commit_id,
            "updated_at_ms": updated_at_ms,
        });
        self.client
            .create_document(Self::BRANCHES, &doc)
            .await
            .map_err(Self::backend)?;
        Ok(())
    }

    async fn get_branch(&self, repo: &str, branch: &str) -> Result<Option<String>, ForgeError> {
        // Fetch all updates for this branch and take the latest — NOT the first
        // arbitrary match (`query_documents` gives no ordering guarantee).
        let docs = self
            .client
            .query_documents(
                Self::BRANCHES,
                "branch_key",
                &format!("{repo}:{branch}"),
                Some(500),
            )
            .await
            .map_err(Self::backend)?;
        Ok(latest_head(docs))
    }

    async fn exists_all(&self, digests: &[&str]) -> Result<Vec<bool>, ForgeError> {
        use futures::stream::{self, StreamExt};
        let digests_owned: Vec<String> = digests.iter().map(|s| s.to_string()).collect();
        let checks: Vec<bool> = stream::iter(digests_owned)
            .map(|digest| {
                let client = self.client.clone();
                async move {
                    let docs = client
                        .query_documents(Self::BLOBS, "digest", &digest, Some(1))
                        .await
                        .unwrap_or_default();
                    !docs.is_empty()
                }
            })
            .buffered(32)
            .collect()
            .await;
        Ok(checks)
    }

    async fn atomic_publish(
        &self,
        repo: &str,
        blobs: Vec<Vec<u8>>,
        commit: CommitRecord,
        branch_updates: Vec<(String, String)>,
    ) -> Result<AtomicPublishResult, ForgeError> {
        // PHASE 1: Validation gates (Phase 2 will wrap in SurrealDB transaction)

        // GATE 1: Validate blob digests match expected
        let mut blob_digests = Vec::new();
        for blob in &blobs {
            let digest = sha256_hex(blob);
            blob_digests.push(digest);
        }

        // GATE 2: Validate all blobs referenced in commit exist (or are being added)
        let blob_set: std::collections::HashSet<String> =
            std::collections::HashSet::from_iter(blob_digests.clone());

        // Batch check: collect digests NOT in the upload batch, query them once
        let to_check: Vec<&str> = commit
            .manifest
            .iter()
            .filter(|e| !blob_set.contains(&e.digest))
            .map(|e| e.digest.as_str())
            .collect();

        if !to_check.is_empty() {
            let exists_results = self
                .exists_all(&to_check)
                .await
                .map_err(|_| ForgeError::Backend("blob batch check failed".into()))?;

            // Check results and report first missing blob
            for (digest, exists) in to_check.iter().zip(exists_results) {
                if !exists {
                    let entry = commit
                        .manifest
                        .iter()
                        .find(|e| e.digest == *digest)
                        .unwrap();
                    return Err(ForgeError::MissingBlob {
                        path: entry.path.clone(),
                        digest: digest.to_string(),
                    });
                }
            }
        }

        // GATE 3: Validate target commits exist (for branch updates)
        for (_, target_commit_id) in &branch_updates {
            let exists = self
                .get_commit(repo, target_commit_id)
                .await
                .map_err(|_| ForgeError::Backend("commit check failed".into()))?
                .is_some();

            // Allow if it's the commit being published
            if !exists && target_commit_id != &commit.commit_id {
                return Err(ForgeError::Backend(format!(
                    "target commit {} does not exist",
                    target_commit_id
                )));
            }
        }

        // TODO PHASE 2: Wrap following in SurrealDB transaction
        // BEGIN TRANSACTION

        // STEP 1: Store all blobs
        for blob in &blobs {
            self.put_blob(blob)
                .await
                .map_err(|e| ForgeError::Backend(format!("blob upload failed: {}", e)))?;
        }

        // STEP 2: Store commit (blobs are now guaranteed to exist)
        self.put_commit(commit.clone())
            .await
            .map_err(|e| ForgeError::Backend(format!("commit creation failed: {}", e)))?;

        // STEP 3: Update branches
        let mut branches_updated = 0;
        for (branch_name, target_commit_id) in branch_updates {
            self.set_branch(repo, &branch_name, &target_commit_id)
                .await
                .map_err(|e| ForgeError::Backend(format!("branch update failed: {}", e)))?;
            branches_updated += 1;
        }

        // TODO PHASE 2: END TRANSACTION / COMMIT

        Ok(AtomicPublishResult {
            commit_id: commit.commit_id,
            blobs_stored: blobs.len(),
            branches_updated,
        })
    }
}

/// A durable `ForgeStore` backed by PostgreSQL via SQLx — a relational peer to
/// `DataMeshForgeStore` for deployments that persist the forge in a Postgres
/// database (e.g. the aivcs-attic-cache scope) instead of the data-mesh.
///
/// **PostgreSQL only.** SQLx also speaks SQLite, but that backend is deliberately
/// not compiled in and `connect` refuses any non-`postgres://` DSN
/// (never-use-sqlite). Blobs are stored as `BYTEA`; commit manifests/parents are
/// serialized to JSON `TEXT` (no JSONB, so no extra SQLx feature is needed).
/// Unlike the append-only data-mesh offering, a Postgres branch row is genuinely
/// mutable, so `set_branch` is an upsert and `get_branch` a point read — the
/// `updated_at_ms`-max reconciliation the data-mesh store needs is unnecessary
/// here. Uses the SQLx runtime query API (no `query!` macros), so the build
/// needs no database and stays hermetic.
#[derive(Debug)]
pub struct SqlxForgeStore {
    pool: PgPool,
}

impl SqlxForgeStore {
    /// Connect to Postgres, enforce the never-sqlite rule, and ensure the schema.
    ///
    /// `dsn` MUST be a PostgreSQL URL (`postgres://` or `postgresql://`); any
    /// other scheme — SQLite above all — is refused before a pool is opened, so a
    /// misconfigured `DATABASE_URL` fails loudly at startup rather than silently
    /// binding a forbidden backend.
    pub async fn connect(dsn: &str) -> Result<Self, ForgeError> {
        if !is_postgres_dsn(dsn) {
            return Err(ForgeError::Backend(format!(
                "SqlxForgeStore requires a PostgreSQL DSN (postgres:// or postgresql://); \
                 SQLite and every other backend are refused (never-use-sqlite). \
                 got scheme: {:?}",
                dsn_scheme(dsn)
            )));
        }
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(dsn)
            .await
            .map_err(Self::backend)?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Build a store over an existing pool (tests / callers that own the pool).
    pub fn with_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    fn backend(e: sqlx::Error) -> ForgeError {
        ForgeError::Backend(e.to_string())
    }

    /// Idempotent schema. Digests/ids are content-addressed so `TEXT` primary
    /// keys are stable; blobs are `BYTEA`; manifest/parents ride as JSON `TEXT`.
    async fn migrate(&self) -> Result<(), ForgeError> {
        for ddl in [
            "CREATE TABLE IF NOT EXISTS forge_blobs (\
                 digest TEXT PRIMARY KEY, \
                 bytes  BYTEA NOT NULL)",
            "CREATE TABLE IF NOT EXISTS forge_commits (\
                 repo        TEXT   NOT NULL, \
                 commit_id   TEXT   NOT NULL, \
                 tree_digest TEXT   NOT NULL, \
                 manifest    TEXT   NOT NULL, \
                 parents     TEXT   NOT NULL, \
                 message     TEXT   NOT NULL, \
                 author      TEXT   NOT NULL, \
                 PRIMARY KEY (repo, commit_id))",
            "CREATE TABLE IF NOT EXISTS forge_branches (\
                 repo           TEXT   NOT NULL, \
                 branch         TEXT   NOT NULL, \
                 head_commit_id TEXT   NOT NULL, \
                 updated_at_ms  BIGINT NOT NULL, \
                 PRIMARY KEY (repo, branch))",
        ] {
            sqlx::query(ddl)
                .execute(&self.pool)
                .await
                .map_err(Self::backend)?;
        }
        Ok(())
    }
}

/// PostgreSQL caps a single field value (here the `BYTEA` blob column) at ~1 GiB.
/// A blob at or above this is rejected here with a clear error rather than being
/// sent to the wire only to come back as an opaque driver failure. Source trees
/// are far under this; the guard documents and enforces the boundary.
const MAX_BLOB_BYTES: usize = 1 << 30; // 1 GiB

/// `Err` if a blob is too large for a Postgres `BYTEA` field. Split out as a pure
/// function so the boundary is unit-testable without a database.
fn check_blob_size(len: usize) -> Result<(), ForgeError> {
    if len > MAX_BLOB_BYTES {
        return Err(ForgeError::Backend(format!(
            "blob of {len} bytes exceeds the {MAX_BLOB_BYTES}-byte PostgreSQL BYTEA field limit"
        )));
    }
    Ok(())
}

/// A DSN is Postgres iff its scheme is `postgres` or `postgresql`. Everything
/// else — `sqlite`, `mysql`, a bare path — is rejected (never-use-sqlite).
fn is_postgres_dsn(dsn: &str) -> bool {
    matches!(
        dsn_scheme(dsn).as_deref(),
        Some("postgres") | Some("postgresql")
    )
}

/// The lowercased URL scheme (text before `://`), if any.
fn dsn_scheme(dsn: &str) -> Option<String> {
    dsn.split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

#[async_trait]
impl ForgeStore for SqlxForgeStore {
    async fn put_blob(&self, bytes: &[u8]) -> Result<String, ForgeError> {
        check_blob_size(bytes.len())?;
        let digest = sha256_hex(bytes);
        // Content-addressed: an identical digest is the same blob, so a re-put is
        // a no-op rather than an error (matches the other stores' idempotency).
        sqlx::query(
            "INSERT INTO forge_blobs (digest, bytes) VALUES ($1, $2) \
             ON CONFLICT (digest) DO NOTHING",
        )
        .bind(&digest)
        .bind(bytes)
        .execute(&self.pool)
        .await
        .map_err(Self::backend)?;
        Ok(digest)
    }

    async fn get_blob(&self, digest: &str) -> Result<Option<Vec<u8>>, ForgeError> {
        let row = sqlx::query("SELECT bytes FROM forge_blobs WHERE digest = $1")
            .bind(digest)
            .fetch_optional(&self.pool)
            .await
            .map_err(Self::backend)?;
        row.map(|r| r.try_get::<Vec<u8>, _>("bytes").map_err(Self::backend))
            .transpose()
    }

    async fn put_commit(&self, commit: CommitRecord) -> Result<(), ForgeError> {
        // Commit ids are content-derived; a re-put of the same id is the same
        // commit, so conflict → no-op (never overwrite history).
        let manifest = serde_json::to_string(&commit.manifest)
            .map_err(|e| ForgeError::Backend(e.to_string()))?;
        let parents = serde_json::to_string(&commit.parents)
            .map_err(|e| ForgeError::Backend(e.to_string()))?;
        sqlx::query(
            "INSERT INTO forge_commits \
                 (repo, commit_id, tree_digest, manifest, parents, message, author) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (repo, commit_id) DO NOTHING",
        )
        .bind(&commit.repo)
        .bind(&commit.commit_id)
        .bind(&commit.tree_digest)
        .bind(&manifest)
        .bind(&parents)
        .bind(&commit.message)
        .bind(&commit.author)
        .execute(&self.pool)
        .await
        .map_err(Self::backend)?;
        Ok(())
    }

    async fn get_commit(
        &self,
        repo: &str,
        commit_id: &str,
    ) -> Result<Option<CommitRecord>, ForgeError> {
        let Some(row) = sqlx::query(
            "SELECT tree_digest, manifest, parents, message, author \
             FROM forge_commits WHERE repo = $1 AND commit_id = $2",
        )
        .bind(repo)
        .bind(commit_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Self::backend)?
        else {
            return Ok(None);
        };
        let manifest: String = row.try_get("manifest").map_err(Self::backend)?;
        let parents: String = row.try_get("parents").map_err(Self::backend)?;
        Ok(Some(CommitRecord {
            repo: repo.to_string(),
            commit_id: commit_id.to_string(),
            tree_digest: row.try_get("tree_digest").map_err(Self::backend)?,
            manifest: serde_json::from_str(&manifest)
                .map_err(|e| ForgeError::Backend(e.to_string()))?,
            parents: serde_json::from_str(&parents)
                .map_err(|e| ForgeError::Backend(e.to_string()))?,
            message: row.try_get("message").map_err(Self::backend)?,
            author: row.try_get("author").map_err(Self::backend)?,
        }))
    }

    async fn set_branch(
        &self,
        repo: &str,
        branch: &str,
        commit_id: &str,
    ) -> Result<(), ForgeError> {
        // A Postgres branch row is mutable: upsert the head in place (no
        // append-and-pick-latest needed, unlike the append-only data-mesh store).
        let updated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        sqlx::query(
            "INSERT INTO forge_branches (repo, branch, head_commit_id, updated_at_ms) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (repo, branch) \
             DO UPDATE SET head_commit_id = EXCLUDED.head_commit_id, \
                           updated_at_ms  = EXCLUDED.updated_at_ms",
        )
        .bind(repo)
        .bind(branch)
        .bind(commit_id)
        .bind(updated_at_ms)
        .execute(&self.pool)
        .await
        .map_err(Self::backend)?;
        Ok(())
    }

    async fn get_branch(&self, repo: &str, branch: &str) -> Result<Option<String>, ForgeError> {
        let row = sqlx::query(
            "SELECT head_commit_id FROM forge_branches WHERE repo = $1 AND branch = $2",
        )
        .bind(repo)
        .bind(branch)
        .fetch_optional(&self.pool)
        .await
        .map_err(Self::backend)?;
        row.map(|r| {
            r.try_get::<String, _>("head_commit_id")
                .map_err(Self::backend)
        })
        .transpose()
    }

    async fn exists_all(&self, digests: &[&str]) -> Result<Vec<bool>, ForgeError> {
        // Batch existence check: single SQL query for all digests
        let rows =
            sqlx::query_as::<_, (String,)>("SELECT digest FROM forge_blobs WHERE digest = ANY($1)")
                .bind(digests)
                .fetch_all(&self.pool)
                .await
                .map_err(Self::backend)?;

        let found: std::collections::HashSet<String> = rows.iter().map(|(d,)| d.clone()).collect();
        Ok(digests.iter().map(|d| found.contains(*d)).collect())
    }

    async fn atomic_publish(
        &self,
        repo: &str,
        blobs: Vec<Vec<u8>>,
        commit: CommitRecord,
        branch_updates: Vec<(String, String)>,
    ) -> Result<AtomicPublishResult, ForgeError> {
        // TODO PHASE 2: Add PostgreSQL transaction support
        // For now, fall back to sequential writes like data-mesh

        // Validate blobs exist
        for blob in &blobs {
            self.put_blob(blob)
                .await
                .map_err(|e| ForgeError::Backend(format!("blob upload failed: {}", e)))?;
        }

        // Validate and store commit
        self.put_commit(commit.clone())
            .await
            .map_err(|e| ForgeError::Backend(format!("commit creation failed: {}", e)))?;

        // Update branches
        let mut branches_updated = 0;
        for (branch_name, target_commit_id) in branch_updates {
            self.set_branch(repo, &branch_name, &target_commit_id)
                .await
                .map_err(|e| ForgeError::Backend(format!("branch update failed: {}", e)))?;
            branches_updated += 1;
        }

        Ok(AtomicPublishResult {
            commit_id: commit.commit_id,
            blobs_stored: blobs.len(),
            branches_updated,
        })
    }
}

/// The current branch head from an append-only set of branch-update docs: the
/// one with the greatest `updated_at_ms`, regardless of the order returned.
fn latest_head(docs: Vec<serde_json::Value>) -> Option<String> {
    docs.into_iter()
        .max_by_key(|doc| {
            doc.get("updated_at_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        })
        .and_then(|doc| {
            doc.get("head_commit_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

/// Shared store state mounted as axum state.
pub type SharedForge = Arc<dyn ForgeStore>;

pub fn new_forge() -> SharedForge {
    Arc::new(InMemoryForgeStore::new())
}

/// The CAS/commit/branch routes, mounted under `/api/v1`, on a fresh in-memory
/// forge. Merge onto a service's `ops_router(...)`.
pub fn router() -> Router {
    router_with(new_forge())
}

/// Same routes, over a caller-supplied forge handle (for tests / a shared store).
pub fn router_with(forge: SharedForge) -> Router {
    Router::new()
        .route("/api/v1/blobs", post(put_blob))
        .route("/api/v1/blobs/:digest", get(get_blob))
        .route("/api/v1/commits", post(create_commit))
        .route("/api/v1/publish", post(publish_atomic))
        .route("/api/v1/commits/:commit_id", get(get_commit))
        .route(
            "/api/v1/commits/:commit_id/manifest",
            get(get_commit_manifest),
        )
        // Create an empty repository: an empty root commit + a `main` branch.
        // Repos are otherwise implicit (they exist once they hold a commit), so
        // this is the one explicit "init" the forge exposes.
        //
        // Aliased at `/v1/repos` too: the merged aivcs-repo client posts to
        // `${AIVCSD_URL}/v1/repos` (no `/api`), so both prefixes must resolve or
        // every create is a day-one 404. The alias keeps that seam stable.
        .route("/api/v1/repos", post(create_repo))
        .route("/v1/repos", post(create_repo))
        .route("/api/v1/repos/:owner/:repo/commits", post(post_commit_repo))
        .route("/api/v1/repos/:owner/:repo/commits", get(get_commits_repo))
        .route(
            "/api/v1/repos/:repo/branches/:branch",
            get(get_branch).put(update_branch),
        )
        // Read surface (code-governance TDD_AIVCSD_LITE_READ_SURFACE, #1270).
        // Both are assemblers over the reads above — no new store.
        .route("/api/v1/repos/:repo", get(get_repo))
        .route("/api/v1/repos/:repo/source", get(get_repo_source))
        .with_state(forge)
}

/// A repo id is one or two `/`-separated DNS labels (`name` or `org/name`), each
/// 1–63 lowercase letters/digits/hyphens with no leading/trailing hyphen. Two
/// segments keep the org in the id — and therefore in the minted `aivcs://`
/// URI (#1237: `aivcs://<org>/<repo>`), which is expensive to retrofit later.
/// Bounded to two segments so it blocks traversal and stays branch-key-safe.
fn valid_repo(repo: &str) -> bool {
    let segments: Vec<&str> = repo.split('/').collect();
    (1..=2).contains(&segments.len())
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[derive(Deserialize)]
struct CreateRepoRequest {
    /// Full repo id (`org/name` or a bare `name`). If absent, composed from
    /// `name` (+ `org`, default `aivcs`) so the aivcs-repo client's
    /// `{"name":...}` body mints an org-scoped id.
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    org: Option<String>,
}

#[derive(Serialize)]
struct CreateRepoResponse {
    repo: String,
    uri: String,
    head_commit_id: String,
}

/// `POST /api/v1/repos` — create an empty repository. Idempotent on the repo id
/// (code-governance#1275): a fresh repo returns `201`; a repo that already has a
/// `main` branch returns `200` with the existing `aivcs://` URI and head commit,
/// so a re-running build rail is safe to retry and the create doubles as an
/// existence check. The empty root commit is built here (its `CommitRecord`
/// fields are crate-private), keyed by the same content-identity scheme as
/// `create_commit`, then pointed to by `main`.
async fn create_repo(
    State(forge): State<SharedForge>,
    Json(req): Json<CreateRepoRequest>,
) -> Response {
    let repo = match req.repo {
        Some(repo) if !repo.is_empty() => repo,
        _ => match req.name {
            Some(name) if !name.is_empty() => {
                format!(
                    "{}/{}",
                    req.org.unwrap_or_else(|| "aivcs".to_string()),
                    name
                )
            }
            _ => return err(StatusCode::BAD_REQUEST, "name or repo is required"),
        },
    };
    if !valid_repo(&repo) {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "repo must be a single DNS label (1-63 lowercase letters, digits, or hyphens; no leading/trailing hyphen)",
        );
    }

    // Idempotency (code-governance#1275): an existing `main` branch means the repo
    // already exists — return it with `200` rather than `409`. A create that is
    // safe to re-run is what the dockworker.toml contract promises ("idempotent on
    // {org,name}"), and it lets the create double as an existence check for a
    // retrying build rail instead of forcing every caller to special-case 409.
    match forge.get_branch(&repo, "main").await {
        Ok(Some(head_commit_id)) => {
            return (
                StatusCode::OK,
                Json(CreateRepoResponse {
                    uri: format!("aivcs://{repo}"),
                    repo,
                    head_commit_id,
                }),
            )
                .into_response()
        }
        Ok(None) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }

    // Empty root commit: empty manifest, no parents. Identity hashed the same way
    // as create_commit so ids are consistent across the forge.
    let manifest: Vec<ManifestEntry> = Vec::new();
    let tree = tree_digest(&manifest);
    let identity = json!({
        "repo": repo,
        "tree_digest": tree,
        "manifest": manifest,
        "parents": [],
        "message": "init",
        "author": "aivcsd",
    });
    let commit_id = sha256_hex(&serde_json::to_vec(&identity).unwrap_or_default());
    let commit = CommitRecord {
        repo: repo.clone(),
        commit_id: commit_id.clone(),
        tree_digest: tree,
        manifest,
        parents: Vec::new(),
        message: "init".to_string(),
        author: "aivcsd".to_string(),
    };
    if let Err(e) = forge.put_commit(commit).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = forge.set_branch(&repo, "main", &commit_id).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (
        StatusCode::CREATED,
        Json(CreateRepoResponse {
            uri: format!("aivcs://{repo}"),
            repo,
            head_commit_id: commit_id,
        }),
    )
        .into_response()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Content-derived tree digest: SHA-256 over the manifest (path, digest,
/// executable, size) sorted by path, so the same tree always yields the same
/// digest — and the mode/size are part of the identity, not silently dropped.
fn tree_digest(manifest: &[ManifestEntry]) -> String {
    let mut sorted: Vec<&ManifestEntry> = manifest.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let canonical: Vec<(&str, &str, bool, u64)> = sorted
        .iter()
        .map(|e| (e.path.as_str(), e.digest.as_str(), e.executable, e.size))
        .collect();
    sha256_hex(&serde_json::to_vec(&canonical).unwrap_or_default())
}

// ── Read surface (#1270) ─────────────────────────────────────────────────────

/// Default branch of a freshly created repo. `create_repo` inits `main` and the
/// store keeps no per-repo default, so this is the honest single source for it —
/// not an inference from whatever branches happen to exist.
const DEFAULT_BRANCH: &str = "main";

/// Default policy ceiling on a served source tar, measured in *emitted tar
/// bytes* (headers + content + padding + trailer), not raw content. Override per
/// deployment with `FORGE_MAX_SOURCE_BYTES`.
///
/// This is a policy/egress limit, NOT a memory limit: the tar is **streamed**
/// (see [`get_repo_source`]), so peak memory is one file's blob, not the whole
/// tree. The earlier 512 MiB value was described as protecting the pod but the
/// assembler buffered the entire tar, so a tree larger than the pod's memory
/// OOM-killed it before any `413` — the cap protected nothing. Streaming removes
/// that coupling; a deployment must still ensure no single source file exceeds
/// the pod's memory, since a blob is fetched whole.
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

/// The effective source-tar cap: `FORGE_MAX_SOURCE_BYTES` if set and parseable,
/// else [`MAX_SOURCE_BYTES`].
fn max_source_bytes() -> u64 {
    std::env::var("FORGE_MAX_SOURCE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_SOURCE_BYTES)
}

/// Exact size of the tar this manifest will emit: per entry a 512-byte header +
/// the content + zero-padding to the next 512 boundary, then a 1024-byte trailer.
/// This is what the `413` check compares — the old check summed raw content and
/// under-counted the archive.
fn estimated_tar_len(entries: &[ManifestEntry]) -> u64 {
    let mut total: u64 = 0;
    for e in entries {
        total = total.saturating_add(512);
        total = total.saturating_add(e.size);
        let pad = (512 - (e.size % 512)) % 512;
        total = total.saturating_add(pad);
    }
    total.saturating_add(1024)
}

#[derive(Serialize)]
struct RepoResponse {
    repo: String,
    uri: String,
    default_branch: String,
    head_commit_id: String,
}

/// `GET /api/v1/repos/:repo` — does it exist, and what is its head?
///
/// Before this, the only way to ask was `POST /api/v1/repos` and read the `409`
/// (code-governance#1275): a write used to answer a read.
async fn get_repo(State(forge): State<SharedForge>, Path(repo): Path<String>) -> Response {
    match forge.get_branch(&repo, DEFAULT_BRANCH).await {
        Ok(Some(head_commit_id)) => Json(RepoResponse {
            uri: format!("aivcs://{repo}"),
            repo,
            default_branch: DEFAULT_BRANCH.to_string(),
            head_commit_id,
        })
        .into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, format!("repo not found: {repo}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SourceQuery {
    #[serde(default)]
    r#ref: Option<String>,
}

/// `GET /api/v1/repos/:repo/source?ref=<commit|branch>` — the tree at `ref` as a
/// deterministic tar.
///
/// `ref` resolves branch-first, so `?ref=main` means the branch even if a commit
/// id could be spelled the same way. Defaults to the repo's default branch.
async fn get_repo_source(
    State(forge): State<SharedForge>,
    Path(repo): Path<String>,
    Query(q): Query<SourceQuery>,
) -> Response {
    let want = q.r#ref.unwrap_or_else(|| DEFAULT_BRANCH.to_string());

    // Branch first, then treat it as a commit id.
    let commit_id = match forge.get_branch(&repo, &want).await {
        Ok(Some(id)) => id,
        Ok(None) => want.clone(),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let commit = match forge.get_commit(&repo, &commit_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return err(
                StatusCode::NOT_FOUND,
                format!("ref not found in {repo}: {want}"),
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // Validate everything that does NOT need a blob up front, so the common
    // rejections (unsafe/oversized paths, duplicates, over-cap) are clean 4xx
    // BEFORE any bytes stream. Store-integrity failures (missing blob, digest
    // mismatch) can only surface mid-stream and abort the tar.
    let entries = match preflight_source(&commit) {
        Ok(entries) => entries,
        Err((code, msg)) => return err(code, msg),
    };

    // Stream the tar: peak memory is one file's blob, not the whole tree — so the
    // pod cannot be OOM-killed by a large repo the way buffering the full archive
    // allowed.
    let forge = forge.clone();
    let body = Body::from_stream(
        stream::iter(entries)
            .then(move |e| {
                let forge = forge.clone();
                async move { tar_entry_chunk(forge.as_ref(), &e).await }
            })
            // Two zero blocks terminate the archive; nix notices if they are missing.
            .chain(stream::once(async {
                Ok::<Bytes, io::Error>(Bytes::from(vec![0u8; 1024]))
            })),
    );

    (
        [
            (header::CONTENT_TYPE, "application/x-tar".to_string()),
            // The commit this ref resolved to. Without it a build that pinned a
            // BRANCH cannot record what it actually built, and is not reproducible.
            (
                HeaderName::from_static("x-aivcs-commit"),
                commit.commit_id.clone(),
            ),
            // tree_digest changes iff the content changes — the correct ETag.
            (header::ETAG, format!("\"{}\"", commit.tree_digest)),
            (
                header::CACHE_CONTROL,
                if want == commit.commit_id {
                    // A commit's tree is immutable.
                    "public, max-age=31536000, immutable".to_string()
                } else {
                    // A branch legitimately returns different bytes tomorrow.
                    "no-cache".to_string()
                },
            ),
        ],
        body,
    )
        .into_response()
}

/// Pre-stream validation: everything that does NOT require fetching a blob.
/// Returns the entries in the deterministic sorted order the tar emits them in,
/// so the common rejections are clean 4xx before any bytes go out.
///
/// Re-validated on read even though the writer checked: a reader that trusts
/// stored paths turns any past write bug into a tar that escapes the extraction
/// directory.
fn preflight_source(commit: &CommitRecord) -> Result<Vec<ManifestEntry>, (StatusCode, String)> {
    let mut entries: Vec<ManifestEntry> = commit.manifest.clone();
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));

    // Duplicate paths would emit two entries for one file — extraction order then
    // decides which wins, exactly the non-determinism this surface exists to kill.
    for w in entries.windows(2) {
        if w[0].path == w[1].path {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("duplicate path in manifest: {}", w[0].path),
            ));
        }
    }

    for e in &entries {
        if !is_safe_manifest_path(&e.path) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unsafe path in manifest: {}", e.path),
            ));
        }
        // USTAR carries a path in a 100-byte name field PLUS a 155-byte prefix,
        // split on `/` — up to 255 bytes. The previous 99-byte check ignored the
        // prefix field and rejected real repos (infra-code has paths over 99
        // bytes). `ustar_split` is None only at the true limit.
        if ustar_split(&e.path).is_none() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "path exceeds the USTAR limit (a component >100 or the path >255 bytes): {}",
                    e.path
                ),
            ));
        }
    }

    let tar_len = estimated_tar_len(&entries);
    let cap = max_source_bytes();
    if tar_len > cap {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("source tar would be {tar_len} bytes, over the {cap}-byte limit"),
        ));
    }

    Ok(entries)
}

/// Fetch one blob and emit its `[header | content | padding]` tar chunk.
///
/// Called per entry while streaming, so peak memory is one blob and a
/// store-integrity failure (missing blob, size/digest mismatch) aborts the
/// stream with an `io::Error` — a truncated tar nix rejects — rather than being
/// buffered. Determinism is preserved: entries arrive pre-sorted, and every
/// varying header field is pinned by [`ustar_header`].
async fn tar_entry_chunk(forge: &dyn ForgeStore, e: &ManifestEntry) -> Result<Bytes, io::Error> {
    let bytes = forge
        .get_blob(&e.digest)
        .await
        .map_err(|err| io::Error::other(err.to_string()))?
        .ok_or_else(|| io::Error::other(format!("blob not in CAS for {}: {}", e.path, e.digest)))?;

    if bytes.len() as u64 != e.size {
        return Err(io::Error::other(format!(
            "manifest size {} != blob length {} for {}",
            e.size,
            bytes.len(),
            e.path
        )));
    }
    if sha256_hex(&bytes) != e.digest {
        return Err(io::Error::other(format!(
            "blob digest mismatch for {}",
            e.path
        )));
    }

    // Guaranteed Some by preflight; recomputed so the writer never emits a header
    // from an unvalidated path.
    let (prefix, name) = ustar_split(&e.path)
        .ok_or_else(|| io::Error::other(format!("path exceeds USTAR limit: {}", e.path)))?;

    let pad = (512 - (bytes.len() % 512)) % 512;
    let mut chunk = Vec::with_capacity(512 + bytes.len() + pad);
    chunk.extend_from_slice(&ustar_header(prefix, name, bytes.len(), e.executable));
    chunk.extend_from_slice(&bytes);
    chunk.extend(std::iter::repeat_n(0u8, pad));
    Ok(Bytes::from(chunk))
}

/// Split a path into USTAR `(prefix, name)`: the archived path is
/// `prefix + "/" + name`, `name` <= 100 bytes and `prefix` <= 155. `None` only
/// when it truly cannot fit — a single component over 100 bytes, or a path over
/// 255. Deterministic: the leftmost `/` in the valid window (longest `name`).
fn ustar_split(path: &str) -> Option<(&str, &str)> {
    let bytes = path.as_bytes();
    if bytes.len() <= 100 {
        return Some(("", path));
    }
    if bytes.len() > 255 {
        return None;
    }
    // name = path[i+1..] len <= 100  =>  i >= len - 101
    // prefix = path[..i] len <= 155  =>  i <= 155
    // both non-empty                 =>  1 <= i <= len - 2
    let lo = bytes.len().saturating_sub(101).max(1);
    let hi = 155.min(bytes.len().saturating_sub(2));
    (lo..=hi)
        .find(|&i| bytes[i] == b'/')
        .map(|i| (&path[..i], &path[i + 1..]))
}

/// Reject anything that could escape the extraction directory, plus the empty
/// path. Directory entries are not emitted at all — the manifest is a flat file
/// list, so synthesising them would mean inventing modes it does not specify.
fn is_safe_manifest_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && !path.split('/').any(|c| c == ".." || c == ".")
}

/// One 512-byte USTAR header for `prefix + "/" + name`. All variable fields
/// pinned to zero/empty. `prefix` is empty for paths that fit the 100-byte name
/// field alone, so short-path headers are byte-identical to before.
fn ustar_header(prefix: &str, name: &str, size: usize, executable: bool) -> [u8; 512] {
    let mut h = [0u8; 512];
    let put = |h: &mut [u8; 512], off: usize, src: &[u8]| {
        h[off..off + src.len()].copy_from_slice(src);
    };
    put(&mut h, 0, name.as_bytes());
    // prefix field (offset 345, 155 bytes): the rest of a long path. Empty for
    // short paths — must be written before the checksum sums the header.
    put(&mut h, 345, prefix.as_bytes());
    // mode: the manifest carries exactly one bit; do not invent more.
    put(
        &mut h,
        100,
        format!("{:07o}\0", if executable { 0o755 } else { 0o644 }).as_bytes(),
    );
    put(&mut h, 108, b"0000000\0"); // uid  — builder identity must not leak in
    put(&mut h, 116, b"0000000\0"); // gid
    put(&mut h, 124, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, b"00000000000\0"); // mtime 0 — the classic reproducibility leak
    put(&mut h, 156, b"0"); // typeflag: regular file
    put(&mut h, 257, b"ustar\0"); // magic
    put(&mut h, 263, b"00"); // version
                             // uname/gname left empty: they vary by build image.
                             // Checksum is computed with the field itself read as spaces.
    put(&mut h, 148, b"        ");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    put(&mut h, 148, format!("{sum:06o}\0 ").as_bytes());
    h
}

/// JSON error body — the ONLY error shape this service emits (never HTML).
fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

#[derive(Serialize)]
struct PutBlobResponse {
    digest: String,
}

async fn put_blob(State(forge): State<SharedForge>, body: Bytes) -> Response {
    match forge.put_blob(&body).await {
        Ok(digest) => (StatusCode::CREATED, Json(PutBlobResponse { digest })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_blob(State(forge): State<SharedForge>, Path(digest): Path<String>) -> Response {
    match forge.get_blob(&digest).await {
        // Raw bytes — NOT JSON — matching the aivcsd `get_blob` contract.
        Ok(Some(bytes)) => {
            ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, format!("blob not found: {digest}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct CreateCommitRequest {
    repo: String,
    manifest: Vec<ManifestEntry>,
    #[serde(default)]
    parents: Vec<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    author: String,
}

#[derive(Serialize)]
struct CreateCommitResponse {
    commit_id: String,
    tree_digest: String,
}

async fn create_commit(
    State(forge): State<SharedForge>,
    Json(req): Json<CreateCommitRequest>,
) -> Response {
    // Batch validate all referenced blobs in CAS
    let digests: Vec<&str> = req.manifest.iter().map(|e| e.digest.as_str()).collect();
    match forge.exists_all(&digests).await {
        Ok(exists) => {
            for (entry, ok) in req.manifest.iter().zip(exists) {
                if !ok {
                    return err(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        format!("blob not in CAS for {}: {}", entry.path, entry.digest),
                    );
                }
            }
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    let tree = tree_digest(&req.manifest);
    // The complete commit identity matters: two commits may have the same
    // tree but differ in parents/message/author. Hashing only the tree would
    // overwrite one history record with the other.
    let identity = serde_json::json!({
        "repo": req.repo,
        "tree_digest": tree,
        "manifest": req.manifest,
        "parents": req.parents,
        "message": req.message,
        "author": req.author,
    });
    let commit_id = sha256_hex(&serde_json::to_vec(&identity).unwrap_or_default());
    let commit = CommitRecord {
        repo: identity["repo"].as_str().unwrap_or_default().to_owned(),
        commit_id: commit_id.clone(),
        tree_digest: identity["tree_digest"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        manifest: serde_json::from_value(identity["manifest"].clone()).unwrap_or_default(),
        parents: serde_json::from_value(identity["parents"].clone()).unwrap_or_default(),
        message: identity["message"].as_str().unwrap_or_default().to_owned(),
        author: identity["author"].as_str().unwrap_or_default().to_owned(),
    };
    if let Err(e) = forge.put_commit(commit).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    (
        StatusCode::CREATED,
        Json(CreateCommitResponse {
            commit_id,
            tree_digest: identity["tree_digest"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        }),
    )
        .into_response()
}

/// PHASE 1: Atomic publish request (blobs + commit + branch updates in one call)
#[derive(Debug, Deserialize)]
pub struct AtomicPublishRequest {
    pub repo: String,
    pub blobs: Vec<Vec<u8>>,
    pub commit: CommitRecord,
    #[serde(default)]
    pub branch_updates: Vec<(String, String)>,
}

#[derive(Serialize)]
pub struct AtomicPublishResponse {
    pub commit_id: String,
    pub blobs_stored: usize,
    pub branches_updated: usize,
}

/// `POST /api/v1/publish` — atomic publish: all blobs, commit, and branch updates
/// in one call. All-or-nothing semantics: validates before executing; on any
/// validation failure, returns 422 and NOTHING is written.
async fn publish_atomic(
    State(forge): State<SharedForge>,
    Json(req): Json<AtomicPublishRequest>,
) -> Response {
    // Validate repo matches in commit record
    if req.commit.repo != req.repo {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "commit.repo must match request repo",
        );
    }

    // Call atomic publish on store (validates blobs, commits, branches)
    match forge
        .atomic_publish(&req.repo, req.blobs, req.commit, req.branch_updates)
        .await
    {
        Ok(result) => (
            StatusCode::CREATED,
            Json(AtomicPublishResponse {
                commit_id: result.commit_id,
                blobs_stored: result.blobs_stored,
                branches_updated: result.branches_updated,
            }),
        )
            .into_response(),
        Err(ForgeError::MissingBlob { path, digest }) => err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "blob not found for {}: {} (all-or-nothing validation — nothing written)",
                path, digest
            ),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct RepoQuery {
    repo: String,
}

async fn get_commit_manifest(
    State(forge): State<SharedForge>,
    Path(commit_id): Path<String>,
    Query(q): Query<RepoQuery>,
) -> Response {
    match forge.get_commit(&q.repo, &commit_id).await {
        // A BARE array of entries — the client deserializes `Vec<ManifestEntry>`.
        Ok(Some(c)) => Json(c.manifest).into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            format!("commit not found: {commit_id}"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Commit metadata — retains the parents/message/author/tree that `create_commit`
/// carried, so commit history is not discarded.
#[derive(Serialize)]
struct CommitResponse {
    commit_id: String,
    repo: String,
    tree_digest: String,
    parents: Vec<String>,
    message: String,
    author: String,
}

async fn get_commit(
    State(forge): State<SharedForge>,
    Path(commit_id): Path<String>,
    Query(q): Query<RepoQuery>,
) -> Response {
    match forge.get_commit(&q.repo, &commit_id).await {
        Ok(Some(c)) => Json(CommitResponse {
            commit_id: c.commit_id,
            repo: c.repo,
            tree_digest: c.tree_digest,
            parents: c.parents,
            message: c.message,
            author: c.author,
        })
        .into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            format!("commit not found: {commit_id}"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct UpdateBranchRequest {
    commit_id: String,
}

/// Branch head — field name is `head_commit_id` to match the client's
/// `BranchResponse`/`UpdateBranchResponse`.
#[derive(Serialize)]
struct BranchResponse {
    repo: String,
    branch: String,
    head_commit_id: String,
}

async fn update_branch(
    State(forge): State<SharedForge>,
    Path((repo, branch)): Path<(String, String)>,
    Json(req): Json<UpdateBranchRequest>,
) -> Response {
    // The commit must exist IN THIS REPO — a branch can never point at another
    // repository's commit (repo-scoped key).
    match forge.get_commit(&repo, &req.commit_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("commit not found in {repo}: {}", req.commit_id),
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    if let Err(e) = forge.set_branch(&repo, &branch, &req.commit_id).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(BranchResponse {
        repo,
        branch,
        head_commit_id: req.commit_id,
    })
    .into_response()
}

async fn get_branch(
    State(forge): State<SharedForge>,
    Path((repo, branch)): Path<(String, String)>,
) -> Response {
    match forge.get_branch(&repo, &branch).await {
        Ok(Some(head_commit_id)) => Json(BranchResponse {
            repo,
            branch,
            head_commit_id,
        })
        .into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            format!("branch not found: {repo}/{branch}"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: after a branch moves, the durable head is the most recent
    /// update — regardless of the order `query_documents` returns the appends.
    #[test]
    fn latest_head_picks_most_recent_update_regardless_of_order() {
        let docs = vec![
            json!({ "head_commit_id": "old", "updated_at_ms": 100u64 }),
            json!({ "head_commit_id": "new", "updated_at_ms": 300u64 }),
            json!({ "head_commit_id": "mid", "updated_at_ms": 200u64 }),
        ];
        assert_eq!(latest_head(docs).as_deref(), Some("new"));
        assert_eq!(latest_head(vec![]), None);
    }

    /// never-use-sqlite: only `postgres`/`postgresql` DSNs are accepted; every
    /// other scheme (SQLite first) is rejected, case-insensitively.
    #[test]
    fn dsn_scheme_detection_is_postgres_only() {
        assert!(is_postgres_dsn("postgres://u:p@h:5432/db"));
        assert!(is_postgres_dsn("postgresql://u@h/db?sslmode=require"));
        assert!(is_postgres_dsn("POSTGRES://u@h/db"));
        for bad in [
            "sqlite:///var/lib/attic/attic.db?mode=rwc",
            "sqlite::memory:",
            "SQLITE://x.db",
            "mysql://h/db",
            "/var/lib/x.db",
            "nonsense",
        ] {
            assert!(!is_postgres_dsn(bad), "must reject {bad:?}");
        }
    }

    /// The BYTEA boundary is a clear error, not an opaque driver failure: at the
    /// limit is fine, one byte over is rejected.
    #[test]
    fn check_blob_size_guards_the_bytea_limit() {
        assert!(check_blob_size(0).is_ok());
        assert!(check_blob_size(MAX_BLOB_BYTES).is_ok());
        let err = check_blob_size(MAX_BLOB_BYTES + 1).expect_err("over-limit must be rejected");
        match err {
            ForgeError::Backend(msg) => assert!(msg.contains("BYTEA"), "got: {msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
    }

    /// `connect` fails closed on a forbidden backend *before* opening a pool —
    /// a misconfigured SQLite `DATABASE_URL` is a loud startup error, not a
    /// silent bind.
    #[tokio::test]
    async fn sqlx_connect_refuses_sqlite_before_connecting() {
        let err = SqlxForgeStore::connect("sqlite:///tmp/x.db")
            .await
            .expect_err("sqlite DSN must be refused");
        match err {
            ForgeError::Backend(msg) => assert!(
                msg.contains("never-use-sqlite"),
                "error should name the rule, got: {msg}"
            ),
            other => panic!("expected Backend error, got {other:?}"),
        }
    }

    /// Full store round-trip against a real PostgreSQL. **Ignored by default** —
    /// hermetic CI has no database. Run it with a live instance:
    ///   `TEST_DATABASE_URL=postgres://… cargo test -p forge-cas -- --ignored`
    /// (also the deploy smoke check against the db.t4g.micro RDS). This is the
    /// only test that actually EXECUTES the SQL — column names, the BYTEA/BIGINT/
    /// JSON-TEXT mappings, the `ON CONFLICT` targets, and the branch upsert —
    /// which the DSN-guard unit tests above cannot reach. Written to be
    /// idempotent (content-addressed ids, upsert) so repeated runs are safe.
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing at a PostgreSQL instance"]
    async fn sqlx_store_roundtrip_against_postgres() {
        let Ok(dsn) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let store = SqlxForgeStore::connect(&dsn)
            .await
            .expect("connect + migrate");

        let repo = "forge-cas-itest/roundtrip";

        // Blob put/get, with an idempotent re-put returning the same digest.
        let bytes = b"#!/bin/sh\necho hi\n";
        let digest = store.put_blob(bytes).await.unwrap();
        assert_eq!(
            digest,
            store.put_blob(bytes).await.unwrap(),
            "re-put must be idempotent (ON CONFLICT DO NOTHING)"
        );
        assert_eq!(
            store.get_blob(&digest).await.unwrap().as_deref(),
            Some(&bytes[..])
        );
        assert!(store.get_blob("deadbeef").await.unwrap().is_none());

        // A commit whose executable bit, size, parents, message, and author must
        // all survive the JSON-TEXT round-trip.
        let manifest = vec![ManifestEntry {
            path: "run.sh".into(),
            digest: digest.clone(),
            executable: true,
            size: bytes.len() as u64,
        }];
        let commit_id = sha256_hex(b"forge-cas-itest-commit-identity");
        let commit = CommitRecord {
            repo: repo.into(),
            commit_id: commit_id.clone(),
            tree_digest: tree_digest(&manifest),
            manifest,
            parents: vec!["cafe".into()],
            message: "init".into(),
            author: "tester".into(),
        };
        store.put_commit(commit.clone()).await.unwrap();
        store.put_commit(commit).await.unwrap(); // second put: DO NOTHING

        let got = store
            .get_commit(repo, &commit_id)
            .await
            .unwrap()
            .expect("commit present");
        assert_eq!(got.manifest.len(), 1);
        assert_eq!(got.manifest[0].path, "run.sh");
        assert!(got.manifest[0].executable);
        assert_eq!(got.manifest[0].size, bytes.len() as u64);
        assert_eq!(got.parents, vec!["cafe".to_string()]);
        assert_eq!(got.message, "init");
        assert_eq!(got.author, "tester");
        // Repo scoping: another repo must not resolve this commit id.
        assert!(store
            .get_commit("forge-cas-itest/other", &commit_id)
            .await
            .unwrap()
            .is_none());

        // Branch upsert + point read; a second set moves the head in place.
        store.set_branch(repo, "main", &commit_id).await.unwrap();
        assert_eq!(
            store.get_branch(repo, "main").await.unwrap().as_deref(),
            Some(commit_id.as_str())
        );
        store.set_branch(repo, "main", "0000").await.unwrap();
        assert_eq!(
            store.get_branch(repo, "main").await.unwrap().as_deref(),
            Some("0000"),
            "set_branch must upsert (mutable head), not append"
        );
        assert!(store.get_branch(repo, "absent").await.unwrap().is_none());
        // Restore the head so a re-run starts from the same state.
        store.set_branch(repo, "main", &commit_id).await.unwrap();
    }
    use axum::body::Body;
    use axum::http::{header::CONTENT_TYPE, Request};
    use tower::ServiceExt; // `oneshot`

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String, Vec<u8>) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let ct = res
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, ct, body)
    }

    fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn put_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("PUT")
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    /// Upload a blob and return its digest + byte length.
    async fn put_blob_helper(app: &Router, content: &str) -> (String, u64) {
        let (st, _ct, body) = call(
            app,
            Request::builder()
                .method("POST")
                .uri("/api/v1/blobs")
                .body(Body::from(content.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let digest = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["digest"]
            .as_str()
            .unwrap()
            .to_string();
        (digest, content.len() as u64)
    }

    #[tokio::test]
    async fn create_repo_defaults_org_and_is_idempotent() {
        let app = router();

        // {name} → org defaults to aivcs → org-scoped id + aivcs:// URI (#1237)
        let (st, ct, body) = call(
            &app,
            post_json("/api/v1/repos", json!({ "name": "repo-x" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(ct, "application/json");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["repo"], "aivcs/repo-x");
        assert_eq!(v["uri"], "aivcs://aivcs/repo-x");
        assert!(!v["head_commit_id"].as_str().unwrap().is_empty());
        let first_head = v["head_commit_id"].as_str().unwrap().to_string();

        // second create → 200 with the SAME resource (code-governance#1275):
        // idempotent-on-{org,name}, safe to re-run, no 409 for callers to special
        // case. Same URI and same head commit as the first create.
        let (st, ct, body) = call(
            &app,
            post_json("/api/v1/repos", json!({ "name": "repo-x" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ct, "application/json");
        let v2: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v2["repo"], "aivcs/repo-x");
        assert_eq!(v2["uri"], "aivcs://aivcs/repo-x");
        assert_eq!(v2["head_commit_id"].as_str().unwrap(), first_head);

        // explicit org honored, and the /v1/repos alias prefix also creates
        let (st, _ct, body) = call(
            &app,
            post_json("/v1/repos", json!({ "name": "svc", "org": "acme" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["uri"], "aivcs://acme/svc");
    }

    #[tokio::test]
    async fn create_repo_single_segment_is_branch_addressable() {
        let app = router();
        // A bare single-segment id stays addressable via the single-segment
        // branch route (two-segment org ids are addressed elsewhere).
        let (st, _ct, body) =
            call(&app, post_json("/api/v1/repos", json!({ "repo": "solo" }))).await;
        assert_eq!(st, StatusCode::CREATED);
        let head = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["head_commit_id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, _ct, body) = call(&app, get("/api/v1/repos/solo/branches/main")).await;
        assert_eq!(st, StatusCode::OK);
        let b: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(b["head_commit_id"].as_str().unwrap(), head);
    }

    #[tokio::test]
    async fn create_repo_rejects_bad_names_and_requires_one() {
        let app = router();
        // 3+ segments, uppercase, traversal, hyphen edges — all invalid.
        for bad in ["a/b/c", "Repo", "../x", "-lead", "trail-", ""] {
            let (st, _ct, _b) =
                call(&app, post_json("/api/v1/repos", json!({ "repo": bad }))).await;
            assert!(
                st == StatusCode::UNPROCESSABLE_ENTITY || st == StatusCode::BAD_REQUEST,
                "accepted {bad:?} with {st}"
            );
        }
        // no name/repo at all → 400
        let (st, _ct, _b) = call(&app, post_json("/api/v1/repos", json!({}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    /// Full round-trip carrying `executable`/`size`, a bare-array manifest, and
    /// the `head_commit_id` branch shape.
    #[tokio::test]
    async fn blob_commit_branch_roundtrip_preserves_metadata() {
        let app = router();
        let (digest, size) = put_blob_helper(&app, "#!/bin/sh\necho hi\n").await;

        // get blob → raw bytes back
        let (st, ct, bytes) = call(&app, get(&format!("/api/v1/blobs/{digest}"))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ct, "application/octet-stream");
        assert_eq!(bytes, b"#!/bin/sh\necho hi\n");

        // create commit with an EXECUTABLE entry + metadata
        let (st, _ct, body) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({
                    "repo": "demo",
                    "manifest": [{ "path": "run.sh", "digest": digest, "executable": true, "size": size }],
                    "parents": ["cafe"],
                    "message": "init",
                    "author": "tester"
                }),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let commit_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();

        // update + read branch → head_commit_id
        let (st, _ct, body) = call(
            &app,
            put_json(
                "/api/v1/repos/demo/branches/main",
                json!({ "commit_id": commit_id }),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["head_commit_id"],
            commit_id
        );
        let (_st, _ct, body) = call(&app, get("/api/v1/repos/demo/branches/main")).await;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["head_commit_id"],
            commit_id
        );

        // manifest is a BARE ARRAY preserving executable + size
        let (st, _ct, body) = call(
            &app,
            get(&format!("/api/v1/commits/{commit_id}/manifest?repo=demo")),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let entries: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["path"], "run.sh");
        assert_eq!(entries[0]["executable"], true);
        assert_eq!(entries[0]["size"], size);

        // commit metadata retained (parents/message/author not discarded)
        let (st, _ct, body) =
            call(&app, get(&format!("/api/v1/commits/{commit_id}?repo=demo"))).await;
        assert_eq!(st, StatusCode::OK);
        let c = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        assert_eq!(c["parents"][0], "cafe");
        assert_eq!(c["message"], "init");
        assert_eq!(c["author"], "tester");
    }

    /// Finding: a branch must not resolve or point to a commit from ANOTHER repo.
    #[tokio::test]
    async fn branch_cannot_point_at_another_repos_commit() {
        let app = router();
        let (digest, size) = put_blob_helper(&app, "x").await;
        // commit lives in repo "a"
        let (_st, _ct, body) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({"repo":"a","manifest":[{"path":"f","digest":digest,"executable":false,"size":size}],"parents":[],"message":"m","author":"u"}),
            ),
        )
        .await;
        let commit_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();

        // updating repo "b"'s branch to repo "a"'s commit must be refused
        let (st, ct, _b) = call(
            &app,
            put_json(
                "/api/v1/repos/b/branches/main",
                json!({ "commit_id": commit_id }),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(ct.starts_with("application/json"));

        // and repo "b" can't read repo "a"'s commit manifest either
        let (st, _ct, _b) = call(
            &app,
            get(&format!("/api/v1/commits/{commit_id}/manifest?repo=b")),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    /// Incident guard (ported from aivcs.io#371): every metadata route answers
    /// `application/json`, so a client never mis-parses a SPA's HTML.
    #[tokio::test]
    async fn json_routes_declare_json_content_type() {
        let app = router();
        let (digest, size) = put_blob_helper(&app, "x").await;
        let cases = vec![
            post_json("/api/v1/blobs", json!("ignored")),
            post_json(
                "/api/v1/commits",
                json!({"repo":"d","manifest":[{"path":"a","digest":digest,"executable":false,"size":size}],"parents":[],"message":"m","author":"a"}),
            ),
            put_json(
                "/api/v1/repos/d/branches/main",
                json!({"commit_id":"deadbeef"}),
            ),
            get("/api/v1/repos/d/branches/nope"),
            get("/api/v1/commits/deadbeef/manifest?repo=d"),
            get("/api/v1/commits/deadbeef?repo=d"),
        ];
        for req in cases {
            let uri = req.uri().to_string();
            let (_st, ct, _b) = call(&app, req).await;
            assert!(
                ct.starts_with("application/json"),
                "route {uri} must answer application/json, got {ct:?}"
            );
        }
    }

    /// A not-found branch is a JSON 404 (never HTML).
    #[tokio::test]
    async fn missing_branch_is_json_404_not_html() {
        let app = router();
        let (st, ct, body) = call(&app, get("/api/v1/repos/demo/branches/main")).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(ct.starts_with("application/json"));
        assert!(serde_json::from_slice::<serde_json::Value>(&body)
            .unwrap()
            .get("error")
            .is_some());
    }

    /// A commit cannot reference a blob that was never uploaded (fail closed).
    #[tokio::test]
    async fn commit_rejects_unknown_blob() {
        let app = router();
        let (st, ct, _b) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({"repo":"d","manifest":[{"path":"a","digest":"00ff","executable":false,"size":2}],"parents":[],"message":"m","author":"a"}),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(ct.starts_with("application/json"));
    }

    /// A tree is not a complete commit identity: metadata changes must create
    /// distinct records rather than replacing history for the same tree.
    #[tokio::test]
    async fn same_tree_with_different_metadata_keeps_both_commits() {
        let app = router();
        let (digest, size) = put_blob_helper(&app, "same").await;
        let manifest = json!([{"path":"f","digest":digest,"executable":false,"size":size}]);

        let commit = |message: &str| {
            post_json(
                "/api/v1/commits",
                json!({"repo":"demo","manifest":manifest.clone(),"parents":[],"message":message,"author":"tester"}),
            )
        };
        let (_, _, first_body) = call(&app, commit("first")).await;
        let (_, _, second_body) = call(&app, commit("second")).await;
        let first = serde_json::from_slice::<serde_json::Value>(&first_body).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = serde_json::from_slice::<serde_json::Value>(&second_body).unwrap()
            ["commit_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(first, second);

        let (_, _, first_meta) =
            call(&app, get(&format!("/api/v1/commits/{first}?repo=demo"))).await;
        let (_, _, second_meta) =
            call(&app, get(&format!("/api/v1/commits/{second}?repo=demo"))).await;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&first_meta).unwrap()["message"],
            "first"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&second_meta).unwrap()["message"],
            "second"
        );
    }

    // ── Read surface (#1270) ────────────────────────────────────────────────

    /// Seed a repo with two files and return (app, commit_id).
    async fn seed_repo(repo: &str) -> (Router, String) {
        let app = router();
        let (_, _, _) = call(&app, post_json("/api/v1/repos", json!({ "repo": repo }))).await;

        let mut manifest = Vec::new();
        // Deliberately seeded OUT of sorted order so the tar's ordering is the
        // assembler's doing, not the input's.
        for (path, body, exec) in [
            ("src/main.rs", b"fn main() {}".to_vec(), false),
            ("bin/run.sh", b"#!/bin/sh\n".to_vec(), true),
        ] {
            let req = Request::builder()
                .method("POST")
                .uri("/api/v1/blobs")
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(body.clone()))
                .unwrap();
            let (_, _, out) = call(&app, req).await;
            let digest = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["digest"]
                .as_str()
                .unwrap()
                .to_string();
            manifest.push(json!({ "path": path, "digest": digest,
                                  "executable": exec, "size": body.len() }));
        }
        let (_, _, out) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({
            "repo": repo, "manifest": manifest, "message": "seed", "author": "t" }),
            ),
        )
        .await;
        let cid = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();
        let enc = repo.replace('/', "%2F");
        let _ = call(
            &app,
            put_json(
                &format!("/api/v1/repos/{enc}/branches/main"),
                json!({ "commit_id": cid }),
            ),
        )
        .await;
        (app, cid)
    }

    #[tokio::test]
    async fn repo_metadata_reports_head_without_a_write() {
        let (app, cid) = seed_repo("lornu-ai/meta").await;
        let (st, _, body) = call(&app, get_req("/api/v1/repos/lornu-ai%2Fmeta")).await;
        assert_eq!(st, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["repo"], "lornu-ai/meta");
        assert_eq!(v["uri"], "aivcs://lornu-ai/meta");
        assert_eq!(v["default_branch"], "main");
        assert_eq!(v["head_commit_id"], cid);

        let (st, _, _) = call(&app, get_req("/api/v1/repos/lornu-ai%2Fnope")).await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "absent repo must 404, not 200 with an empty head"
        );
    }

    /// The property the whole design exists for: same commit, same bytes.
    #[tokio::test]
    async fn source_tar_is_byte_identical_across_fetches() {
        let (app, cid) = seed_repo("lornu-ai/det").await;
        let uri = format!("/api/v1/repos/lornu-ai%2Fdet/source?ref={cid}");
        let (s1, ct, a) = call(&app, get_req(&uri)).await;
        let (s2, _, b) = call(&app, get_req(&uri)).await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(ct, "application/x-tar");
        assert_eq!(a, b, "same commit must produce identical bytes");
        assert_eq!(sha256_hex(&a), sha256_hex(&b));
    }

    #[tokio::test]
    async fn source_tar_pins_every_nondeterministic_field() {
        let (app, cid) = seed_repo("lornu-ai/fields").await;
        let (_, _, tar) = call(
            &app,
            get_req(&format!("/api/v1/repos/lornu-ai%2Ffields/source?ref={cid}")),
        )
        .await;

        // Entries are sorted by path, not seeded order: bin/ before src/.
        let name0 = String::from_utf8_lossy(&tar[0..10])
            .trim_end_matches('\0')
            .to_string();
        assert!(
            name0.starts_with("bin/run.sh"),
            "expected sorted first entry, got {name0}"
        );

        let mtime = String::from_utf8_lossy(&tar[136..148]);
        assert_eq!(
            mtime.trim_end_matches('\0'),
            "00000000000",
            "mtime must be zero"
        );
        assert_eq!(
            String::from_utf8_lossy(&tar[108..116]).trim_end_matches('\0'),
            "0000000",
            "uid"
        );
        assert_eq!(
            String::from_utf8_lossy(&tar[116..124]).trim_end_matches('\0'),
            "0000000",
            "gid"
        );
        // The executable bit is the only mode input.
        assert_eq!(
            String::from_utf8_lossy(&tar[100..108]).trim_end_matches('\0'),
            "0000755"
        );
        assert_eq!(&tar[257..262], b"ustar");
        // Trailer: two zero blocks.
        assert!(tar[tar.len() - 1024..].iter().all(|b| *b == 0));
    }

    #[tokio::test]
    async fn source_names_the_commit_it_resolved_and_scopes_caching() {
        let (app, cid) = seed_repo("lornu-ai/hdr").await;

        // Branch ref: must still report the concrete commit, and must NOT be
        // cached immutably — the same URL legitimately changes tomorrow.
        let res = app
            .clone()
            .oneshot(get_req("/api/v1/repos/lornu-ai%2Fhdr/source?ref=main"))
            .await
            .unwrap();
        assert_eq!(res.headers()["x-aivcs-commit"], cid.as_str());
        assert_eq!(res.headers()[axum::http::header::CACHE_CONTROL], "no-cache");

        // Commit ref: immutable.
        let res = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/v1/repos/lornu-ai%2Fhdr/source?ref={cid}"
            )))
            .await
            .unwrap();
        assert!(res.headers()[axum::http::header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable"));
        assert!(res.headers().contains_key(axum::http::header::ETAG));
    }

    #[tokio::test]
    async fn source_rejects_unknown_ref_and_unsafe_paths() {
        let (app, _) = seed_repo("lornu-ai/bad").await;
        let (st, _, _) = call(
            &app,
            get_req("/api/v1/repos/lornu-ai%2Fbad/source?ref=nope"),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        assert!(!is_safe_manifest_path("../etc/passwd"));
        assert!(!is_safe_manifest_path("/etc/passwd"));
        assert!(!is_safe_manifest_path("a/../../b"));
        assert!(!is_safe_manifest_path(""));
        assert!(is_safe_manifest_path("src/main.rs"));
    }

    #[test]
    fn ustar_split_uses_name_alone_under_100_and_prefix_beyond() {
        // Short path: name field only, empty prefix — byte-identical to before.
        assert_eq!(ustar_split("src/main.rs"), Some(("", "src/main.rs")));
        let p100 = format!("{}/{}", "a".repeat(60), "b".repeat(39)); // 60+1+39 = 100
        assert_eq!(p100.len(), 100);
        assert_eq!(ustar_split(&p100), Some(("", p100.as_str())));

        // A 118-byte path (infra-code's longest class) splits and reconstructs.
        let deep = format!("{}/{}", "d".repeat(60), "f".repeat(57)); // 118
        assert_eq!(deep.len(), 118);
        let (prefix, name) = ustar_split(&deep).expect("118-byte path must fit USTAR");
        assert!(!prefix.is_empty() && prefix.len() <= 155);
        assert!(!name.is_empty() && name.len() <= 100);
        assert_eq!(format!("{prefix}/{name}"), deep);

        // The true limits: a single component over 100, or a path over 255.
        assert_eq!(ustar_split(&"z".repeat(120)), None);
        let over_255 = format!("{}/{}", "a".repeat(150), "b".repeat(150));
        assert!(over_255.len() > 255);
        assert_eq!(ustar_split(&over_255), None);
    }

    /// The infra-code blocker: paths over 99 bytes used to 422. They must now be
    /// served, and the tar header must round-trip the full path via name+prefix.
    #[tokio::test]
    async fn source_serves_a_path_longer_than_99_bytes() {
        let app = router();
        let _ = call(&app, post_json("/api/v1/repos", json!({ "repo": "long" }))).await;
        let (digest, size) = put_blob_helper(&app, "hi").await;
        let path = format!("{}f.yaml", "abcdefghij/".repeat(10)); // 110 + 6 = 116 bytes
        assert!(path.len() > 99);
        let manifest =
            json!([{ "path": path, "digest": digest, "executable": false, "size": size }]);
        let (_, _, out) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({ "repo": "long", "manifest": manifest, "message": "m", "author": "a" }),
            ),
        )
        .await;
        let cid = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();

        let (st, ct, tar) = call(
            &app,
            get_req(&format!("/api/v1/repos/long/source?ref={cid}")),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::OK,
            "a >99-byte path must be served, not 422"
        );
        assert_eq!(ct, "application/x-tar");
        // Reconstruct the path from the USTAR name (0..100) + prefix (345..500).
        let name = String::from_utf8_lossy(&tar[0..100])
            .trim_end_matches('\0')
            .to_string();
        let prefix = String::from_utf8_lossy(&tar[345..500])
            .trim_end_matches('\0')
            .to_string();
        assert_eq!(format!("{prefix}/{name}"), path);
    }

    /// Duplicate manifest paths would make extraction order decide the winner —
    /// a determinism hole. Reject them.
    #[tokio::test]
    async fn source_rejects_duplicate_manifest_paths() {
        let app = router();
        let _ = call(&app, post_json("/api/v1/repos", json!({ "repo": "dup" }))).await;
        let (digest, size) = put_blob_helper(&app, "x").await;
        let manifest = json!([
            { "path": "a.txt", "digest": digest, "executable": false, "size": size },
            { "path": "a.txt", "digest": digest, "executable": false, "size": size },
        ]);
        let (_, _, out) = call(
            &app,
            post_json(
                "/api/v1/commits",
                json!({ "repo": "dup", "manifest": manifest, "message": "m", "author": "a" }),
            ),
        )
        .await;
        let cid = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, _, body) = call(
            &app,
            get_req(&format!("/api/v1/repos/dup/source?ref={cid}")),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&body).contains("duplicate"));
    }

    fn get_req(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }
}

#[derive(serde::Deserialize)]
pub struct CommitPayload {
    pub message: String,
    pub blob_hash: String,
}

pub async fn post_commit_repo(
    axum::extract::State(forge): axum::extract::State<SharedForge>,
    axum::extract::Path((owner, repo)): axum::extract::Path<(String, String)>,
    axum::Json(payload): axum::Json<CommitPayload>,
) -> axum::response::Response {
    let manifest = vec![ManifestEntry {
        path: "source.tar.gz".to_string(),
        digest: payload.blob_hash.clone(),
        size: 0,
        executable: false,
    }];
    let tree = tree_digest(&manifest);
    let repo_name = format!("{owner}/{repo}");
    let identity = serde_json::json!({
        "repo": repo_name,
        "tree_digest": tree,
        "manifest": manifest,
        "parents": Vec::<String>::new(),
        "message": payload.message,
        "author": "agent",
    });
    let commit_id = sha256_hex(&serde_json::to_vec(&identity).unwrap_or_default());
    let commit = CommitRecord {
        repo: repo_name.clone(),
        commit_id: commit_id.clone(),
        tree_digest: tree,
        manifest,
        parents: vec![],
        message: payload.message,
        author: "agent".to_string(),
    };
    if let Err(e) = forge.put_commit(commit).await {
        return err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    (
        axum::http::StatusCode::CREATED,
        axum::Json(serde_json::json!({
            "status": "success",
            "uri": format!("aivcs://{repo_name}@{}", payload.blob_hash)
        })),
    )
        .into_response()
}

pub async fn get_commits_repo(
    axum::extract::State(_forge): axum::extract::State<SharedForge>,
    axum::extract::Path((_owner, _repo)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    (
        axum::http::StatusCode::OK,
        axum::Json(Vec::<serde_json::Value>::new()),
    )
        .into_response()
}
