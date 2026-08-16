use aivcs_core::deploy::deploy_by_digest;
use aivcs_core::domain::agent_spec::AgentSpec;
use aivcs_core::domain::error::AivcsError;
use aivcs_core::{replay_run, DeployByDigestRunner};
use chrono::{DateTime, Utc};
use oxidized_state::fakes::{MemoryReleaseRegistry, MemoryRunLedger};
use oxidized_state::storage_traits::{ReleaseRegistry, RunLedger, RunStatus};
use oxidized_state::{ContentDigest, RunEvent};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_spec(seed: &str) -> AgentSpec {
    AgentSpec::new(
        "abc123def456abc123def456abc123def456abc1".to_string(),
        format!("graph-{}", seed),
        format!("prompts-{}", seed),
        format!("tools-{}", seed),
        format!("config-{}", seed),
    )
    .expect("make_spec")
}

fn fixed_timestamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .expect("parse timestamp")
        .with_timezone(&Utc)
}

/// Promote a spec into a fresh registry, returning the registry.
async fn setup_registry(agent_name: &str, seed: &str) -> MemoryReleaseRegistry {
    let registry = MemoryReleaseRegistry::new();
    let spec = make_spec(seed);
    let digest = ContentDigest::try_from(spec.spec_digest.clone()).expect("valid digest");
    let metadata = oxidized_state::ReleaseMetadata {
        version_label: None,
        promoted_by: "ci".to_string(),
        notes: None,
    };
    registry
        .promote(agent_name, &digest, metadata)
        .await
        .expect("promote");
    registry
}

// ---------------------------------------------------------------------------
// deploy_by_digest (registry lookup → runner → replay) tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deployed_digest_matches_replay_golden() {
    let registry = setup_registry("agent-a", "v1").await;
    let ledger_a = MemoryRunLedger::new();
    let ledger_b = MemoryRunLedger::new();
    let ts = Some(fixed_timestamp());

    let result_a = deploy_by_digest(&registry, &ledger_a, "agent-a", ts)
        .await
        .expect("deploy_a");
    let result_b = deploy_by_digest(&registry, &ledger_b, "agent-a", ts)
        .await
        .expect("deploy_b");

    assert_eq!(
        result_a.summary.replay_digest,
        result_b.summary.replay_digest
    );
}

#[tokio::test]
async fn deploy_returns_correct_spec_digest() {
    let spec = make_spec("v1");
    let registry = setup_registry("agent-b", "v1").await;
    let ledger = MemoryRunLedger::new();

    let result = deploy_by_digest(&registry, &ledger, "agent-b", Some(fixed_timestamp()))
        .await
        .expect("deploy");

    assert_eq!(result.spec_digest, spec.spec_digest);
}

#[tokio::test]
async fn deploy_run_status_completed() {
    let registry = setup_registry("agent-c", "v1").await;
    let ledger = MemoryRunLedger::new();

    let result = deploy_by_digest(&registry, &ledger, "agent-c", Some(fixed_timestamp()))
        .await
        .expect("deploy");

    assert_eq!(result.summary.status, RunStatus::Completed);
}

#[tokio::test]
async fn deploy_emits_three_events() {
    let registry = setup_registry("agent-d", "v1").await;
    let ledger = MemoryRunLedger::new();

    let result = deploy_by_digest(&registry, &ledger, "agent-d", Some(fixed_timestamp()))
        .await
        .expect("deploy");

    // DeployByDigestRunner emits 3 events
    assert_eq!(result.summary.event_count, 3);
}

#[tokio::test]
async fn deploy_no_release_returns_error() {
    let registry = MemoryReleaseRegistry::new(); // no releases
    let ledger = MemoryRunLedger::new();

    let err = deploy_by_digest(&registry, &ledger, "nonexistent-agent", None)
        .await
        .unwrap_err();

    match err {
        AivcsError::ReleaseConflict(msg) => {
            assert!(msg.contains("nonexistent-agent"));
        }
        other => panic!("Expected ReleaseConflict, got {:?}", other),
    }
}

#[tokio::test]
async fn deploy_events_contain_spec_digest() {
    let registry = setup_registry("agent-f", "v1").await;
    let ledger = MemoryRunLedger::new();

    let result = deploy_by_digest(&registry, &ledger, "agent-f", Some(fixed_timestamp()))
        .await
        .expect("deploy");

    let events = ledger.get_events(&result.run_id).await.expect("get_events");
    assert_eq!(events[0].kind, "deploy_started");
    assert_eq!(
        events[0].payload["spec_digest"].as_str().unwrap(),
        result.spec_digest
    );
}

// ---------------------------------------------------------------------------
// DeployByDigestRunner (low-level runner) golden test from develop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deploy_by_digest_matches_replay_golden() {
    let ledger = MemoryRunLedger::new();
    let digest = ContentDigest::from_bytes(b"spec-deploy-golden-v1");
    let ts = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("parse timestamp")
        .with_timezone(&Utc);

    let output = DeployByDigestRunner::run_at(&ledger, &digest, "agent-golden", ts)
        .await
        .expect("run by digest");

    let (events, summary) = replay_run(&ledger, &output.run_id.0).await.expect("replay");

    let expected_events = vec![
        RunEvent {
            seq: 1,
            kind: "deploy_started".to_string(),
            payload: serde_json::json!({
                "spec_digest": digest.as_str(),
            }),
            timestamp: ts,
        },
        RunEvent {
            seq: 2,
            kind: "agent_executed".to_string(),
            payload: serde_json::json!({
                "agent_name": "agent-golden",
                "spec_digest": digest.as_str(),
            }),
            timestamp: ts,
        },
        RunEvent {
            seq: 3,
            kind: "deploy_completed".to_string(),
            payload: serde_json::json!({
                "success": true,
            }),
            timestamp: ts,
        },
    ];

    let expected_digest =
        ContentDigest::from_bytes(&serde_json::to_vec(&expected_events).expect("serialize events"));

    assert_eq!(events.len(), expected_events.len());
    for (actual, expected) in events.iter().zip(expected_events.iter()) {
        assert_eq!(actual.seq, expected.seq);
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.payload, expected.payload);
        assert_eq!(actual.timestamp, expected.timestamp);
    }
    assert_eq!(summary.event_count, 3);
    assert_eq!(summary.replay_digest, expected_digest.as_str());

    let run = ledger.get_run(&output.run_id).await.expect("get run");
    let run_summary = run.summary.expect("run summary");
    assert!(run_summary.final_state_digest.is_none());
}
