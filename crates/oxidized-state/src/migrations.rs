//! SurrealDB schema migrations and initialization
//!
//! This module provides initialization functions to set up all tables
//! with proper constraints, indexes, and ACID guarantees.

use crate::error::StateError;
use crate::Result;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use tracing::{debug, info};

impl From<surrealdb::Error> for StateError {
    fn from(err: surrealdb::Error) -> Self {
        StateError::Query(err.to_string())
    }
}

/// Initialize all AIVCS tables in SurrealDB
///
/// This should be called once on first connection to set up the schema.
/// Safe to call multiple times (idempotent).
pub async fn init_schema(db: &Surreal<Any>) -> Result<()> {
    info!("Initializing AIVCS SurrealDB schema");

    // Core VCS tables
    init_commits_table(db).await?;
    init_snapshots_table(db).await?;
    init_graph_edges_table(db).await?;
    init_memories_table(db).await?;
    init_agents_table(db).await?;

    // Run Ledger tables
    init_runs_table(db).await?;
    init_run_events_table(db).await?;

    // Release Registry tables
    init_releases_table(db).await?;

    // CI tables
    init_ci_tables(db).await?;

    // Memory and Decision tables (EPIC5)
    init_decisions_table(db).await?;
    init_memory_provenances_table(db).await?;

    info!("AIVCS schema initialization complete");
    Ok(())
}

/// Initialize `runs` table with constraints and indexes
///
/// Schema:
/// ```text
/// TABLE runs {
///   run_id:              STRING (primary key, unique)
///   spec_digest:         STRING (indexed)
///   git_sha:             STRING? (optional, indexed)
///   agent_name:          STRING (indexed)
///   tags:                OBJECT
///   status:              STRING (enum: RUNNING | COMPLETED | FAILED | CANCELLED)
///   total_events:        INT
///   final_state_digest:  STRING?
///   duration_ms:         INT
///   success:             BOOL
///   created_at:          DATETIME (indexed)
///   completed_at:        DATETIME?
/// }
/// ```
///
/// Constraints:
/// - `run_id` is unique (prevents duplicate runs)
/// - `status` must be one of: "RUNNING", "COMPLETED", "FAILED", "CANCELLED"
/// - `status` transitions: RUNNING → COMPLETED | FAILED | CANCELLED (enforced via app logic)
/// - Completed runs are immutable (enforced via app logic)
async fn init_runs_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing runs table");

    // Create table with constraints
    let sql = r#"
        DEFINE TABLE runs SCHEMALESS
            PERMISSIONS
                FOR create FULL
                FOR select FULL
                FOR update FULL
                FOR delete NONE;

        -- Ensure run_id is unique
        DEFINE INDEX idx_run_id ON TABLE runs COLUMNS run_id UNIQUE;

        -- Index spec_digest for listing runs by agent version
        DEFINE INDEX idx_spec_digest ON TABLE runs COLUMNS spec_digest;

        -- Index agent_name for finding runs by agent
        DEFINE INDEX idx_agent_name ON TABLE runs COLUMNS agent_name;

        -- Index git_sha for correlating runs with git commits
        DEFINE INDEX idx_git_sha ON TABLE runs COLUMNS git_sha;

        -- Index created_at for time-range queries
        DEFINE INDEX idx_created_at ON TABLE runs COLUMNS created_at;

        -- Composite index (spec_digest, created_at) for fast agent version history
        DEFINE INDEX idx_spec_digest_created_at ON TABLE runs COLUMNS spec_digest, created_at;

        -- Composite index (run_id, status) for state queries
        DEFINE INDEX idx_run_id_status ON TABLE runs COLUMNS run_id, status;
    "#;

    db.query(sql).await?;
    info!("✓ runs table initialized");
    Ok(())
}

