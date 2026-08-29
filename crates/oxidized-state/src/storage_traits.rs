//! Storage trait definitions for AIVCS
//!
//! These traits define the core storage abstractions:
//! - `CasStore`: Content-addressed storage (put/get by digest)
//! - `RunLedger`: Execution run persistence (events, summaries)
//! - `ReleaseRegistry`: Agent release management (promote/rollback)
//!
//! All traits are async and backend-agnostic. In-memory fakes are provided
//! for testing via the `fakes` module.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::StorageError;

/// Result type for storage operations
pub type StorageResult<T> = std::result::Result<T, StorageError>;

// ---------------------------------------------------------------------------
// CasStore — Content-Addressed Storage
// ---------------------------------------------------------------------------

/// Content digest (SHA-256 hex string).
///
/// The inner field is private to guarantee the string is always valid
/// lowercase hex produced by `from_bytes` or validated via `TryFrom<String>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentDigest(String);

impl ContentDigest {
    /// Compute the SHA-256 digest of the given bytes.
    pub fn from_bytes(data: &[u8]) -> Self {
        use sha2::Digest;
        let mut hasher = Sha256::new();
        hasher.update(data);
        ContentDigest(hex::encode(hasher.finalize()))
    }

    /// Return the full hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Short form (first 12 hex chars).
    pub fn short(&self) -> String {
        self.0.chars().take(12).collect()
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = StorageError;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidDigest { digest: s });
        }
        Ok(ContentDigest(s.to_ascii_lowercase()))
    }
}

impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Content-addressed blob store.
///
/// Guarantees:
/// - `put(data)` always returns the SHA-256 digest of `data`.
/// - `get(digest)` returns the exact bytes previously stored.
/// - Same content always yields the same digest (deduplication).
#[async_trait]
pub trait CasStore: Send + Sync {
    /// Store bytes and return their content digest.
    async fn put(&self, data: &[u8]) -> StorageResult<ContentDigest>;

    /// Retrieve bytes by digest. Returns `StorageError::NotFound` if absent.
    async fn get(&self, digest: &ContentDigest) -> StorageResult<Vec<u8>>;

    /// Check whether a digest exists in the store.
    async fn contains(&self, digest: &ContentDigest) -> StorageResult<bool>;

    /// Delete content by digest. No-op if absent.
    async fn delete(&self, digest: &ContentDigest) -> StorageResult<()>;
}

// ---------------------------------------------------------------------------
// RunLedger — Execution Run Persistence
// ---------------------------------------------------------------------------

/// Unique identifier for an execution run
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl RunId {
    /// Generate a new random RunId
    pub fn new() -> Self {
        RunId(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Metadata for a specific execution run
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunMetadata {
    /// Git SHA at time of run
    pub git_sha: Option<String>,
    /// Agent name
    pub agent_name: String,
    /// Arbitrary key-value tags
    pub tags: serde_json::Value,
    /// Evaluation context for this run
    #[serde(default)]
    pub evaluation: EvaluationMetadata,
}

/// Metadata for tasks that are part of an agent evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationMetadata {
    /// Path to the success rubric for this evaluation.
    pub rubric_path: Option<String>,
    /// Whether this run is a production-promotion gate.
    pub is_promotion_gate: bool,
    /// Current maturity phase of the agent (1-4).
    pub agent_phase: u8,
}

/// A single event in an execution run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    /// Monotonic sequence number within the run
    pub seq: u64,
    /// Event kind (e.g. "graph_started", "node_entered", "tool_called")
    pub kind: String,
    /// Event payload
    pub payload: serde_json::Value,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// Summary produced when a run completes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total events recorded
    pub total_events: u64,
    /// Final state digest (if applicable)
    pub final_state_digest: Option<ContentDigest>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Whether the run succeeded
    pub success: bool,
}

/// Status of a run
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Full run record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub spec_digest: ContentDigest,
    pub metadata: RunMetadata,
    pub status: RunStatus,
    pub summary: Option<RunSummary>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Execution run ledger.
///
/// Guarantees:
/// - Events are ordered by monotonic `seq` within a run.
/// - A run transitions: Running → Completed | Failed (terminal).
/// - Completed runs are immutable.
#[async_trait]
pub trait RunLedger: Send + Sync {
    /// Create a new run, returning its unique ID.
    async fn create_run(
        &self,
        spec_digest: &ContentDigest,
        metadata: RunMetadata,
    ) -> StorageResult<RunId>;

    /// Append an event to an active run. Fails if the run is completed/failed.
    async fn append_event(&self, run_id: &RunId, event: RunEvent) -> StorageResult<()>;

    /// Mark a run as completed with a summary.
    async fn complete_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()>;

    /// Mark a run as failed with a summary.
    async fn fail_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()>;

    /// Mark a run as cancelled.
    async fn cancel_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()>;

    /// Retrieve a run record by ID.
    async fn get_run(&self, run_id: &RunId) -> StorageResult<RunRecord>;

    /// Retrieve all events for a run, ordered by seq.
    async fn get_events(&self, run_id: &RunId) -> StorageResult<Vec<RunEvent>>;

    /// List runs, optionally filtered by spec digest.
    async fn list_runs(&self, spec_digest: Option<&ContentDigest>)
        -> StorageResult<Vec<RunRecord>>;
}

// ---------------------------------------------------------------------------
// ReleaseRegistry — Agent Release Management
// ---------------------------------------------------------------------------

/// Metadata for a release
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseMetadata {
    /// Human-readable version label (e.g. "v1.2.3")
    pub version_label: Option<String>,
    /// Who or what promoted this release
    pub promoted_by: String,
    /// Release notes
    pub notes: Option<String>,
}

/// A single release record (pointer from name → spec digest)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseRecord {
    /// Agent name this release belongs to
    pub name: String,
    /// The spec digest being released
    pub spec_digest: ContentDigest,
    /// Release metadata
    pub metadata: ReleaseMetadata,
    /// When this release was created
    pub created_at: DateTime<Utc>,
}

/// Agent release registry.
///
/// Semantics:
/// - `promote` creates a new release entry as the new current release.
/// - `rollback` reverts to the previous release by re-appending it as a new
///   entry, preserving the full audit trail (history is append-only).
/// - `history` returns the complete release chain in reverse chronological
///   order (newest first).
#[async_trait]
pub trait ReleaseRegistry: Send + Sync {
    /// Promote a new release for the given agent name.
    async fn promote(
        &self,
        name: &str,
        spec_digest: &ContentDigest,
        metadata: ReleaseMetadata,
    ) -> StorageResult<ReleaseRecord>;

    /// Roll back to the previous release. Fails if no previous release exists.
    async fn rollback(&self, name: &str) -> StorageResult<ReleaseRecord>;

    /// Get the current (most recent) release for a name, if any.
    async fn current(&self, name: &str) -> StorageResult<Option<ReleaseRecord>>;

    /// Get full release history for a name (newest first).
    async fn history(&self, name: &str) -> StorageResult<Vec<ReleaseRecord>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_serialization() {
        let status = RunStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"RUNNING\"");

        let status = RunStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"COMPLETED\"");
    }
}
