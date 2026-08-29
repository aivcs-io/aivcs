//! Golden digest contract test for deterministic replay.
//!
//! Builds a fixed-input run with seeded timestamps and asserts the replay
//! digest equals a hardcoded hex constant. Fails when serialization changes,
//! acting as a canary for accidental replay format drift.

use aivcs_core::replay_run;
use oxidized_state::fakes::MemoryRunLedger;
use oxidized_state::storage_traits::{
    ContentDigest, RunEvent, RunId, RunLedger, RunMetadata, RunSummary,
};
use std::sync::Arc;

/// Build the canonical seeded run used by the golden-digest pin test.
async fn build_seeded_run(ledger: &dyn RunLedger) -> RunId {
    let spec_digest = ContentDigest::from_bytes(b"golden_spec_v1");
    let metadata = RunMetadata {
        git_sha: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
        agent_name: "golden_agent".to_string(),
        tags: serde_json::json!({}),
        evaluation: Default::default(),
    };

    let run_id = ledger
        .create_run(&spec_digest, metadata)
        .await
        .expect("create_run");

    // Fixed timestamp — must never change or the digest will change.
    let ts = chrono::DateTime::parse_from_rfc3339("2024-06-01T12:00:00Z")
        .expect("parse ts")
        .with_timezone(&chrono::Utc);

    let events: Vec<RunEvent> = vec![
        RunEvent {
            seq: 1,
            kind: "graph_started".to_string(),
            payload: serde_json::json!({ "graph_name": "golden_graph", "entry_point": "start" }),
            timestamp: ts,
        },
        RunEvent {
            seq: 2,
            kind: "node_entered".to_string(),
            payload: serde_json::json!({ "node_id": "node_0", "iteration": 1 }),
            timestamp: ts,
        },
        RunEvent {
            seq: 3,
            kind: "node_exited".to_string(),
            payload: serde_json::json!({ "node_id": "node_0", "next_node": null, "duration_ms": 42 }),
            timestamp: ts,
        },
        RunEvent {
            seq: 4,
            kind: "graph_completed".to_string(),
            payload: serde_json::json!({ "iterations": 1, "duration_ms": 100 }),
            timestamp: ts,
        },
    ];

    for event in events {
        ledger.append_event(&run_id, event).await.expect("append");
    }

    let summary = RunSummary {
        total_events: 4,
        final_state_digest: None,
        duration_ms: 100,
        success: true,
    };
    ledger
        .complete_run(&run_id, summary)
        .await
        .expect("complete_run");

    run_id
}

/// Golden digest pin test.
///
/// The hardcoded constant below is the SHA-256 of the canonical JSON
/// serialization of the four-event sequence above.
///
/// If this test fails, it means the event serialization format has changed.
/// Update the constant **only** after verifying the change is intentional.
const GOLDEN_REPLAY_DIGEST: &str =
    "f5557fd15de85c61ff7507c0b97c671f8ba7471324794df16ceb54b906b0c3f8";

#[tokio::test]
async fn test_golden_digest_pin() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let run_id = build_seeded_run(&*ledger).await;

    let (_events, summary) = replay_run(&*ledger, &run_id.0).await.expect("replay_run");

    // Assert against the pinned literal — catches serialization drift even
    // when the change is internally consistent.
    assert_eq!(
        summary.replay_digest, GOLDEN_REPLAY_DIGEST,
        "Golden digest mismatch — serialization format may have changed. \
         If intentional, update GOLDEN_REPLAY_DIGEST."
    );

    // Digest must be a valid 64-char hex string.
    assert_eq!(summary.replay_digest.len(), 64);
    assert!(summary.replay_digest.chars().all(|c| c.is_ascii_hexdigit()));

    // event_count must be exactly 4.
    assert_eq!(summary.event_count, 4);

    // spec_digest must match what was used to create the run.
    let expected_spec = ContentDigest::from_bytes(b"golden_spec_v1");
    assert_eq!(summary.spec_digest, expected_spec);
}

/// Two independent ledgers with identical seeded input must produce the same digest.
#[tokio::test]
async fn test_golden_digest_identical_inputs_match() {
    let ts = chrono::DateTime::parse_from_rfc3339("2024-06-01T12:00:00Z")
        .expect("parse")
        .with_timezone(&chrono::Utc);

    async fn build(ledger: &dyn RunLedger, ts: chrono::DateTime<chrono::Utc>) -> RunId {
        let spec = ContentDigest::from_bytes(b"spec");
        let meta = RunMetadata {
            git_sha: None,
            agent_name: "a".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };
        let id = ledger.create_run(&spec, meta).await.unwrap();
        ledger
            .append_event(
                &id,
                RunEvent {
                    seq: 1,
                    kind: "graph_started".to_string(),
                    payload: serde_json::json!({}),
                    timestamp: ts,
                },
            )
            .await
            .unwrap();
        ledger
            .complete_run(
                &id,
                RunSummary {
                    total_events: 1,
                    final_state_digest: None,
                    duration_ms: 0,
                    success: true,
                },
            )
            .await
            .unwrap();
        id
    }

    let ledger_a: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let ledger_b: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let id_a = build(&*ledger_a, ts).await;
    let id_b = build(&*ledger_b, ts).await;

    let (_, sum_a) = replay_run(&*ledger_a, &id_a.0).await.unwrap();
    let (_, sum_b) = replay_run(&*ledger_b, &id_b.0).await.unwrap();

    assert_eq!(sum_a.replay_digest, sum_b.replay_digest);
}