/// Initialize `run_events` table with constraints and indexes
///
/// Schema:
/// ```text
/// TABLE run_events {
///   run_id:     STRING (foreign key to runs.run_id)
///   seq:        INT (monotonic sequence within run)
///   kind:       STRING (event type)
///   payload:    OBJECT (event data)
///   timestamp:  DATETIME
/// }
/// ```
///
/// Constraints:
/// - `(run_id, seq)` is unique and clustered (prevents duplicate seq)
/// - `seq` is 1-indexed and monotonically increasing within a run
/// - Enforced via application logic during append_event()
async fn init_run_events_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing run_events table");

    let sql = r#"
        DEFINE TABLE run_events SCHEMALESS
            PERMISSIONS
                FOR create FULL
                FOR select FULL
                FOR update NONE
                FOR delete NONE;

        -- Composite unique index: (run_id, seq) ensures no duplicate sequences per run
        -- This is the most critical constraint for event ordering
        DEFINE INDEX idx_run_id_seq ON TABLE run_events COLUMNS run_id, seq UNIQUE;

        -- Index run_id for fast event retrieval by run
        DEFINE INDEX idx_run_id ON TABLE run_events COLUMNS run_id;

        -- Index (run_id, timestamp) for time-ordered queries
        DEFINE INDEX idx_run_id_timestamp ON TABLE run_events COLUMNS run_id, timestamp;

        -- Index event kind for filtering by event type
        DEFINE INDEX idx_kind ON TABLE run_events COLUMNS kind;

        -- Composite index (run_id, seq, timestamp) for sorted event retrieval
        DEFINE INDEX idx_run_id_seq_timestamp ON TABLE run_events COLUMNS run_id, seq, timestamp;
    "#;

    db.query(sql).await?;
    info!("✓ run_events table initialized");
    Ok(())
}

/// Initialize `releases` table with constraints and indexes
///
/// Schema:
/// ```text
/// TABLE releases {
///   agent_name:     STRING (part of uniqueness constraint)
///   spec_digest:    STRING
///   version_label:  STRING? (optional semantic version)
///   promoted_by:    STRING (who promoted this release)
///   notes:          STRING? (release notes)
///   created_at:     DATETIME (unique per agent+time)
/// }
/// ```
///
/// Semantics:
/// - Release history is append-only (new release entry for rollback)
/// - Most recent release (by created_at) is "current"
/// - Uniqueness enforced at application layer (can have same spec_digest multiple times)
async fn init_releases_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing releases table");

    let sql = r#"
        DEFINE TABLE releases SCHEMAFULL;
        DEFINE FIELD name ON releases TYPE string;
        DEFINE FIELD spec_digest ON releases TYPE string;
        DEFINE FIELD metadata ON releases FLEXIBLE TYPE object;
        DEFINE FIELD version_label ON releases TYPE option<string>;
        DEFINE FIELD promoted_by ON releases TYPE option<string>;
        DEFINE FIELD notes ON releases TYPE option<string>;
        DEFINE FIELD created_at ON releases TYPE datetime;

        DEFINE INDEX idx_release_name ON releases FIELDS name;
        DEFINE INDEX idx_release_name_created_at ON releases FIELDS name, created_at;
        DEFINE INDEX idx_spec_digest ON releases FIELDS spec_digest;
    "#;

    db.query(sql).await?;
    info!("✓ releases table initialized");
    Ok(())
}

/// Initialize `commits` table
async fn init_commits_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing commits table");

    let sql = r#"
        DEFINE TABLE commits SCHEMAFULL;
        DEFINE FIELD commit_id ON commits TYPE object;
        DEFINE FIELD commit_id.hash ON commits TYPE string;
        DEFINE FIELD commit_id.logic_hash ON commits TYPE option<string>;
        DEFINE FIELD commit_id.state_hash ON commits TYPE string;
        DEFINE FIELD commit_id.env_hash ON commits TYPE option<string>;
        DEFINE FIELD parent_ids ON commits TYPE array<string>;
        DEFINE FIELD message ON commits TYPE string;
        DEFINE FIELD author ON commits TYPE string;
        DEFINE FIELD created_at ON commits TYPE datetime;
        DEFINE INDEX idx_commit_hash ON commits FIELDS commit_id.hash UNIQUE;
        DEFINE INDEX idx_author ON commits FIELDS author;
    "#;

    db.query(sql).await?;
    info!("✓ commits table initialized");
    Ok(())
}

