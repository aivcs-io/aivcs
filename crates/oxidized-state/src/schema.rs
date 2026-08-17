//! Schema definitions for AIVCS state records
//!
//! Tables:
//! - commits: Version control commits (graph nodes)
//! - agents: Registered agent metadata
//! - memories: Agent memory/context snapshots

use chrono::{DateTime, Utc};

/// Module for serializing record IDs as strings while tolerating SurrealDB
/// `Thing` values on legacy reads.
mod record_id {
    use serde::{self, Deserialize, Deserializer, Serialize, Serializer};

    fn extract_string(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Array(values) => values.iter().find_map(extract_string),
            serde_json::Value::Object(map) => {
                if let Some(id) = map.get("id") {
                    return extract_string(id);
                }
                map.values().find_map(extract_string)
            }
            _ => None,
        }
    }

    pub fn serialize<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(value, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<serde_json::Value>::deserialize(deserializer)?;
        Ok(value.and_then(|value| extract_string(&value)))
    }
}

/// Module for serializing chrono DateTime as RFC3339 strings.
mod datetime {
    use chrono::{DateTime, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&date.to_rfc3339())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// Module for serializing optional chrono DateTime as RFC3339 strings.
mod datetime_opt {
    use chrono::{DateTime, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(date: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => serializer.serialize_some(&d.to_rfc3339()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let sd = Option::<String>::deserialize(deserializer)?;
        sd.map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(serde::de::Error::custom)
        })
        .transpose()
    }
}
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Composite Commit ID - hash of (Logic + State + Environment)
///
/// A commit in AIVCS is a tuple of:
/// 1. Logic: The Rust binaries/scripts hash
/// 2. State: The agent state snapshot hash
/// 3. Environment: The Nix Flake hash
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitId {
    /// The composite hash
    pub hash: String,
    /// Logic hash component
    pub logic_hash: Option<String>,
    /// State hash component
    pub state_hash: String,
    /// Environment (Nix) hash component
    pub env_hash: Option<String>,
}

impl CommitId {
    /// Create a new CommitId from state only (Phase 1 MVP)
    pub fn from_state(state: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(state);
        let state_hash = hex::encode(hasher.finalize());

        // Consistent with new(None, state_hash, None)
        Self::new(None, &state_hash, None)
    }

    /// Create a full composite CommitId (Phase 2+)
    pub fn new(logic_hash: Option<&str>, state_hash: &str, env_hash: Option<&str>) -> Self {
        let mut hasher = Sha256::new();

        // Use markers and separators to prevent hash collisions
        // Logic component
        hasher.update(b"L");
        if let Some(lh) = logic_hash {
            hasher.update(b"S");
            hasher.update(lh.as_bytes());
        } else {
            hasher.update(b"N");
        }
        hasher.update(b"\0");

        // State component (always present)
        hasher.update(b"S:");
        hasher.update(state_hash.as_bytes());
        hasher.update(b"\0");

        // Environment component
        hasher.update(b"E");
        if let Some(eh) = env_hash {
            hasher.update(b"S");
            hasher.update(eh.as_bytes());
        } else {
            hasher.update(b"N");
        }

        let composite = hex::encode(hasher.finalize());

        CommitId {
            hash: composite,
            logic_hash: logic_hash.map(String::from),
            state_hash: state_hash.to_string(),
            env_hash: env_hash.map(String::from),
        }
    }

    /// Get short hash (first 8 characters)
    pub fn short(&self) -> String {
        self.hash.chars().take(8).collect()
    }
}

impl std::fmt::Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash)
    }
}

/// Commit record stored in SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// The commit ID (composite hash)
    pub commit_id: CommitId,
    /// Parent commit IDs (empty for root commits)
    pub parent_ids: Vec<String>,
    /// Commit message
    pub message: String,
    /// Author/agent that created the commit
    pub author: String,
    /// Timestamp of commit creation
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

impl CommitRecord {
    /// Create a new commit record
    pub fn new(commit_id: CommitId, parent_ids: Vec<String>, message: &str, author: &str) -> Self {
        CommitRecord {
            id: None,
            commit_id,
            parent_ids,
            message: message.to_string(),
            author: author.to_string(),
            created_at: Utc::now(),
        }
    }
}

