//! Run trace artifact persistence and retention policy.
//!
//! A [`RunTraceArtifact`] is a self-contained, content-verified record of a
//! completed run. It includes the event sequence, a SHA-256 replay digest,
//! and provenance fields from the [`RunRecord`].
//!
//! Artifacts are written to `<dir>/<run_id>/trace.json` with a companion
//! `<dir>/<run_id>/trace.digest` file for integrity checks.
//!
//! [`RetentionPolicy`] can prune an artifact directory by age or count.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use oxidized_state::storage_traits::{ContentDigest, RunEvent, RunRecord};

use crate::domain::{AivcsError, Result};

/// A self-contained, integrity-checked record of a completed run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTraceArtifact {
    /// The run identifier.
    pub run_id: String,
    /// Hex string of the spec digest recorded at run creation.
    pub spec_digest: String,
    /// Agent name from the run metadata.
    pub agent_name: String,
    /// Final status string (`"Completed"` or `"Failed"`).
    pub status: String,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run reached a terminal state, if known.
    pub completed_at: Option<DateTime<Utc>>,
    /// All events in seq order.
    pub events: Vec<RunEvent>,
    /// SHA-256 hex digest of `serde_json::to_vec(&events)`.
    pub replay_digest: String,
    /// Number of events.
    pub event_count: usize,
}

impl RunTraceArtifact {
    /// Construct a `RunTraceArtifact` from a run record, event list, and pre-computed digest.
    pub fn from_replay(record: &RunRecord, events: Vec<RunEvent>, replay_digest: String) -> Self {
        let status = format!("{:?}", record.status);
        Self {
            run_id: record.run_id.to_string(),
            spec_digest: record.spec_digest.as_str().to_string(),
            agent_name: record.metadata.agent_name.clone(),
            status,
            created_at: record.created_at,
            completed_at: record.completed_at,
            event_count: events.len(),
            events,
            replay_digest,
        }
    }
}

/// Write a `RunTraceArtifact` to `<dir>/<run_id>/trace.json`.
///
/// Also writes `<dir>/<run_id>/trace.digest` containing the replay digest for
/// out-of-band verification.
///
/// Returns the path to `trace.json`.
pub fn write_trace_artifact(artifact: &RunTraceArtifact, dir: &Path) -> Result<PathBuf> {
    let run_dir = dir.join(&artifact.run_id);
    std::fs::create_dir_all(&run_dir)?;

    let trace_path = run_dir.join("trace.json");
    let digest_path = run_dir.join("trace.digest");

    let json = serde_json::to_vec_pretty(artifact)?;
    std::fs::write(&trace_path, &json)?;
    std::fs::write(&digest_path, artifact.replay_digest.as_bytes())?;

    Ok(trace_path)
}

/// Read and integrity-verify a `RunTraceArtifact` from `<dir>/<run_id>/trace.json`.
///
/// Performs two integrity checks:
/// 1. Reads `trace.digest` (companion file) and compares it to the `replay_digest`
///    stored inside `trace.json`. Detects out-of-band tampering of `trace.json`
///    when the companion digest file was not also updated.
/// 2. Recomputes the SHA-256 digest of the event list and compares it to the
///    stored `replay_digest`. Detects in-place event tampering.
///
/// Returns `AivcsError::DigestMismatch` if either check fails.
pub fn read_trace_artifact(run_id: &str, dir: &Path) -> Result<RunTraceArtifact> {
    let run_dir = dir.join(run_id);
    let trace_path = run_dir.join("trace.json");
    let digest_path = run_dir.join("trace.digest");

    let json = std::fs::read(&trace_path)?;
    let artifact: RunTraceArtifact = serde_json::from_slice(&json)?;

    // Check 1: verify companion trace.digest matches the JSON's replay_digest
    if digest_path.exists() {
        let companion_digest = std::fs::read_to_string(&digest_path)?.trim().to_string();
        if companion_digest != artifact.replay_digest {
            return Err(AivcsError::DigestMismatch {
                expected: companion_digest,
                actual: artifact.replay_digest.clone(),
            });
        }
    }

    // Check 2: re-derive the digest from events and verify it matches
    let events_json = serde_json::to_vec(&artifact.events)?;
    let actual_digest = ContentDigest::from_bytes(&events_json).as_str().to_string();

    if actual_digest != artifact.replay_digest {
        return Err(AivcsError::DigestMismatch {
            expected: artifact.replay_digest.clone(),
            actual: actual_digest,
        });
    }

    Ok(artifact)
}