/// Initialize `snapshots` table
async fn init_snapshots_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing snapshots table");

    let sql = r#"
        DEFINE TABLE snapshots SCHEMAFULL;
        DEFINE FIELD commit_id ON snapshots TYPE string;
        DEFINE FIELD state ON snapshots FLEXIBLE TYPE object;
        DEFINE FIELD size_bytes ON snapshots TYPE int;
        DEFINE FIELD created_at ON snapshots TYPE datetime;
        DEFINE INDEX idx_snapshot_commit ON snapshots FIELDS commit_id UNIQUE;
    "#;

    db.query(sql).await?;
    info!("✓ snapshots table initialized");
    Ok(())
}

/// Initialize `graph_edges` table
async fn init_graph_edges_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing graph_edges table");

    let sql = r#"
        DEFINE TABLE graph_edges SCHEMAFULL;
        DEFINE FIELD child_id ON graph_edges TYPE string;
        DEFINE FIELD parent_id ON graph_edges TYPE string;
        DEFINE FIELD edge_type ON graph_edges TYPE string;
        DEFINE FIELD created_at ON graph_edges TYPE datetime;
        DEFINE INDEX idx_edge_child ON graph_edges FIELDS child_id;
        DEFINE INDEX idx_edge_parent ON graph_edges FIELDS parent_id;
    "#;

    db.query(sql).await?;
    info!("✓ graph_edges table initialized");
    Ok(())
}

/// Initialize `memories` table
async fn init_memories_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing memories table");

    let sql = r#"
        DEFINE TABLE memories SCHEMAFULL;
        DEFINE FIELD commit_id ON memories TYPE string;
        DEFINE FIELD key ON memories TYPE string;
        DEFINE FIELD content ON memories TYPE string;
        DEFINE FIELD embedding ON memories TYPE option<array>;
        DEFINE FIELD metadata ON memories FLEXIBLE TYPE object;
        DEFINE FIELD created_at ON memories TYPE datetime;
        DEFINE INDEX idx_memory_commit ON memories FIELDS commit_id;
    "#;

    db.query(sql).await?;
    info!("✓ memories table initialized");
    Ok(())
}

/// Initialize `agents` table
async fn init_agents_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing agents table");

    let sql = r#"
        DEFINE TABLE agents SCHEMAFULL;
        DEFINE FIELD agent_id ON agents TYPE string;
        DEFINE FIELD name ON agents TYPE string;
        DEFINE FIELD agent_type ON agents TYPE string;
        DEFINE FIELD config ON agents FLEXIBLE TYPE object;
        DEFINE FIELD created_at ON agents TYPE datetime;
        DEFINE INDEX idx_agent_id ON agents FIELDS agent_id UNIQUE;
    "#;

    db.query(sql).await?;
    info!("✓ agents table initialized");
    Ok(())
}

/// Initialize CI related tables
async fn init_ci_tables(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing CI tables");

    let sql = r#"
        -- CI snapshot table (content-addressed by digest)
        DEFINE TABLE ci_snapshots SCHEMAFULL;
        DEFINE FIELD digest ON ci_snapshots TYPE string;
        DEFINE FIELD snapshot_json ON ci_snapshots TYPE string;
        DEFINE INDEX idx_ci_snapshot_digest ON ci_snapshots FIELDS digest UNIQUE;

        -- CI pipeline table (content-addressed by digest)
        DEFINE TABLE ci_pipelines SCHEMAFULL;
        DEFINE FIELD digest ON ci_pipelines TYPE string;
        DEFINE FIELD pipeline_json ON ci_pipelines TYPE string;
        DEFINE INDEX idx_ci_pipeline_digest ON ci_pipelines FIELDS digest UNIQUE;

        -- CI run table (linked by run_id and digests)
        DEFINE TABLE ci_runs SCHEMAFULL;
        DEFINE FIELD run_id ON ci_runs TYPE string;
        DEFINE FIELD snapshot_digest ON ci_runs TYPE string;
        DEFINE FIELD pipeline_digest ON ci_runs TYPE string;
        DEFINE FIELD status ON ci_runs TYPE string;
        DEFINE FIELD run_json ON ci_runs TYPE string;
        DEFINE FIELD started_at ON ci_runs TYPE option<string>;
        DEFINE FIELD finished_at ON ci_runs TYPE option<string>;
        DEFINE INDEX idx_ci_run_id ON ci_runs FIELDS run_id UNIQUE;
        DEFINE INDEX idx_ci_run_snapshot ON ci_runs FIELDS snapshot_digest;
    "#;

    db.query(sql).await?;
    info!("✓ CI tables initialized");
    Ok(())
}