/// Snapshot record - the actual agent state data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// The commit ID this snapshot belongs to
    pub commit_id: String,
    /// Serialized agent state (JSON)
    pub state: serde_json::Value,
    /// Size in bytes
    pub size_bytes: u64,
    /// Timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

impl SnapshotRecord {
    /// Create a new snapshot record
    pub fn new(commit_id: &str, state: serde_json::Value) -> Self {
        let size = serde_json::to_string(&state)
            .map(|s| s.len() as u64)
            .unwrap_or(0);

        SnapshotRecord {
            id: None,
            commit_id: commit_id.to_string(),
            state,
            size_bytes: size,
            created_at: Utc::now(),
        }
    }
}

/// Agent record - registered agent metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// Agent UUID
    pub agent_id: Uuid,
    /// Agent name
    pub name: String,
    /// Agent type/kind
    pub agent_type: String,
    /// Configuration (JSON)
    pub config: serde_json::Value,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

impl AgentRecord {
    /// Create a new agent record
    pub fn new(name: &str, agent_type: &str, config: serde_json::Value) -> Self {
        AgentRecord {
            id: None,
            agent_id: Uuid::new_v4(),
            name: name.to_string(),
            agent_type: agent_type.to_string(),
            config,
            created_at: Utc::now(),
        }
    }
}

/// Memory record - agent memory/context for RAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// The commit ID this memory belongs to
    pub commit_id: String,
    /// Memory key/namespace
    pub key: String,
    /// Memory content (text for embedding)
    pub content: String,
    /// Optional embedding vector (for semantic search)
    pub embedding: Option<Vec<f32>>,
    /// Metadata
    pub metadata: serde_json::Value,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

impl MemoryRecord {
    /// Create a new memory record
    pub fn new(commit_id: &str, key: &str, content: &str) -> Self {
        MemoryRecord {
            id: None,
            commit_id: commit_id.to_string(),
            key: key.to_string(),
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
        }
    }

    /// Set embedding vector
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Graph edge - represents commit relationships (parent -> child)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Child commit ID
    pub child_id: String,
    /// Parent commit ID
    pub parent_id: String,
    /// Edge type (normal, merge, fork)
    pub edge_type: EdgeType,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

/// Type of graph edge
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EdgeType {
    /// Normal parent-child relationship
    Normal,
    /// Merge commit (multiple parents)
    Merge,
    /// Fork/branch point
    Fork,
}

impl GraphEdge {
    /// Create a new normal graph edge
    pub fn new(child_id: &str, parent_id: &str) -> Self {
        GraphEdge {
            child_id: child_id.to_string(),
            parent_id: parent_id.to_string(),
            edge_type: EdgeType::Normal,
            created_at: Utc::now(),
        }
    }

