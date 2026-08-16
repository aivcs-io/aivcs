//! Oxidized-State: data-mesh-backed state layer for AIVCS
//!
//! This crate provides the persistence layer for the Agent Version Control System.
//! The governed handle uses the data-mesh offering with an in-process fallback for
//! local execution and tests. Legacy SurrealDB-backed helpers remain in the crate
//! for the older run-ledger and migration paths.
//!
//! ## Layer 0 - Data/Persistence
//!
//! Focus: Data integrity, transactionality, and graph traversal.
//!
//! ## Key Components
//!
//! - `SurrealHandle`: Manages the data-mesh-backed state store
//! - `SnapshotRecord`: Schema mapping to the Document Layer (State + Memory)
//! - `GraphEdge`: Schema mapping to the Graph Layer (Commit -> Parent)
//! - `RunRecord`, `RunEventRecord`: Schema for execution run ledger
//! - `ReleaseRecordSchema`: Schema for release management
//! - `init_schema`: Initialize all tables with constraints and indexes

mod ci;
mod error;
pub mod fakes;
mod handle;
pub mod migrations;
mod schema;
pub mod storage_traits;
pub mod surreal_ledger;
pub mod surreal_release_registry;

pub use ci::{
    CiArtifact, CiCommand, CiPipelineSpec, CiRunRecord, CiRunStatus, CiSnapshot, CiStepResult,
    CiStepSpec,
};
pub use error::{StateError, StorageError};
pub use handle::{CloudConfig, SurrealHandle};
pub use migrations::init_schema;
pub use schema::{
    AgentRecord, CommitId, CommitRecord, DecisionRecord, EdgeType, GraphEdge,
    MemoryProvenanceRecord, MemoryRecord, ProvenanceSourceType, ReleaseRecordSchema,
    RunEventRecord as DbRunEventRecord, RunRecord as DbRunRecord, SnapshotRecord,
};
pub use storage_traits::{
    CasStore, ContentDigest, ReleaseMetadata, ReleaseRecord, ReleaseRegistry, RunEvent, RunId,
    RunLedger, RunMetadata, RunRecord, RunStatus, RunSummary, StorageResult,
};
pub use surreal_ledger::SurrealRunLedger;
pub use surreal_release_registry::SurrealDbReleaseRegistry;

/// Result type for oxidized-state operations
pub type Result<T> = std::result::Result<T, StateError>;
