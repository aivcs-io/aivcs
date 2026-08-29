//! SurrealDB-backed RunLedger implementation
//!
//! Uses `schema::RunRecord` and `schema::RunEventRecord` for persistence,
//! converting to/from `storage_traits` types at the boundary.

use async_trait::async_trait;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use tracing::{debug, info};

use crate::error::{StateError, StorageError};
use crate::migrations;
use crate::schema::RunEventRecord as DbEvent;
use crate::schema::RunRecord as DbRun;
use crate::storage_traits::{
    ContentDigest, EvaluationMetadata, RunEvent, RunId, RunLedger, RunMetadata, RunRecord,
    RunStatus, RunSummary, StorageResult,
};

/// SurrealDB-backed implementation of [`RunLedger`].
pub struct SurrealRunLedger {
    db: Surreal<Any>,
    _tempdir: Option<tempfile::TempDir>,
}

impl SurrealRunLedger {
    /// Create an in-memory instance for testing.
    ///
    /// Connects to a unique temp `surrealkv://` database, selects `aivcs/main`,
    /// and runs `init_schema`.
    pub async fn in_memory() -> crate::Result<Self> {
        let tempdir = tempfile::tempdir().map_err(|e| {
            StateError::Connection(format!("Failed to create temp database directory: {e}"))
        })?;
        let url = format!("surrealkv://{}", tempdir.path().display());

        let db = surrealdb::engine::any::connect(&url)
            .await
            .map_err(|e| StateError::Connection(e.to_string()))?;

        db.use_ns("aivcs")
            .use_db("main")
            .await
            .map_err(|e| StateError::Connection(e.to_string()))?;

        migrations::init_schema(&db).await?;

        info!("SurrealRunLedger connected (in-memory, {})", url);
        Ok(Self {
            db,
            _tempdir: Some(tempdir),
        })
    }

    /// Create from environment variables.
    ///
    /// Uses the same env-var chain as [`crate::handle::SurrealHandle::setup_from_env`].
    pub async fn from_env() -> crate::Result<Self> {
        use crate::handle::CloudConfig;
        use surrealdb::opt::auth::{Database, Root};

        if let Ok(config) = CloudConfig::from_env() {
            let db = surrealdb::engine::any::connect(&config.endpoint)
                .await
                .map_err(|e| StateError::Connection(e.to_string()))?;

            if config.is_root {
                db.signin(Root {
                    username: &config.username,
                    password: &config.password,
                })
                .await
                .map_err(|e| StateError::Connection(format!("Root auth failed: {e}")))?;
            } else {
                db.signin(Database {
                    namespace: &config.namespace,
                    database: &config.database,
                    username: &config.username,
                    password: &config.password,
                })
                .await
                .map_err(|e| StateError::Connection(format!("DB auth failed: {e}")))?;
            }

            db.use_ns(&config.namespace)
                .use_db(&config.database)
                .await
                .map_err(|e| StateError::Connection(e.to_string()))?;

            migrations::init_schema(&db).await?;
            info!("SurrealRunLedger connected (cloud)");
            return Ok(Self { db, _tempdir: None });
        }

        if let Ok(url) = std::env::var("SURREALDB_URL") {
            let db = surrealdb::engine::any::connect(&url)
                .await
                .map_err(|e| StateError::Connection(e.to_string()))?;

            db.use_ns("aivcs")
                .use_db("main")
                .await
                .map_err(|e| StateError::Connection(e.to_string()))?;

            migrations::init_schema(&db).await?;
            info!("SurrealRunLedger connected ({})", url);
            return Ok(Self { db, _tempdir: None });
        }

        // Default to local persistence in .aivcs/db
        let path = ".aivcs/db";
        std::fs::create_dir_all(path).map_err(|e| {
            StateError::Connection(format!(
                "Failed to create database directory {}: {}",
                path, e
            ))
        })?;
        let url = format!("surrealkv://{}", path);
        info!(
            "No cloud config or SURREALDB_URL found, using local persistence: {}",
            url
        );

        let db = surrealdb::engine::any::connect(&url)
            .await
            .map_err(|e| StateError::Connection(format!("Failed to connect to {}: {}", url, e)))?;

        db.use_ns("aivcs")
            .use_db("main")
            .await
            .map_err(|e| StateError::Connection(e.to_string()))?;

        migrations::init_schema(&db).await?;
        Ok(Self { db, _tempdir: None })
    }

