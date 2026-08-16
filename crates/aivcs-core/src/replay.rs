//! Replay functionality for recorded runs.
//!
//! This module provides the `replay_run` function to fetch and replay
//! all events for a given run from the `RunLedger`, computing a deterministic
//! digest over the event sequence for golden equality testing.

use tracing::instrument;

use oxidized_state::storage_traits::{
    ContentDigest, RunEvent, RunId, RunLedger, RunStatus as StorageRunStatus,
};

use crate::diff::state_diff::CHECKPOINT_SAVED_KIND;
use crate::domain::{AivcsError, Result};
use crate::metrics::METRICS;

/// Summary produced after replaying a run's events.
#[derive(Debug, Clone)]
pub struct ReplaySummary {
    /// The run ID that was replayed.
    pub run_id: String,
    /// The agent name that produced the run.
    pub agent_name: String,
    /// The final status of the run (Running, Completed, Failed).
    pub status: StorageRunStatus,
    /// Number of events in the run.
    pub event_count: usize,
    /// SHA-256 hex digest of `serde_json::to_vec(&events)`.
    ///
    /// Used for golden equality checks: two runs with identical events
    /// will produce identical digests.
    pub replay_digest: String,
    /// The spec digest recorded on the run at creation time.
    pub spec_digest: ContentDigest,
}

/// A resume point extracted from the last `CheckpointSaved` event in a run.
#[derive(Debug, Clone)]
pub struct ResumePoint {
    /// The checkpoint identifier from the event payload.
    pub checkpoint_id: String,
    /// The sequence number of the checkpoint event.
    pub checkpoint_seq: u64,
    /// The node identifier from the event payload.
    pub node_id: String,
    /// All events up to and including the checkpoint event.
    pub events_before: Vec<RunEvent>,
}

/// Verify that the spec digest recorded for `run_id_str` matches `expected_spec`.
///
/// This is a pre-flight gate that must pass before calling `replay_run` when
/// deterministic replay guarantees are required.
///
/// # Errors
///
/// - `AivcsError::StorageError` if the run does not exist.
/// - `AivcsError::DigestMismatch` if the recorded spec digest differs from `expected_spec`.
pub async fn verify_spec_digest(
    ledger: &dyn RunLedger,
    run_id_str: &str,
    expected_spec: &ContentDigest,
) -> Result<()> {
    let run_id = RunId(run_id_str.to_string());
    let record = ledger
        .get_run(&run_id)
        .await
        .map_err(|e| AivcsError::StorageError(e.to_string()))?;

    if record.spec_digest != *expected_spec {
        return Err(AivcsError::DigestMismatch {
            expected: expected_spec.as_str().to_string(),
            actual: record.spec_digest.as_str().to_string(),
        });
    }

    Ok(())
}

/// Find the last `CheckpointSaved` event in `run_id_str` and return a `ResumePoint`.
///
/// Returns `None` if the run has no checkpoint events.
///
/// # Errors
///
/// Returns `AivcsError::StorageError` when the run does not exist.
pub async fn find_resume_point(
    ledger: &dyn RunLedger,
    run_id_str: &str,
) -> Result<Option<ResumePoint>> {
    let run_id = RunId(run_id_str.to_string());
    let events = ledger
        .get_events(&run_id)
        .await
        .map_err(|e| AivcsError::StorageError(e.to_string()))?;

    // Scan for the last CheckpointSaved event
    let checkpoint_pos = events.iter().rposition(|e| e.kind == CHECKPOINT_SAVED_KIND);

    let Some(pos) = checkpoint_pos else {
        return Ok(None);
    };

    let cp_event = &events[pos];
    let checkpoint_id = cp_event
        .payload
        .get("checkpoint_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AivcsError::StorageError(format!(
                "checkpoint_saved event at seq {} missing 'checkpoint_id' field",
                cp_event.seq
            ))
        })?
        .to_string();
    let node_id = cp_event
        .payload
        .get("node_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AivcsError::StorageError(format!(
                "checkpoint_saved event at seq {} missing 'node_id' field",
                cp_event.seq
            ))
        })?
        .to_string();

    let events_before = events[..=pos].to_vec();

    Ok(Some(ResumePoint {
        checkpoint_id,
        checkpoint_seq: cp_event.seq,
        node_id,
        events_before,
    }))
}

