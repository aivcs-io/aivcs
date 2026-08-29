//! CI domain objects for AIVCS.
//!
//! These types are designed to be serialized, content-addressed, and linked
//! together by digest/ID so CI runs become durable, replayable objects.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Input snapshot metadata used to identify a CI run context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiSnapshot {
    /// Source repository commit SHA.
    pub repo_sha: String,
    /// Hash of working tree contents.
    pub workspace_hash: String,
    /// Hash of `.local-ci.toml` contents.
    pub local_ci_config_hash: String,
    /// Fingerprint of execution environment/toolchain.
    pub env_hash: String,
}

impl CiSnapshot {
    /// Returns a deterministic content digest for this snapshot.
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

/// A single command to execute inside CI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCommand {
    /// Binary or shell command.
    pub program: String,
    /// Program arguments.
    pub args: Vec<String>,
    /// Environment key/value overrides.
    pub env: BTreeMap<String, String>,
    /// Optional working directory relative to repo root.
    pub cwd: Option<String>,
}

impl CiCommand {
    /// Returns a deterministic content digest for this command.
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

/// Declarative definition for one CI step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiStepSpec {
    /// Human-friendly step name.
    pub name: String,
    /// Command to execute.
    pub command: CiCommand,
    /// Optional hard timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// If true, step failure does not fail the whole run.
    pub allow_failure: bool,
}

impl CiStepSpec {
    /// Returns a deterministic content digest for this step spec.
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

/// A CI pipeline specification (ordered step list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiPipelineSpec {
    /// Pipeline name.
    pub name: String,
    /// Ordered list of steps.
    pub steps: Vec<CiStepSpec>,
}

impl CiPipelineSpec {
    /// Returns a deterministic content digest for this pipeline spec.
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

/// Runtime status for a step/run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl CiRunStatus {
    /// True when the status represents a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Immutable result for an executed step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiStepResult {
    /// Step name (copied from spec for denormalized queryability).
    pub step_name: String,
    /// Final status.
    pub status: CiRunStatus,
    /// Process exit code when available.
    pub exit_code: Option<i32>,
    /// Start timestamp in RFC3339 format.
    pub started_at: Option<String>,
    /// End timestamp in RFC3339 format.
    pub finished_at: Option<String>,
    /// Digest of step stdout blob, if persisted.
    pub stdout_digest: Option<String>,
    /// Digest of step stderr blob, if persisted.
    pub stderr_digest: Option<String>,
}

/// Metadata for a produced artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiArtifact {
    /// Artifact logical name.
    pub name: String,
    /// Path relative to workspace root.
    pub path: String,
    /// Content digest for immutable retrieval.
    pub digest: String,
    /// Artifact size in bytes.
    pub size_bytes: u64,
    /// Optional MIME type.
    pub media_type: Option<String>,
}

/// Durable CI run object (content-linked to snapshot and pipeline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiRunRecord {
    /// Stable run identifier.
    pub run_id: String,
    /// Digest of [`CiSnapshot`].
    pub snapshot_digest: String,
    /// Digest of [`CiPipelineSpec`].
    pub pipeline_digest: String,
    /// Final run status.
    pub status: CiRunStatus,
    /// Per-step execution results.
    pub step_results: Vec<CiStepResult>,
    /// Produced artifacts.
    pub artifacts: Vec<CiArtifact>,
    /// Start timestamp in RFC3339 format.
    pub started_at: Option<String>,
    /// End timestamp in RFC3339 format.
    pub finished_at: Option<String>,
    /// Extra query metadata (branch, actor, host, etc).
    pub metadata: BTreeMap<String, String>,
}

impl CiRunRecord {
    /// Creates a queued run record from snapshot+pipeline digests.
    pub fn queued(snapshot_digest: impl AsRef<str>, pipeline_digest: impl AsRef<str>) -> Self {
        let snapshot_digest = snapshot_digest.as_ref().to_string();
        let pipeline_digest = pipeline_digest.as_ref().to_string();
        let run_id = digest_two(&snapshot_digest, &pipeline_digest);

        Self {
            run_id,
            snapshot_digest,
            pipeline_digest,
            status: CiRunStatus::Queued,
            step_results: Vec::new(),
            artifacts: Vec::new(),
            started_at: None,
            finished_at: None,
            metadata: BTreeMap::new(),
        }
    }

    /// Returns a deterministic content digest for this run record.
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

fn digest_json<T: Serialize>(value: &T) -> String {
    let bytes =
        serde_json::to_vec(value).expect("CI domain objects must be serializable for hashing");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn digest_two(left: &str, right: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(left.as_bytes());
    hasher.update([0u8]);
    hasher.update(right.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_digest_is_deterministic() {
        let a = CiSnapshot {
            repo_sha: "abc123".to_string(),
            workspace_hash: "ws1".to_string(),
            local_ci_config_hash: "cfg1".to_string(),
            env_hash: "env1".to_string(),
        };
        let b = a.clone();
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn pipeline_digest_changes_when_step_changes() {
        let mut env = BTreeMap::new();
        env.insert("RUST_LOG".to_string(), "info".to_string());

        let p1 = CiPipelineSpec {
            name: "default".to_string(),
            steps: vec![CiStepSpec {
                name: "test".to_string(),
                command: CiCommand {
                    program: "cargo".to_string(),
                    args: vec!["test".to_string()],
                    env: env.clone(),
                    cwd: None,
                },
                timeout_secs: Some(600),
                allow_failure: false,
            }],
        };

        let p2 = CiPipelineSpec {
            steps: vec![CiStepSpec {
                name: "test".to_string(),
                command: CiCommand {
                    program: "cargo".to_string(),
                    args: vec!["test".to_string(), "--all-features".to_string()],
                    env,
                    cwd: None,
                },
                timeout_secs: Some(600),
                allow_failure: false,
            }],
            ..p1.clone()
        };

        assert_ne!(p1.digest(), p2.digest());
    }

    #[test]
    fn queued_run_id_is_stable_for_same_inputs() {
        let r1 = CiRunRecord::queued("snap-a", "pipe-b");
        let r2 = CiRunRecord::queued("snap-a", "pipe-b");
        assert_eq!(r1.run_id, r2.run_id);
        assert_eq!(r1.status, CiRunStatus::Queued);
    }

    #[test]
    fn status_terminal_semantics_are_correct() {
        assert!(!CiRunStatus::Queued.is_terminal());
        assert!(!CiRunStatus::Running.is_terminal());
        assert!(CiRunStatus::Succeeded.is_terminal());
        assert!(CiRunStatus::Failed.is_terminal());
        assert!(CiRunStatus::Cancelled.is_terminal());
    }
}