    /// Create a merge edge
    pub fn merge(child_id: &str, parent_id: &str) -> Self {
        GraphEdge {
            child_id: child_id.to_string(),
            parent_id: parent_id.to_string(),
            edge_type: EdgeType::Merge,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// RunLedger Records — Execution Run Persistence
// ---------------------------------------------------------------------------

/// Run record - execution run metadata and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// Unique run ID (UUID string)
    pub run_id: String,
    /// Agent spec digest (SHA256)
    pub spec_digest: String,
    /// Git SHA at time of run (optional)
    pub git_sha: Option<String>,
    /// Agent name
    pub agent_name: String,
    /// Arbitrary tags (JSON)
    pub tags: serde_json::Value,
    /// Run status: "RUNNING" | "COMPLETED" | "FAILED" | "CANCELLED"
    pub status: String,
    /// Total events recorded
    pub total_events: u64,
    /// Final state digest (if completed)
    pub final_state_digest: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Whether run succeeded
    pub success: bool,
    /// Path to the success rubric for this evaluation.
    pub rubric_path: Option<String>,
    /// Whether this run is a production-promotion gate.
    #[serde(default)]
    pub is_promotion_gate: bool,
    /// Current maturity phase of the agent (1-4).
    #[serde(default)]
    pub agent_phase: u8,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
    /// Completed timestamp (if terminal)
    #[serde(default, with = "datetime_opt")]
    pub completed_at: Option<DateTime<Utc>>,
}

impl RunRecord {
    /// Create a new run record in "running" state
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: String,
        spec_digest: String,
        git_sha: Option<String>,
        agent_name: String,
        tags: serde_json::Value,
        rubric_path: Option<String>,
        is_promotion_gate: bool,
        agent_phase: u8,
    ) -> Self {
        RunRecord {
            id: None,
            run_id,
            spec_digest,
            git_sha,
            agent_name,
            tags,
            status: "RUNNING".to_string(),
            total_events: 0,
            final_state_digest: None,
            duration_ms: 0,
            success: false,
            rubric_path,
            is_promotion_gate,
            agent_phase,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    /// Mark run as completed
    pub fn complete(
        mut self,
        total_events: u64,
        final_state_digest: Option<String>,
        duration_ms: u64,
    ) -> Self {
        self.status = "COMPLETED".to_string();
        self.total_events = total_events;
        self.final_state_digest = final_state_digest;
        self.duration_ms = duration_ms;
        self.success = true;
        self.completed_at = Some(Utc::now());
        self
    }

    /// Mark run as failed
    pub fn fail(mut self, total_events: u64, duration_ms: u64) -> Self {
        self.status = "FAILED".to_string();
        self.total_events = total_events;
        self.duration_ms = duration_ms;
        self.success = false;
        self.completed_at = Some(Utc::now());
        self
    }

    /// Mark run as cancelled
    pub fn cancel(mut self, total_events: u64, duration_ms: u64) -> Self {
        self.status = "CANCELLED".to_string();
        self.total_events = total_events;
        self.duration_ms = duration_ms;
        self.success = false;
        self.completed_at = Some(Utc::now());
        self
    }
}

/// Run event record - single event in execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// Run ID this event belongs to
    pub run_id: String,
    /// Monotonic sequence number within run (1-indexed)
    pub seq: u64,
    /// Event kind (e.g. "graph_started", "node_entered", "tool_called")
    pub kind: String,
    /// Event payload (JSON)
    pub payload: serde_json::Value,
    /// Event timestamp
    #[serde(with = "datetime")]
    pub timestamp: DateTime<Utc>,
}

impl RunEventRecord {
    /// Create a new run event record
    pub fn new(run_id: String, seq: u64, kind: String, payload: serde_json::Value) -> Self {
        RunEventRecord {
            id: None,
            run_id,
            seq,
            kind,
            payload,
            timestamp: Utc::now(),
        }
    }
}

/// Release record - agent release and version management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseRecordSchema {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// Release/Agent name
    pub name: String,
    /// Spec digest being released
    pub spec_digest: String,
    /// Version label (e.g. "v1.2.3")
    pub version_label: Option<String>,
    /// Who or what promoted this release
    pub promoted_by: String,
    /// Release notes
    pub notes: Option<String>,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
}

impl ReleaseRecordSchema {
    /// Create a new release record
    pub fn new(
        name: String,
        spec_digest: String,
        version_label: Option<String>,
        promoted_by: String,
        notes: Option<String>,
    ) -> Self {
        ReleaseRecordSchema {
            id: None,
            name,
            spec_digest,
            version_label,
            promoted_by,
            notes,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Decision and Memory Provenance Records — Decision Learning and Lineage
// ---------------------------------------------------------------------------

/// Decision record - captures agent decisions with rationale and outcomes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// Unique decision ID (UUID string)
    pub decision_id: String,
    /// Associated commit ID
    pub commit_id: String,
    /// Task/context this decision was about
    pub task: String,
    /// What was decided/action taken
    pub action: String,
    /// Why this decision was made
    pub rationale: String,
    /// Alternative options considered
    pub alternatives: Vec<String>,
    /// Confidence level (0.0-1.0)
    pub confidence: f32,
    /// Decision outcome
    pub outcome: Option<String>, // JSON serialized DecisionOutcome enum
    /// Decision timestamp
    #[serde(with = "datetime")]
    pub timestamp: DateTime<Utc>,
    /// When outcome was recorded (if any)
    #[serde(default, with = "datetime_opt")]
    pub outcome_at: Option<DateTime<Utc>>,
}

impl DecisionRecord {
    /// Create a new decision record
    pub fn new(
        decision_id: String,
        commit_id: String,
        task: String,
        action: String,
        rationale: String,
        confidence: f32,
    ) -> Self {
        DecisionRecord {
            id: None,
            decision_id,
            commit_id,
            task,
            action,
            rationale,
            alternatives: Vec::new(),
            confidence,
            outcome: None,
            timestamp: Utc::now(),
            outcome_at: None,
        }
    }

    /// Add alternatives to consider
    pub fn with_alternatives(mut self, alternatives: Vec<String>) -> Self {
        self.alternatives = alternatives;
        self
    }

    /// Record the decision outcome
    pub fn with_outcome(mut self, outcome: String) -> Self {
        self.outcome = Some(outcome);
        self.outcome_at = Some(Utc::now());
        self
    }
}

/// Provenance source for memory records
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSourceType {
    /// From a run execution trace
    RunTrace,
    /// From a state snapshot
    StateSnapshot,
    /// From user annotation
    UserAnnotation,
    /// Derived from another memory
    MemoryDerivation,
}

/// Memory provenance record - tracks lineage of memories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenanceRecord {
    /// Record ID
    #[serde(default, with = "record_id")]
    pub id: Option<String>,
    /// The memory ID this provenance describes
    pub memory_id: String,
    /// Source type (JSON serialized ProvenanceSourceType)
    pub source_type: String,
    /// Source details (JSON: run_id, event_idx, commit_id, user_id, parent_id, etc.)
    pub source_data: serde_json::Value,
    /// Parent memory ID if derived
    pub derived_from: Option<String>,
    /// Created timestamp
    #[serde(with = "datetime")]
    pub created_at: DateTime<Utc>,
    /// When this provenance became invalid/stale
    #[serde(default, with = "datetime_opt")]
    pub invalidated_at: Option<DateTime<Utc>>,
}

impl MemoryProvenanceRecord {
    /// Create provenance for a run trace source
    pub fn from_run_trace(memory_id: String, run_id: String, event_idx: usize) -> Self {
        MemoryProvenanceRecord {
            id: None,
            memory_id,
            source_type: ProvenanceSourceType::RunTrace.to_string(),
            source_data: serde_json::json!({ "run_id": run_id, "event_idx": event_idx }),
            derived_from: None,
            created_at: Utc::now(),
            invalidated_at: None,
        }
    }