    // -- private helpers -----------------------------------------------------

    /// Fetch a run row by ID (owned string), returning the DB row or RunNotFound.
    async fn fetch_run(&self, rid: &str) -> StorageResult<DbRun> {
        let rid_owned = rid.to_string();
        let mut res = self
            .db
            .query(
                "SELECT run_id, spec_digest, git_sha, agent_name, tags, status, total_events, final_state_digest, duration_ms, success, rubric_path, is_promotion_gate, agent_phase, created_at, completed_at FROM runs WHERE run_id = $rid",
            )
            .bind(("rid", rid_owned))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        let rows: Vec<DbRun> = res
            .take(0)
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        rows.into_iter()
            .next()
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: rid.to_string(),
            })
    }

    /// Fetch a run row and verify it is in "RUNNING" state.
    async fn fetch_running(&self, rid: &str) -> StorageResult<DbRun> {
        let row = self.fetch_run(rid).await?;
        if row.status != "RUNNING" {
            return Err(StorageError::InvalidRunState {
                run_id: rid.to_string(),
                status: row.status,
                expected: "RUNNING".to_string(),
            });
        }
        Ok(row)
    }

    /// Convert a `schema::RunRecord` (DB row) into a `storage_traits::RunRecord`.
    fn db_run_to_record(row: DbRun) -> StorageResult<RunRecord> {
        let status = match row.status.as_str() {
            "RUNNING" => RunStatus::Running,
            "COMPLETED" => RunStatus::Completed,
            "FAILED" => RunStatus::Failed,
            "CANCELLED" => RunStatus::Cancelled,
            other => {
                return Err(StorageError::Backend(format!(
                    "unknown run status: {other}"
                )))
            }
        };

        let summary = if status != RunStatus::Running {
            let final_state_digest = row
                .final_state_digest
                .map(ContentDigest::try_from)
                .transpose()?;
            Some(RunSummary {
                total_events: row.total_events,
                final_state_digest,
                duration_ms: row.duration_ms,
                success: row.success,
            })
        } else {
            None
        };

        Ok(RunRecord {
            run_id: RunId(row.run_id),
            spec_digest: ContentDigest::try_from(row.spec_digest)?,
            metadata: RunMetadata {
                git_sha: row.git_sha,
                agent_name: row.agent_name,
                tags: row.tags,
                evaluation: EvaluationMetadata {
                    rubric_path: row.rubric_path,
                    is_promotion_gate: row.is_promotion_gate,
                    agent_phase: row.agent_phase,
                },
            },
            status,
            summary,
            created_at: row.created_at,
            completed_at: row.completed_at,
        })
    }

    /// Convert a `schema::RunEventRecord` (DB row) into a `storage_traits::RunEvent`.
    fn db_event_to_event(row: DbEvent) -> RunEvent {
        RunEvent {
            seq: row.seq,
            kind: row.kind,
            payload: row.payload,
            timestamp: row.timestamp,
        }
    }
}

#[async_trait]
impl RunLedger for SurrealRunLedger {
    async fn create_run(
        &self,
        spec_digest: &ContentDigest,
        metadata: RunMetadata,
    ) -> StorageResult<RunId> {
        let run_id = RunId::new();
        let db_row = DbRun::new(
            run_id.0.clone(),
            spec_digest.as_str().to_string(),
            metadata.git_sha,
            metadata.agent_name,
            metadata.tags,
            metadata.evaluation.rubric_path,
            metadata.evaluation.is_promotion_gate,
            metadata.evaluation.agent_phase,
        );

        debug!(run_id = %run_id, "creating run");

        self.db
            .query("CREATE runs CONTENT $row")
            .bind(("row", db_row))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(run_id)
    }