/// Retention policy for pruning run trace artifact directories.
#[derive(Debug, Clone, Default)]
pub struct RetentionPolicy {
    /// Remove runs older than this many days. `None` means no age limit.
    pub max_age_days: Option<u64>,
    /// Keep at most this many runs (newest first). `None` means no count limit.
    pub max_runs: Option<usize>,
}

impl RetentionPolicy {
    /// Scan `<dir>/*/trace.json`, apply retention rules, and delete runs that
    /// exceed the policy.
    ///
    /// Returns the number of pruned entries.
    ///
    /// Rules are applied in order:
    /// 1. Age: runs with `created_at` older than `max_age_days` are deleted.
    /// 2. Count: after age pruning, if more than `max_runs` remain, the oldest
    ///    are deleted until the count limit is satisfied.
    pub fn prune(&self, dir: &Path) -> Result<usize> {
        // Collect all run artifact directories that contain a trace.json
        let mut entries: Vec<(DateTime<Utc>, PathBuf)> = Vec::new();

        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(AivcsError::Io(e)),
        };

        for entry in read_dir {
            let entry = entry?;
            let trace_path = entry.path().join("trace.json");
            if !trace_path.exists() {
                continue;
            }
            let json = std::fs::read(&trace_path)?;
            if let Ok(artifact) = serde_json::from_slice::<RunTraceArtifact>(&json) {
                entries.push((artifact.created_at, entry.path()));
            }
        }

        // Sort by created_at descending (newest first) for count-based pruning
        entries.sort_by_key(|e| std::cmp::Reverse(e.0));

        let mut pruned = 0usize;
        let now = Utc::now();

        // Age-based pruning
        if let Some(max_days) = self.max_age_days {
            let cutoff = now - chrono::Duration::days(max_days as i64);
            let mut kept = Vec::new();
            for (created_at, path) in entries {
                if created_at < cutoff {
                    std::fs::remove_dir_all(&path)?;
                    pruned += 1;
                } else {
                    kept.push((created_at, path));
                }
            }
            entries = kept;
        }

        // Count-based pruning (entries is already newest-first)
        if let Some(max_runs) = self.max_runs {
            if entries.len() > max_runs {
                for (_, path) in entries.drain(max_runs..) {
                    std::fs::remove_dir_all(&path)?;
                    pruned += 1;
                }
            }
        }

        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oxidized_state::storage_traits::{RunId, RunMetadata, RunStatus, RunSummary};
    use tempfile::tempdir;

    fn make_record(run_id: &str, created_at: DateTime<Utc>) -> RunRecord {
        RunRecord {
            run_id: RunId(run_id.to_string()),
            spec_digest: ContentDigest::from_bytes(b"spec"),
            metadata: RunMetadata {
                git_sha: None,
                agent_name: "agent".to_string(),
                tags: serde_json::json!({}),
                evaluation: Default::default(),
            },
            status: RunStatus::Completed,
            summary: Some(RunSummary {
                total_events: 1,
                final_state_digest: None,
                duration_ms: 10,
                success: true,
            }),
            created_at,
            completed_at: Some(created_at),
        }
    }

    fn make_events(ts: DateTime<Utc>) -> Vec<RunEvent> {
        vec![RunEvent {
            seq: 1,
            kind: "graph_started".to_string(),
            payload: serde_json::json!({}),
            timestamp: ts,
        }]
    }