    /// Create provenance for a state snapshot source
    pub fn from_snapshot(memory_id: String, commit_id: String) -> Self {
        MemoryProvenanceRecord {
            id: None,
            memory_id,
            source_type: ProvenanceSourceType::StateSnapshot.to_string(),
            source_data: serde_json::json!({ "commit_id": commit_id }),
            derived_from: None,
            created_at: Utc::now(),
            invalidated_at: None,
        }
    }

    /// Create provenance for user annotation
    pub fn from_user_annotation(memory_id: String, user_id: String) -> Self {
        MemoryProvenanceRecord {
            id: None,
            memory_id,
            source_type: ProvenanceSourceType::UserAnnotation.to_string(),
            source_data: serde_json::json!({ "user_id": user_id }),
            derived_from: None,
            created_at: Utc::now(),
            invalidated_at: None,
        }
    }

    /// Create provenance for derived memory
    pub fn from_derivation(memory_id: String, parent_id: String, derivation: String) -> Self {
        MemoryProvenanceRecord {
            id: None,
            memory_id,
            source_type: ProvenanceSourceType::MemoryDerivation.to_string(),
            source_data: serde_json::json!({ "derivation": derivation }),
            derived_from: Some(parent_id),
            created_at: Utc::now(),
            invalidated_at: None,
        }
    }