/// Fetch and replay all events for `run_id_str` from the ledger.
///
/// Events are returned in `seq` ascending order. The `ReplaySummary`
/// includes a deterministic SHA-256 digest computed over the serialized
/// event sequence.
///
/// # Errors
///
/// Returns `AivcsError::StorageError` when the run does not exist.
/// This is the "missing artifact rejection" test gate.
///
/// # Example
///
/// ```no_run
/// # use aivcs_core::replay_run;
/// # use oxidized_state::fakes::MemoryRunLedger;
/// # use oxidized_state::RunLedger;
/// # use std::sync::Arc;
/// # #[tokio::main]
/// # async fn main() {
/// let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
/// // Would fail with StorageError for non-existent run
/// let _result = replay_run(&*ledger, "run-12345").await;
/// # }
/// ```
#[instrument(skip(ledger), fields(run_id = %run_id_str))]
pub async fn replay_run(
    ledger: &dyn RunLedger,
    run_id_str: &str,
) -> Result<(Vec<RunEvent>, ReplaySummary)> {
    let _span = crate::obs::RunSpan::enter(run_id_str);
    METRICS.inc_replays();

    let run_id = RunId(run_id_str.to_string());

    // Fetch run record — returns StorageError::RunNotFound if absent
    let record = ledger
        .get_run(&run_id)
        .await
        .map_err(|e| AivcsError::StorageError(e.to_string()))?;

    // Fetch events in seq order (both MemoryRunLedger and SurrealRunLedger sort by seq)
    let events = ledger
        .get_events(&run_id)
        .await
        .map_err(|e| AivcsError::StorageError(e.to_string()))?;

    // Compute deterministic digest: SHA-256 over serde_json::to_vec(&events)
    let events_json = serde_json::to_vec(&events).map_err(AivcsError::Serialization)?;
    let replay_digest = ContentDigest::from_bytes(&events_json).as_str().to_string();

    let summary = ReplaySummary {
        run_id: record.run_id.to_string(),
        agent_name: record.metadata.agent_name.clone(),
        status: record.status,
        event_count: events.len(),
        replay_digest,
        spec_digest: record.spec_digest,
    };

    Ok((events, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxidized_state::fakes::MemoryRunLedger;
    use oxidized_state::storage_traits::{ContentDigest, RunMetadata};
    use std::sync::Arc;

    /// Helper to build a run with the specified number of node pairs.
    async fn build_run(
        ledger: &dyn RunLedger,
        n_nodes: u32,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<RunId> {
        let spec_digest = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: Some("test_sha".to_string()),
            agent_name: "test_agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger
            .create_run(&spec_digest, metadata)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))?;

        // Emit events
        let mut seq = 1u64;

        // GraphStarted
        let event = RunEvent {
            seq,
            kind: "graph_started".to_string(),
            payload: serde_json::json!({}),
            timestamp,
        };
        ledger
            .append_event(&run_id, event)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))?;
        seq += 1;

        // N node pairs
        for i in 0..n_nodes {
            // NodeEntered
            let event = RunEvent {
                seq,
                kind: "node_entered".to_string(),
                payload: serde_json::json!({"node_id": format!("node_{}", i)}),
                timestamp,
            };
            ledger
                .append_event(&run_id, event)
                .await
                .map_err(|e| AivcsError::StorageError(e.to_string()))?;
            seq += 1;

            // NodeExited
            let event = RunEvent {
                seq,
                kind: "node_exited".to_string(),
                payload: serde_json::json!({"node_id": format!("node_{}", i)}),
                timestamp,
            };
            ledger
                .append_event(&run_id, event)
                .await
                .map_err(|e| AivcsError::StorageError(e.to_string()))?;
            seq += 1;
        }

        // GraphCompleted
        let event = RunEvent {
            seq,
            kind: "graph_completed".to_string(),
            payload: serde_json::json!({}),
            timestamp,
        };
        ledger
            .append_event(&run_id, event)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))?;

        // Complete the run
        let summary = oxidized_state::storage_traits::RunSummary {
            total_events: seq,
            final_state_digest: None,
            duration_ms: 1000,
            success: true,
        };
        ledger
            .complete_run(&run_id, summary)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))?;

        Ok(run_id)
    }

    #[tokio::test]
    async fn test_replay_golden_digest_equality() {
        let ledger_a: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let ledger_b: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        // Use fixed timestamp for both runs to ensure identical events
        let fixed_timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .expect("parse timestamp")
            .with_timezone(&chrono::Utc);

        // Build identical runs with same timestamp
        let run_a = build_run(&*ledger_a, 2, fixed_timestamp)
            .await
            .expect("build_run_a");
        let run_b = build_run(&*ledger_b, 2, fixed_timestamp)
            .await
            .expect("build_run_b");

        // Replay both
        let (_events_a, summary_a) = replay_run(&*ledger_a, &run_a.0).await.expect("replay_a");
        let (_events_b, summary_b) = replay_run(&*ledger_b, &run_b.0).await.expect("replay_b");

        // Golden digests must be equal (same events including timestamps)
        assert_eq!(summary_a.replay_digest, summary_b.replay_digest);
        assert_eq!(summary_a.event_count, summary_b.event_count);
    }

    #[tokio::test]
    async fn test_replay_missing_run_rejection() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let result = replay_run(&*ledger, "nonexistent-run-id").await;

        assert!(result.is_err());
        match result.unwrap_err() {
            AivcsError::StorageError(_) => { /* expected */ }
            other => panic!("Expected StorageError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_replay_event_order() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec_digest = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: Some("test_sha".to_string()),
            agent_name: "test_agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger
            .create_run(&spec_digest, metadata)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("create_run");

        // Append events in reverse seq order (out of order)
        let event3 = RunEvent {
            seq: 3,
            kind: "test_3".to_string(),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };
        ledger
            .append_event(&run_id, event3)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("append");

        let event1 = RunEvent {
            seq: 1,
            kind: "test_1".to_string(),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };
        ledger
            .append_event(&run_id, event1)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("append");

        let event2 = RunEvent {
            seq: 2,
            kind: "test_2".to_string(),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };
        ledger
            .append_event(&run_id, event2)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("append");

        let summary = oxidized_state::storage_traits::RunSummary {
            total_events: 3,
            final_state_digest: None,
            duration_ms: 100,
            success: true,
        };
        ledger
            .complete_run(&run_id, summary)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("complete");

        // Replay should return events in seq order: 1, 2, 3
        let (events, _summary) = replay_run(&*ledger, &run_id.0).await.expect("replay");

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(events[2].seq, 3);
    }

    #[tokio::test]
    async fn test_spec_digest_mismatch_rejected() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec_a = ContentDigest::from_bytes(b"spec_a");
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "test_agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger
            .create_run(&spec_a, metadata)
            .await
            .expect("create_run");

        // Verify with correct spec should pass
        verify_spec_digest(&*ledger, &run_id.0, &spec_a)
            .await
            .expect("correct spec should pass");

        // Verify with different spec should fail with DigestMismatch
        let spec_b = ContentDigest::from_bytes(b"spec_b");
        let result = verify_spec_digest(&*ledger, &run_id.0, &spec_b).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            AivcsError::DigestMismatch { expected, actual } => {
                assert_eq!(expected, spec_b.as_str());
                assert_eq!(actual, spec_a.as_str());
            }
            other => panic!("Expected DigestMismatch, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_find_resume_point_returns_latest_checkpoint() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger.create_run(&spec, metadata).await.expect("create");

        let ts = chrono::Utc::now();

        // First checkpoint at seq 1
        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 1,
                    kind: "checkpoint_saved".to_string(),
                    payload: serde_json::json!({ "checkpoint_id": "cp1", "node_id": "node_a" }),
                    timestamp: ts,
                },
            )
            .await
            .expect("append cp1");

        // Some other event
        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 2,
                    kind: "node_entered".to_string(),
                    payload: serde_json::json!({ "node_id": "node_b", "iteration": 1 }),
                    timestamp: ts,
                },
            )
            .await
            .expect("append node");

        // Second checkpoint at seq 3
        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 3,
                    kind: "checkpoint_saved".to_string(),
                    payload: serde_json::json!({ "checkpoint_id": "cp2", "node_id": "node_b" }),
                    timestamp: ts,
                },
            )
            .await
            .expect("append cp2");

        let resume = find_resume_point(&*ledger, &run_id.0)
            .await
            .expect("find_resume_point")
            .expect("should find a checkpoint");

        assert_eq!(resume.checkpoint_id, "cp2");
        assert_eq!(resume.node_id, "node_b");
        assert_eq!(resume.checkpoint_seq, 3);
        assert_eq!(resume.events_before.len(), 3);
    }

    #[tokio::test]
    async fn test_find_resume_point_no_checkpoint_returns_none() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger.create_run(&spec, metadata).await.expect("create");

        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 1,
                    kind: "graph_started".to_string(),
                    payload: serde_json::json!({}),
                    timestamp: chrono::Utc::now(),
                },
            )
            .await
            .expect("append");

        let resume = find_resume_point(&*ledger, &run_id.0)
            .await
            .expect("find_resume_point");

        assert!(resume.is_none());
    }

    #[tokio::test]
    async fn test_resume_point_events_before_includes_checkpoint() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger.create_run(&spec, metadata).await.expect("create");
        let ts = chrono::Utc::now();

        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 1,
                    kind: "graph_started".to_string(),
                    payload: serde_json::json!({}),
                    timestamp: ts,
                },
            )
            .await
            .expect("append");

        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 2,
                    kind: "checkpoint_saved".to_string(),
                    payload: serde_json::json!({ "checkpoint_id": "cp1", "node_id": "node_x" }),
                    timestamp: ts,
                },
            )
            .await
            .expect("append");

        // Event after checkpoint — should NOT be in events_before
        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 3,
                    kind: "node_entered".to_string(),
                    payload: serde_json::json!({ "node_id": "node_y", "iteration": 1 }),
                    timestamp: ts,
                },
            )
            .await
            .expect("append");

        let resume = find_resume_point(&*ledger, &run_id.0)
            .await
            .expect("find")
            .expect("some");

        // events_before should include exactly events 1 and 2 (up to and including checkpoint)
        assert_eq!(resume.events_before.len(), 2);
        let last = resume.events_before.last().expect("last");
        assert_eq!(last.kind, "checkpoint_saved");
        assert_eq!(last.seq, 2);
    }

    #[tokio::test]
    async fn test_find_resume_point_missing_field_returns_error() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger.create_run(&spec, metadata).await.expect("create");

        // checkpoint_saved event missing both checkpoint_id and node_id
        ledger
            .append_event(
                &run_id,
                RunEvent {
                    seq: 1,
                    kind: "checkpoint_saved".to_string(),
                    payload: serde_json::json!({}),
                    timestamp: chrono::Utc::now(),
                },
            )
            .await
            .expect("append");

        let result = find_resume_point(&*ledger, &run_id.0).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AivcsError::StorageError(msg) => {
                assert!(
                    msg.contains("checkpoint_id"),
                    "error should mention missing field, got: {msg}"
                );
            }
            other => panic!("Expected StorageError, got {:?}", other),
        }

        // checkpoint_saved with checkpoint_id but missing node_id
        let ledger2: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let metadata2 = RunMetadata {
            git_sha: None,
            agent_name: "agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };
        let run_id2 = ledger2.create_run(&spec, metadata2).await.expect("create");

        ledger2
            .append_event(
                &run_id2,
                RunEvent {
                    seq: 1,
                    kind: "checkpoint_saved".to_string(),
                    payload: serde_json::json!({ "checkpoint_id": "cp1" }),
                    timestamp: chrono::Utc::now(),
                },
            )
            .await
            .expect("append");

        let result2 = find_resume_point(&*ledger2, &run_id2.0).await;
        assert!(result2.is_err());
        match result2.unwrap_err() {
            AivcsError::StorageError(msg) => {
                assert!(
                    msg.contains("node_id"),
                    "error should mention missing field, got: {msg}"
                );
            }
            other => panic!("Expected StorageError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_replay_empty_run() {
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

        let spec_digest = ContentDigest::from_bytes(b"test_spec");
        let metadata = RunMetadata {
            git_sha: Some("test_sha".to_string()),
            agent_name: "test_agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };

        let run_id = ledger
            .create_run(&spec_digest, metadata)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("create_run");

        // Complete without appending any events
        let summary = oxidized_state::storage_traits::RunSummary {
            total_events: 0,
            final_state_digest: None,
            duration_ms: 0,
            success: true,
        };
        ledger
            .complete_run(&run_id, summary)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
            .expect("complete");

        let (events, summary) = replay_run(&*ledger, &run_id.0).await.expect("replay");

        assert_eq!(events.len(), 0);
        assert_eq!(summary.event_count, 0);
        // replay_digest should be valid hex (64 chars for SHA256)
        assert_eq!(summary.replay_digest.len(), 64);
        assert!(summary.replay_digest.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