/// Initialize `decisions` table (EPIC5)
///
/// Schema:
/// ```text
/// TABLE decisions {
///   decision_id:    STRING (primary key)
///   commit_id:      STRING (indexed)
///   task:           STRING
///   action:         STRING
///   rationale:      STRING
///   alternatives:   ARRAY<STRING>
///   confidence:     FLOAT (0.0-1.0)
///   outcome:        STRING? (JSON serialized outcome)
///   timestamp:      DATETIME (indexed)
///   outcome_at:     DATETIME?
/// }
/// ```
async fn init_decisions_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing decisions table");

    let sql = r#"
        DEFINE TABLE decisions SCHEMAFULL;
        DEFINE FIELD decision_id ON decisions TYPE string;
        DEFINE FIELD commit_id ON decisions TYPE string;
        DEFINE FIELD task ON decisions TYPE string;
        DEFINE FIELD action ON decisions TYPE string;
        DEFINE FIELD rationale ON decisions TYPE string;
        DEFINE FIELD alternatives ON decisions TYPE array<string>;
        DEFINE FIELD confidence ON decisions TYPE float;
        DEFINE FIELD outcome ON decisions TYPE option<string>;
        DEFINE FIELD timestamp ON decisions TYPE datetime;
        DEFINE FIELD outcome_at ON decisions TYPE option<datetime>;

        DEFINE INDEX idx_decision_id ON decisions FIELDS decision_id UNIQUE;
        DEFINE INDEX idx_decision_commit ON decisions FIELDS commit_id;
        DEFINE INDEX idx_decision_task ON decisions FIELDS task;
        DEFINE INDEX idx_decision_timestamp ON decisions FIELDS timestamp;
        DEFINE INDEX idx_decision_commit_task ON decisions FIELDS commit_id, task;
    "#;

    db.query(sql).await?;
    info!("✓ decisions table initialized");
    Ok(())
}

/// Initialize `memory_provenances` table (EPIC5)
///
/// Schema:
/// ```text
/// TABLE memory_provenances {
///   memory_id:       STRING (indexed)
///   source_type:     STRING (run_trace | state_snapshot | user_annotation | memory_derivation)
///   source_data:     OBJECT (variant-specific fields)
///   derived_from:    STRING? (parent memory_id)
///   created_at:      DATETIME (indexed)
///   invalidated_at:  DATETIME?
/// }
/// ```
async fn init_memory_provenances_table(db: &Surreal<Any>) -> Result<()> {
    debug!("Initializing memory_provenances table");

    let sql = r#"
        DEFINE TABLE memory_provenances SCHEMAFULL;
        DEFINE FIELD memory_id ON memory_provenances TYPE string;
        DEFINE FIELD source_type ON memory_provenances TYPE string;
        DEFINE FIELD source_data ON memory_provenances FLEXIBLE TYPE object;
        DEFINE FIELD derived_from ON memory_provenances TYPE option<string>;
        DEFINE FIELD created_at ON memory_provenances TYPE datetime;
        DEFINE FIELD invalidated_at ON memory_provenances TYPE option<datetime>;

        DEFINE INDEX idx_provenance_memory_id ON memory_provenances FIELDS memory_id;
        DEFINE INDEX idx_provenance_created_at ON memory_provenances FIELDS created_at;
        DEFINE INDEX idx_provenance_derived_from ON memory_provenances FIELDS derived_from;
        DEFINE INDEX idx_provenance_source_type ON memory_provenances FIELDS source_type;
        DEFINE INDEX idx_provenance_invalidated ON memory_provenances FIELDS invalidated_at;
    "#;

    db.query(sql).await?;
    info!("✓ memory_provenances table initialized");
    Ok(())
}

#[cfg(test)]
mod tests {
    // Note: Full integration tests for migrations are in oxidized-state/tests/
    // These tests verify actual schema creation and constraints
}