    /// Mark this provenance as invalidated
    pub fn invalidate(mut self) -> Self {
        self.invalidated_at = Some(Utc::now());
        self
    }
}

impl core::fmt::Display for ProvenanceSourceType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProvenanceSourceType::RunTrace => write!(f, "run_trace"),
            ProvenanceSourceType::StateSnapshot => write!(f, "state_snapshot"),
            ProvenanceSourceType::UserAnnotation => write!(f, "user_annotation"),
            ProvenanceSourceType::MemoryDerivation => write!(f, "memory_derivation"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_id_from_state() {
        let state = b"test state data";
        let commit_id = CommitId::from_state(state);

        assert!(!commit_id.hash.is_empty());
        assert_eq!(commit_id.hash.len(), 64); // SHA256 hex = 64 chars
        assert!(commit_id.logic_hash.is_none());
        assert!(commit_id.env_hash.is_none());
    }

    #[test]
    fn test_commit_id_deterministic() {
        let state = b"same state";
        let id1 = CommitId::from_state(state);
        let id2 = CommitId::from_state(state);

        assert_eq!(id1.hash, id2.hash);
    }

    #[test]
    fn test_commit_id_different_states() {
        let id1 = CommitId::from_state(b"state 1");
        let id2 = CommitId::from_state(b"state 2");

        assert_ne!(id1.hash, id2.hash);
    }

    #[test]
    fn test_commit_id_short() {
        let commit_id = CommitId::from_state(b"test");
        assert_eq!(commit_id.short().len(), 8);
    }

    #[test]
    fn test_composite_commit_id() {
        let commit_id = CommitId::new(Some("logic-hash"), "state-hash", Some("env-hash"));

        assert!(!commit_id.hash.is_empty());
        assert_eq!(commit_id.logic_hash, Some("logic-hash".to_string()));
        assert_eq!(commit_id.env_hash, Some("env-hash".to_string()));
    }

    #[test]
    fn test_commit_id_collision_prevention() {
        // Test that swapping components results in different hashes
        let id1 = CommitId::new(Some("ab"), "cd", None);
        let id2 = CommitId::new(Some("a"), "bcd", None);
        assert_ne!(id1.hash, id2.hash);

        // Test that None vs "none" string doesn't collide if we use prefixes correctly
        // (Wait, we use "none" for None, so if state_hash was "none" and logic_hash was None,
        // it might collide if we don't have prefixes)
        let id3 = CommitId::new(None, "state", None);
        let id4 = CommitId::new(Some("none"), "state", None);
        assert_ne!(id3.hash, id4.hash);
    }

    #[test]
    fn test_snapshot_record_size() {
        let state = serde_json::json!({"key": "value", "nested": {"a": 1}});
        let snapshot = SnapshotRecord::new("commit-123", state);

        assert!(snapshot.size_bytes > 0);
    }

    #[test]
    fn test_run_record_new() {
        let run = RunRecord::new(
            "run-123".to_string(),
            "spec-digest-abc".to_string(),
            Some("abc123".to_string()),
            "test-agent".to_string(),
            serde_json::json!({"env": "test"}),
            None,
            false,
            1,
        );

        assert_eq!(run.run_id, "run-123");
        assert_eq!(run.status, "RUNNING");
        assert_eq!(run.total_events, 0);
        assert!(!run.success);
    }

    #[test]
    fn test_run_record_complete() {
        let run = RunRecord::new(
            "run-123".to_string(),
            "spec-digest-abc".to_string(),
            Some("abc123".to_string()),
            "test-agent".to_string(),
            serde_json::json!({}),
            None,
            false,
            1,
        )
        .complete(5, Some("state-digest-xyz".to_string()), 1000);

        assert_eq!(run.status, "COMPLETED");
        assert_eq!(run.total_events, 5);
        assert!(run.success);
        assert!(run.completed_at.is_some());
    }

    #[test]
    fn test_run_record_fail() {
        let run = RunRecord::new(
            "run-123".to_string(),
            "spec-digest-abc".to_string(),
            None,
            "test-agent".to_string(),
            serde_json::json!({}),
            None,
            false,
            1,
        )
        .fail(2, 500);

        assert_eq!(run.status, "FAILED");
        assert_eq!(run.total_events, 2);
        assert!(!run.success);
        assert!(run.completed_at.is_some());
    }

    #[test]
    fn test_run_event_record() {
        let event = RunEventRecord::new(
            "run-123".to_string(),
            1,
            "graph_started".to_string(),
            serde_json::json!({"graph_id": "g1"}),
        );

        assert_eq!(event.run_id, "run-123");
        assert_eq!(event.seq, 1);
        assert_eq!(event.kind, "graph_started");
    }

    #[test]
    fn test_release_record() {
        let release = ReleaseRecordSchema::new(
            "my-agent".to_string(),
            "spec-digest-abc".to_string(),
            Some("v1.0.0".to_string()),
            "alice".to_string(),
            Some("Initial release".to_string()),
        );

        assert_eq!(release.name, "my-agent");
        assert_eq!(release.version_label, Some("v1.0.0".to_string()));
    }

    #[test]
    fn test_decision_record_new() {
        let decision = DecisionRecord::new(
            "dec-123".to_string(),
            "commit-abc".to_string(),
            "task-optimize".to_string(),
            "use_parallel".to_string(),
            "Improves throughput".to_string(),
            0.85,
        );

        assert_eq!(decision.decision_id, "dec-123");
        assert_eq!(decision.commit_id, "commit-abc");
        assert_eq!(decision.task, "task-optimize");
        assert_eq!(decision.action, "use_parallel");
        assert_eq!(decision.confidence, 0.85);
        assert!(decision.outcome.is_none());
    }

    #[test]
    fn test_decision_record_with_alternatives() {
        let decision = DecisionRecord::new(
            "dec-456".to_string(),
            "commit-def".to_string(),
            "task-retry".to_string(),
            "exponential_backoff".to_string(),
            "Reduces thundering herd".to_string(),
            0.75,
        )
        .with_alternatives(vec!["linear_backoff".to_string(), "no_retry".to_string()]);

        assert_eq!(decision.alternatives.len(), 2);
        assert!(decision
            .alternatives
            .contains(&"linear_backoff".to_string()));
    }

    #[test]
    fn test_decision_record_with_outcome() {
        let outcome_json = serde_json::json!({
            "status": "success",
            "benefit": 1.23,
            "duration_ms": 5000
        });
        let decision = DecisionRecord::new(
            "dec-789".to_string(),
            "commit-ghi".to_string(),
            "task-cache".to_string(),
            "redis_cache".to_string(),
            "Faster lookups".to_string(),
            0.9,
        )
        .with_outcome(outcome_json.to_string());

        assert!(decision.outcome.is_some());
        assert!(decision.outcome_at.is_some());
    }

    #[test]
    fn test_memory_provenance_from_run_trace() {
        let prov = MemoryProvenanceRecord::from_run_trace(
            "mem-123".to_string(),
            "run-456".to_string(),
            42,
        );

        assert_eq!(prov.memory_id, "mem-123");
        assert_eq!(prov.source_type, ProvenanceSourceType::RunTrace.to_string());
        assert!(prov.derived_from.is_none());
        assert!(prov.invalidated_at.is_none());

        let source_data: serde_json::Value = prov.source_data;
        assert_eq!(source_data["run_id"], "run-456");
        assert_eq!(source_data["event_idx"], 42);
    }

    #[test]
    fn test_memory_provenance_from_snapshot() {
        let prov =
            MemoryProvenanceRecord::from_snapshot("mem-789".to_string(), "commit-abc".to_string());

        assert_eq!(prov.memory_id, "mem-789");
        assert_eq!(
            prov.source_type,
            ProvenanceSourceType::StateSnapshot.to_string()
        );
        assert_eq!(prov.source_data["commit_id"], "commit-abc");
    }

    #[test]
    fn test_memory_provenance_from_derivation() {
        let prov = MemoryProvenanceRecord::from_derivation(
            "mem-new".to_string(),
            "mem-parent".to_string(),
            "summarize".to_string(),
        );

        assert_eq!(prov.memory_id, "mem-new");
        assert_eq!(prov.derived_from, Some("mem-parent".to_string()));
        assert_eq!(
            prov.source_type,
            ProvenanceSourceType::MemoryDerivation.to_string()
        );
        assert_eq!(prov.source_data["derivation"], "summarize");
    }

    #[test]
    fn test_memory_provenance_invalidation() {
        let prov = MemoryProvenanceRecord::from_user_annotation(
            "mem-123".to_string(),
            "user-456".to_string(),
        );

        assert!(prov.invalidated_at.is_none());

        let invalidated = prov.invalidate();
        assert!(invalidated.invalidated_at.is_some());
    }

    #[test]
    fn test_provenance_source_type_display() {
        assert_eq!(ProvenanceSourceType::RunTrace.to_string(), "run_trace");
        assert_eq!(
            ProvenanceSourceType::StateSnapshot.to_string(),
            "state_snapshot"
        );
        assert_eq!(
            ProvenanceSourceType::UserAnnotation.to_string(),
            "user_annotation"
        );
        assert_eq!(
            ProvenanceSourceType::MemoryDerivation.to_string(),
            "memory_derivation"
        );
    }
}
