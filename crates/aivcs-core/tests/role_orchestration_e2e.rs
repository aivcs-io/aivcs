//! End-to-end role orchestration workflow tests.
//!
//! Exercises the full planner → coder → (reviewer + tester) → fixer pipeline
//! using `MemoryRunLedger` and deterministic stub executors. Also pins golden
//! replay digests to prove CI reproducibility.

use std::collections::HashSet;
use std::sync::Arc;

use aivcs_core::replay_run;
use aivcs_core::role_orchestration::{
    error::{RoleError, RoleResult},
    executor::{execute_roles_parallel, token_from_result, ParallelRoleConfig},
    merge::merge_parallel_outputs,
    roles::{AgentRole, RoleOutput, RoleTemplate},
    router::build_execution_plan,
};
use oxidized_state::fakes::MemoryRunLedger;
use oxidized_state::storage_traits::{ContentDigest, RunEvent, RunId, RunLedger, RunSummary};

fn spec() -> ContentDigest {
    ContentDigest::from_bytes(b"e2e-spec")
}

/// Deterministic stub: returns canned output for Reviewer and Tester.
#[allow(clippy::manual_async_fn)]
fn clean_stub(
    role: AgentRole,
    _run_id: RunId,
) -> impl std::future::Future<Output = RoleResult<RoleOutput>> {
    async move {
        Ok(match role {
            AgentRole::Reviewer => RoleOutput::Review {
                approved: true,
                comments: vec!["LGTM".to_string()],
                requires_fix: false,
            },
            AgentRole::Tester => RoleOutput::TestReport {
                passed: true,
                total_cases: 5,
                failed_cases: vec![],
                diagnostic_digest: None,
            },
            _ => RoleOutput::Fix {
                patch_digest: "stub-fix".to_string(),
                resolved_issues: vec![],
            },
        })
    }
}

/// Stub where Reviewer approves but Tester fails — forces a conflict.
#[allow(clippy::manual_async_fn)]
fn conflict_stub(
    role: AgentRole,
    _run_id: RunId,
) -> impl std::future::Future<Output = RoleResult<RoleOutput>> {
    async move {
        Ok(match role {
            AgentRole::Reviewer => RoleOutput::Review {
                approved: true,
                comments: vec!["looks fine".to_string()],
                requires_fix: false,
            },
            AgentRole::Tester => RoleOutput::TestReport {
                passed: false,
                total_cases: 5,
                failed_cases: vec!["test_foo".to_string()],
                diagnostic_digest: Some("diag-abc".to_string()),
            },
            _ => RoleOutput::Fix {
                patch_digest: "stub-fix".to_string(),
                resolved_issues: vec![],
            },
        })
    }
}

#[tokio::test]
async fn test_e2e_standard_pipeline_completes_cleanly() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let templates = RoleTemplate::standard_pipeline();

    // Build and validate execution plan
    let plan = build_execution_plan(
        "implement feature X",
        vec![AgentRole::Coder, AgentRole::Reviewer, AgentRole::Tester],
        &templates,
    )
    .unwrap();
    assert_eq!(plan.steps.len(), 3);

    // Execute Reviewer + Tester in parallel
    let results = execute_roles_parallel(
        Arc::clone(&ledger),
        "e2e-parent-1",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        clean_stub,
    )
    .await
    .unwrap();

    assert_eq!(results.len(), 2);

    // All RunIds must be distinct
    let ids: HashSet<_> = results.iter().map(|r| r.run_id.0.clone()).collect();
    assert_eq!(ids.len(), 2);

    // Convert to tokens and verify each
    let tokens: Vec<_> = results
        .into_iter()
        .map(|r| token_from_result(r).unwrap())
        .collect();
    for token in &tokens {
        assert!(token.verify().is_ok());
    }

    // Merge Reviewer + Tester outputs
    let merged = merge_parallel_outputs(&tokens[0], &tokens[1]).unwrap();
    assert!(merged.is_clean(), "standard pipeline should merge cleanly");
    assert!(merged.resolved.is_some());
}

#[tokio::test]
async fn test_e2e_conflict_surfaces_remediation_and_does_not_panic() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let results = execute_roles_parallel(
        ledger,
        "e2e-conflict-parent",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        conflict_stub,
    )
    .await
    .unwrap();

    let tokens: Vec<_> = results
        .into_iter()
        .map(|r| token_from_result(r).unwrap())
        .collect();

    let merged = merge_parallel_outputs(&tokens[0], &tokens[1]).unwrap();
    assert!(!merged.is_clean());
    assert_eq!(merged.conflicts.len(), 1);
    assert!(
        !merged.conflicts[0].remediation.is_empty(),
        "remediation must be non-empty"
    );
}