    #[test]
    fn test_write_and_read_trace_artifact_roundtrip() {
        let dir = tempdir().expect("tempdir");
        let ts = Utc::now();
        let events = make_events(ts);
        let events_json = serde_json::to_vec(&events).unwrap();
        let digest = ContentDigest::from_bytes(&events_json).as_str().to_string();

        let record = make_record("run-abc", ts);
        let artifact = RunTraceArtifact::from_replay(&record, events.clone(), digest.clone());

        let path = write_trace_artifact(&artifact, dir.path()).expect("write");
        assert!(path.exists());

        let loaded = read_trace_artifact("run-abc", dir.path()).expect("read");

        assert_eq!(loaded.run_id, "run-abc");
        assert_eq!(loaded.agent_name, "agent");
        assert_eq!(loaded.replay_digest, digest);
        assert_eq!(loaded.event_count, 1);
        assert_eq!(loaded.events.len(), 1);
    }

    #[test]
    fn test_read_trace_artifact_digest_mismatch_rejected() {
        let dir = tempdir().expect("tempdir");
        let ts = Utc::now();
        let events = make_events(ts);

        let record = make_record("run-xyz", ts);
        // Use a deliberately wrong digest
        let artifact = RunTraceArtifact::from_replay(&record, events, "a".repeat(64));

        // Write with tampered digest
        let run_dir = dir.path().join("run-xyz");
        std::fs::create_dir_all(&run_dir).unwrap();
        let json = serde_json::to_vec_pretty(&artifact).unwrap();
        std::fs::write(run_dir.join("trace.json"), &json).unwrap();

        let result = read_trace_artifact("run-xyz", dir.path());
        assert!(result.is_err());
        match result.unwrap_err() {
            AivcsError::DigestMismatch { .. } => {}
            other => panic!("Expected DigestMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_retention_policy_prunes_old_runs() {
        let dir = tempdir().expect("tempdir");
        let now = Utc::now();

        // Create three runs: one recent, two old
        for (id, days_ago) in [("run-new", 0i64), ("run-old1", 10), ("run-old2", 20)] {
            let ts = now - chrono::Duration::days(days_ago);
            let events = make_events(ts);
            let events_json = serde_json::to_vec(&events).unwrap();
            let digest = ContentDigest::from_bytes(&events_json).as_str().to_string();
            let record = make_record(id, ts);
            let artifact = RunTraceArtifact::from_replay(&record, events, digest);
            write_trace_artifact(&artifact, dir.path()).expect("write");
        }

        let policy = RetentionPolicy {
            max_age_days: Some(5),
            max_runs: None,
        };

        let pruned = policy.prune(dir.path()).expect("prune");
        assert_eq!(pruned, 2, "should prune the two old runs");

        // Only the recent run should remain
        assert!(dir.path().join("run-new").join("trace.json").exists());
        assert!(!dir.path().join("run-old1").exists());
        assert!(!dir.path().join("run-old2").exists());
    }

    #[test]
    fn test_retention_policy_max_runs() {
        let dir = tempdir().expect("tempdir");
        let now = Utc::now();

        // Create 4 runs with different ages
        for (id, days_ago) in [("run-1", 0i64), ("run-2", 1), ("run-3", 2), ("run-4", 3)] {
            let ts = now - chrono::Duration::days(days_ago);
            let events = make_events(ts);
            let events_json = serde_json::to_vec(&events).unwrap();
            let digest = ContentDigest::from_bytes(&events_json).as_str().to_string();
            let record = make_record(id, ts);
            let artifact = RunTraceArtifact::from_replay(&record, events, digest);
            write_trace_artifact(&artifact, dir.path()).expect("write");
        }

        let policy = RetentionPolicy {
            max_age_days: None,
            max_runs: Some(2),
        };

        let pruned = policy.prune(dir.path()).expect("prune");
        assert_eq!(pruned, 2, "should prune 2 oldest runs");

        // The two newest (run-1, run-2) should remain
        assert!(dir.path().join("run-1").join("trace.json").exists());
        assert!(dir.path().join("run-2").join("trace.json").exists());
        assert!(!dir.path().join("run-3").exists());
        assert!(!dir.path().join("run-4").exists());
    }
}