    async fn append_event(&self, run_id: &RunId, event: RunEvent) -> StorageResult<()> {
        self.fetch_running(&run_id.0).await?;

        let db_event = DbEvent::new(run_id.0.clone(), event.seq, event.kind, event.payload);

        self.db
            .query("CREATE run_events CONTENT $row")
            .bind(("row", db_event))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn complete_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let row = self.fetch_running(&run_id.0).await?;

        let final_digest_str = summary
            .final_state_digest
            .as_ref()
            .map(|d| d.as_str().to_string());

        let updated = row.complete(summary.total_events, final_digest_str, summary.duration_ms);
        let rid_owned = run_id.0.clone();

        self.db
            .query("UPDATE runs MERGE $row WHERE run_id = $rid")
            .bind(("row", updated))
            .bind(("rid", rid_owned))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn fail_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let row = self.fetch_running(&run_id.0).await?;

        let updated = row.fail(summary.total_events, summary.duration_ms);
        let rid_owned = run_id.0.clone();

        self.db
            .query("UPDATE runs MERGE $row WHERE run_id = $rid")
            .bind(("row", updated))
            .bind(("rid", rid_owned))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn cancel_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let row = self.fetch_running(&run_id.0).await?;

        let updated = row.cancel(summary.total_events, summary.duration_ms);
        let rid_owned = run_id.0.clone();

        self.db
            .query("UPDATE runs MERGE $row WHERE run_id = $rid")
            .bind(("row", updated))
            .bind(("rid", rid_owned))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn get_run(&self, run_id: &RunId) -> StorageResult<RunRecord> {
        let row = self.fetch_run(&run_id.0).await?;
        Self::db_run_to_record(row)
    }

    async fn get_events(&self, run_id: &RunId) -> StorageResult<Vec<RunEvent>> {
        // Verify run exists
        self.fetch_run(&run_id.0).await?;

        let rid_owned = run_id.0.clone();
        let mut res = self
            .db
            .query(
                "SELECT run_id, seq, kind, payload, timestamp FROM run_events WHERE run_id = $rid ORDER BY seq ASC",
            )
            .bind(("rid", rid_owned))
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        let rows: Vec<DbEvent> = res
            .take(0)
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(rows.into_iter().map(Self::db_event_to_event).collect())
    }

    async fn list_runs(
        &self,
        spec_digest: Option<&ContentDigest>,
    ) -> StorageResult<Vec<RunRecord>> {
        let rows: Vec<DbRun> = if let Some(digest) = spec_digest {
            let sd = digest.as_str().to_string();
            let mut res = self
                .db
                .query(
                    "SELECT run_id, spec_digest, git_sha, agent_name, tags, status, total_events, final_state_digest, duration_ms, success, rubric_path, is_promotion_gate, agent_phase, created_at, completed_at FROM runs WHERE spec_digest = $sd ORDER BY created_at DESC",
                )
                .bind(("sd", sd))
                .await
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            res.take(0)
                .map_err(|e| StorageError::Backend(e.to_string()))?
        } else {
            let mut res = self
                .db
                .query(
                    "SELECT run_id, spec_digest, git_sha, agent_name, tags, status, total_events, final_state_digest, duration_ms, success, rubric_path, is_promotion_gate, agent_phase, created_at, completed_at FROM runs ORDER BY created_at DESC",
                )
                .await
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            res.take(0)
                .map_err(|e| StorageError::Backend(e.to_string()))?
        };

        rows.into_iter().map(Self::db_run_to_record).collect()
    }
}