#[tokio::test]
async fn test_e2e_replay_digest_is_deterministic() {
    // Run the same seeded pipeline twice and compare replay digests.
    let ts = chrono::DateTime::parse_from_rfc3339("2024-06-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    async fn seed_run(ledger: &dyn RunLedger, ts: chrono::DateTime<chrono::Utc>) -> RunId {
        let spec = ContentDigest::from_bytes(b"e2e-spec");
        use oxidized_state::storage_traits::RunMetadata;
        let meta = RunMetadata {
            git_sha: Some("aabbcc".to_string()),
            agent_name: "e2e-agent".to_string(),
            tags: serde_json::json!({}),
            evaluation: Default::default(),
        };
        let run_id = ledger.create_run(&spec, meta).await.unwrap();

        let events = vec![
            RunEvent {
                seq: 1,
                kind: "graph_started".to_string(),
                payload: serde_json::json!({ "graph_name": "e2e", "entry_point": "start" }),
                timestamp: ts,
            },
            RunEvent {
                seq: 2,
                kind: "node_entered".to_string(),
                payload: serde_json::json!({ "node_id": "role_reviewer", "iteration": 1 }),
                timestamp: ts,
            },
            RunEvent {
                seq: 3,
                kind: "node_exited".to_string(),
                payload: serde_json::json!({ "node_id": "role_reviewer", "next_node": null, "duration_ms": 10 }),
                timestamp: ts,
            },
            RunEvent {
                seq: 4,
                kind: "graph_completed".to_string(),
                payload: serde_json::json!({ "iterations": 1, "duration_ms": 50 }),
                timestamp: ts,
            },
        ];
        for e in events {
            ledger.append_event(&run_id, e).await.unwrap();
        }
        ledger
            .complete_run(
                &run_id,
                RunSummary {
                    total_events: 4,
                    final_state_digest: None,
                    duration_ms: 50,
                    success: true,
                },
            )
            .await
            .unwrap();
        run_id
    }

    let ledger_a: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
    let ledger_b: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let id_a = seed_run(&*ledger_a, ts).await;
    let id_b = seed_run(&*ledger_b, ts).await;

    let (_, sum_a) = replay_run(&*ledger_a, &id_a.0).await.unwrap();
    let (_, sum_b) = replay_run(&*ledger_b, &id_b.0).await.unwrap();

    assert_eq!(
        sum_a.replay_digest, sum_b.replay_digest,
        "identical seeded runs must produce identical replay digests"
    );
    assert_eq!(sum_a.event_count, 4);
}

#[tokio::test]
async fn test_e2e_parallel_role_run_ids_are_unique_across_two_pipeline_runs() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let run1 = execute_roles_parallel(
        Arc::clone(&ledger),
        "pipeline-run-1",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        clean_stub,
    )
    .await
    .unwrap();

    let run2 = execute_roles_parallel(
        Arc::clone(&ledger),
        "pipeline-run-2",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        clean_stub,
    )
    .await
    .unwrap();

    let ids1: HashSet<_> = run1.iter().map(|r| r.run_id.0.clone()).collect();
    let ids2: HashSet<_> = run2.iter().map(|r| r.run_id.0.clone()).collect();

    // No RunId from run1 should appear in run2
    assert!(
        ids1.is_disjoint(&ids2),
        "RunIds must be unique across separate pipeline executions"
    );
}

#[tokio::test]
async fn test_e2e_handoff_token_digest_changes_when_output_mutated() {
    // Build a valid token, then mutate its output field in place and verify it fails.
    let original_output = RoleOutput::Review {
        approved: true,
        comments: vec!["ok".to_string()],
        requires_fix: false,
    };
    let mut token = aivcs_core::HandoffToken::new(original_output).unwrap();

    // Token verifies cleanly before mutation.
    assert!(token.verify().is_ok());

    // Mutate the output without recomputing the digest.
    token.output = RoleOutput::Review {
        approved: false,
        comments: vec!["NOT ok".to_string()],
        requires_fix: true,
    };

    let result = token.verify();
    assert!(result.is_err(), "mutated token must fail verification");
    match result.unwrap_err() {
        aivcs_core::AivcsError::DigestMismatch { .. } => {}
        other => panic!("Expected DigestMismatch, got {:?}", other),
    }
}

#[tokio::test]
async fn test_e2e_execution_plan_validates_sequence() {
    let templates = RoleTemplate::standard_pipeline();

    // Valid: Coder accepts from Planner
    assert!(build_execution_plan(
        "task",
        vec![AgentRole::Planner, AgentRole::Coder],
        &templates,
    )
    .is_ok());

    // Invalid: Planner does not accept from Coder
    let err = build_execution_plan(
        "bad task",
        vec![AgentRole::Coder, AgentRole::Planner],
        &templates,
    )
    .unwrap_err();
    assert!(matches!(err, RoleError::UnauthorizedHandoff { .. }));
}
