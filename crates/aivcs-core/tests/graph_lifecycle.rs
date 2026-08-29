//! Integration tests for GraphRunRecorder (graph lifecycle → RunLedger).

use std::sync::Arc;

use aivcs_core::domain::run::{Event, EventKind};
use aivcs_core::recording::GraphRunRecorder;
use oxidized_state::{
    fakes::MemoryRunLedger, ContentDigest, RunLedger, RunMetadata, RunStatus, RunSummary,
};
use uuid::Uuid;

fn make_event(run_id: Uuid, seq: u64, kind: EventKind) -> Event {
    Event::new(run_id, seq, kind, serde_json::json!({}))
}

fn test_metadata() -> RunMetadata {
    RunMetadata {
        git_sha: Some("abc123".to_string()),
        agent_name: "test-agent".to_string(),
        tags: serde_json::json!({}),
        evaluation: Default::default(),
    }
}

#[tokio::test]
async fn happy_path_two_node_lifecycle() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let spec_digest = ContentDigest::from_bytes(b"test-spec");

    let recorder = GraphRunRecorder::start(ledger.clone(), &spec_digest, test_metadata())
        .await
        .expect("start");
    let run_id_uuid = Uuid::new_v4();

    let events = vec![
        make_event(run_id_uuid, 1, EventKind::GraphStarted),
        make_event(
            run_id_uuid,
            2,
            EventKind::NodeEntered {
                node_id: "n1".to_string(),
                iteration: 1,
            },
        ),
        make_event(
            run_id_uuid,
            3,
            EventKind::NodeExited {
                node_id: "n1".to_string(),
                next_node: Some("n2".to_string()),
                duration_ms: 10,
            },
        ),
        make_event(
            run_id_uuid,
            4,
            EventKind::NodeEntered {
                node_id: "n2".to_string(),
                iteration: 1,
            },
        ),
        make_event(
            run_id_uuid,
            5,
            EventKind::NodeExited {
                node_id: "n2".to_string(),
                next_node: None,
                duration_ms: 20,
            },
        ),
        make_event(
            run_id_uuid,
            6,
            EventKind::GraphCompleted {
                iterations: 1,
                duration_ms: 100,
            },
        ),
    ];

    for e in &events {
        recorder.record(e).await.expect("record");
    }

    let rid = recorder.run_id().clone();
    recorder
        .finish_ok(RunSummary {
            total_events: 6,
            final_state_digest: None,
            duration_ms: 100,
            success: true,
        })
        .await
        .expect("finish_ok");

    // Verify events
    let stored = ledger.get_events(&rid).await.expect("get_events");
    assert_eq!(stored.len(), 6);

    let kinds: Vec<&str> = stored.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "graph_started",
            "node_entered",
            "node_exited",
            "node_entered",
            "node_exited",
            "graph_completed",
        ]
    );

    // Verify run status
    let record = ledger.get_run(&rid).await.expect("get_run");
    assert_eq!(record.status, RunStatus::Completed);
}

#[tokio::test]
async fn failure_path_node_failed() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let spec_digest = ContentDigest::from_bytes(b"test-spec");

    let recorder = GraphRunRecorder::start(ledger.clone(), &spec_digest, test_metadata())
        .await
        .expect("start");
    let run_id_uuid = Uuid::new_v4();

    let events = vec![
        make_event(run_id_uuid, 1, EventKind::GraphStarted),
        make_event(
            run_id_uuid,
            2,
            EventKind::NodeEntered {
                node_id: "n1".to_string(),
                iteration: 1,
            },
        ),
        make_event(
            run_id_uuid,
            3,
            EventKind::NodeFailed {
                node_id: "n1".to_string(),
                error: "fail".to_string(),
            },
        ),
        make_event(
            run_id_uuid,
            4,
            EventKind::GraphFailed {
                error: "graph fail".to_string(),
            },
        ),
    ];

    for e in &events {
        recorder.record(e).await.expect("record");
    }

    let rid = recorder.run_id().clone();
    recorder
        .finish_err(RunSummary {
            total_events: 4,
            final_state_digest: None,
            duration_ms: 50,
            success: false,
        })
        .await
        .expect("finish_err");

    // Verify events
    let stored = ledger.get_events(&rid).await.expect("get_events");
    assert_eq!(stored.len(), 4);

    let kinds: Vec<&str> = stored.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "graph_started",
            "node_entered",
            "node_failed",
            "graph_failed"
        ]
    );

    // Verify run status
    let record = ledger.get_run(&rid).await.expect("get_run");
    assert_eq!(record.status, RunStatus::Failed);
}
