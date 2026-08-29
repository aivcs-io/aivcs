//! Integration tests for the HITL controls module.

use chrono::{Duration, Utc};
use uuid::Uuid;

use aivcs_core::hitl_controls::{
    apply_intervention, evaluate_checkpoint, submit_vote, ApprovalCheckpoint, ApprovalPolicy,
    ApprovalVote, CheckpointStatus, DecisionSummary, ExplainabilitySummary, HitlArtifact,
    HitlError, Intervention, InterventionAction, RiskTier, VoteDecision,
};

fn explanation(desc: &str) -> ExplainabilitySummary {
    ExplainabilitySummary {
        action_description: desc.into(),
        changes_summary: "test changes".into(),
        flag_reason: "integration test".into(),
    }
}

fn checkpoint(label: &str, tier: RiskTier) -> ApprovalCheckpoint {
    ApprovalCheckpoint::new(
        label,
        Uuid::new_v4(),
        tier,
        explanation(label),
        Some(300),
        Utc::now(),
    )
}

fn vote(voter: &str, cp: &ApprovalCheckpoint, decision: VoteDecision) -> ApprovalVote {
    ApprovalVote::new(voter, &cp.checkpoint_id, decision, None, Utc::now())
}

// ── Risk tier routing via policy ──

#[test]
fn policy_routes_deploy_prod_to_critical() {
    let policy = ApprovalPolicy::standard();
    let (tier, timeout) = policy.evaluate_risk("deploy-prod-us-east");
    assert_eq!(tier, RiskTier::Critical);
    assert_eq!(timeout, Some(600));
}

#[test]
fn policy_routes_unknown_to_low() {
    let policy = ApprovalPolicy::standard();
    let (tier, _) = policy.evaluate_risk("run-lint");
    assert_eq!(tier, RiskTier::Low);
}

// ── Approval flow: low risk auto-approves ──

#[test]
fn low_risk_auto_approves_with_no_votes() {
    let cp = checkpoint("run-tests", RiskTier::Low);
    let status = evaluate_checkpoint(&cp, &[], Utc::now());
    assert_eq!(status, Some(CheckpointStatus::Approved));
}

// ── Approval flow: high risk needs one approval ──

#[test]
fn high_risk_pending_with_no_votes() {
    let cp = checkpoint("deploy-staging", RiskTier::High);
    assert_eq!(evaluate_checkpoint(&cp, &[], Utc::now()), None);
}

#[test]
fn high_risk_approved_after_one_vote() {
    let cp = checkpoint("deploy-staging", RiskTier::High);
    let v = vote("alice", &cp, VoteDecision::Approve);
    assert_eq!(
        evaluate_checkpoint(&cp, &[v], Utc::now()),
        Some(CheckpointStatus::Approved)
    );
}

// ── Approval flow: critical risk needs two approvals ──

#[test]
fn critical_risk_needs_two_approvals() {
    let cp = checkpoint("deploy-prod", RiskTier::Critical);

    let v1 = vote("alice", &cp, VoteDecision::Approve);
    assert_eq!(
        evaluate_checkpoint(&cp, std::slice::from_ref(&v1), Utc::now()),
        None
    );

    let v2 = vote("bob", &cp, VoteDecision::Approve);
    assert_eq!(
        evaluate_checkpoint(&cp, &[v1, v2], Utc::now()),
        Some(CheckpointStatus::Approved)
    );
}

// ── Rejection overrides approvals ──

#[test]
fn rejection_overrides_approvals() {
    let cp = checkpoint("deploy-prod", RiskTier::Critical);
    let votes = vec![
        vote("alice", &cp, VoteDecision::Approve),
        vote("bob", &cp, VoteDecision::Reject),
    ];
    let status = evaluate_checkpoint(&cp, &votes, Utc::now());
    assert!(matches!(status, Some(CheckpointStatus::Rejected { .. })));
}

// ── Expiry ──

#[test]
fn checkpoint_expires_after_timeout() {
    let now = Utc::now();
    let cp = ApprovalCheckpoint::new(
        "deploy-staging",
        Uuid::new_v4(),
        RiskTier::High,
        explanation("deploy-staging"),
        Some(60),
        now,
    );
    let future = now + Duration::seconds(61);
    assert_eq!(
        evaluate_checkpoint(&cp, &[], future),
        Some(CheckpointStatus::Expired)
    );
}

// ── Duplicate vote rejected ──

#[test]
fn duplicate_vote_rejected() {
    let mut cp = checkpoint("deploy-staging", RiskTier::High);
    let v = vote("alice", &cp, VoteDecision::Approve);
    assert!(submit_vote(&mut cp, &v, &[], Utc::now()).is_ok());
    let err = submit_vote(&mut cp, &v, std::slice::from_ref(&v), Utc::now()).unwrap_err();
    assert!(matches!(err, HitlError::DuplicateVote { .. }));
}

// ── Intervention: pause / continue ──

#[test]
fn pause_and_continue_intervention() {
    let mut cp = checkpoint("deploy-staging", RiskTier::High);

    // Pause.
    let pause = Intervention::new(
        cp.run_id,
        Some(cp.checkpoint_id.clone()),
        "ops-alice",
        InterventionAction::Pause,
        Some("investigating issue".into()),
        Utc::now(),
    );
    apply_intervention(&mut cp, &pause).unwrap();
    assert_eq!(cp.status, CheckpointStatus::Paused);

    // Continue.
    let cont = Intervention::new(
        cp.run_id,
        Some(cp.checkpoint_id.clone()),
        "ops-alice",
        InterventionAction::Continue,
        None,
        Utc::now(),
    );
    apply_intervention(&mut cp, &cont).unwrap();
    assert_eq!(cp.status, CheckpointStatus::Pending);
}

// ── Intervention: abort ──

#[test]
fn abort_intervention_rejects_checkpoint() {
    let mut cp = checkpoint("deploy-staging", RiskTier::High);
    let abort = Intervention::new(
        cp.run_id,
        Some(cp.checkpoint_id.clone()),
        "ops-bob",
        InterventionAction::Abort {
            reason: "wrong target".into(),
        },
        None,
        Utc::now(),
    );
    apply_intervention(&mut cp, &abort).unwrap();
    assert!(matches!(cp.status, CheckpointStatus::Rejected { .. }));
}

// ── Audit artifact persistence ──

#[test]
fn artifact_write_read_roundtrip() {
    let mut cp = checkpoint("schema-migration", RiskTier::Critical);
    cp.status = CheckpointStatus::Approved;

    let votes = vec![
        vote("alice", &cp, VoteDecision::Approve),
        vote("bob", &cp, VoteDecision::Approve),
    ];
    let artifact = HitlArtifact::finalize(cp, votes, vec![], Utc::now());
    assert!(artifact.verify_integrity());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hitl-audit.json");
    aivcs_core::hitl_controls::write_hitl_artifact(&artifact, &path).unwrap();
    let loaded = aivcs_core::hitl_controls::read_hitl_artifact(&path).unwrap();
    assert_eq!(artifact, loaded);
    assert!(loaded.verify_integrity());
}

// ── Decision summary ──

#[test]
fn decision_summary_captures_outcome() {
    let mut cp = checkpoint("deploy-prod", RiskTier::Critical);
    cp.status = CheckpointStatus::Approved;
    let votes = vec![
        vote("alice", &cp, VoteDecision::Approve),
        vote("bob", &cp, VoteDecision::Approve),
    ];
    let artifact = HitlArtifact::finalize(cp, votes, vec![], Utc::now());
    let summary = DecisionSummary::from_artifact(&artifact);
    assert_eq!(summary.outcome, "approved");
    assert_eq!(summary.approval_count, 2);
    assert_eq!(summary.risk_tier, RiskTier::Critical);
}

// ── Explainability: every checkpoint carries explanation ──

#[test]
fn checkpoint_carries_explanation() {
    let cp = checkpoint("publish-crate", RiskTier::High);
    assert!(!cp.explanation.action_description.is_empty());
    assert!(!cp.explanation.changes_summary.is_empty());
    assert!(!cp.explanation.flag_reason.is_empty());
}

// ── Full end-to-end: policy → checkpoint → vote → artifact ──

#[test]
fn end_to_end_approval_flow() {
    // 1. Policy evaluates risk.
    let policy = ApprovalPolicy::standard();
    let label = "deploy-prod-us-west";
    let (tier, timeout) = policy.evaluate_risk(label);
    assert_eq!(tier, RiskTier::Critical);

    // 2. Create checkpoint.
    let now = Utc::now();
    let mut cp = ApprovalCheckpoint::new(
        label,
        Uuid::new_v4(),
        tier,
        explanation(label),
        timeout,
        now,
    );

    // 3. Submit votes.
    let v1 = ApprovalVote::new("alice", &cp.checkpoint_id, VoteDecision::Approve, None, now);
    submit_vote(&mut cp, &v1, &[], now).unwrap();

    let v2 = ApprovalVote::new("bob", &cp.checkpoint_id, VoteDecision::Approve, None, now);
    submit_vote(&mut cp, &v2, std::slice::from_ref(&v1), now).unwrap();

    // 4. Evaluate → approved.
    let all_votes = vec![v1, v2];
    let new_status = evaluate_checkpoint(&cp, &all_votes, now);
    assert_eq!(new_status, Some(CheckpointStatus::Approved));
    cp.status = CheckpointStatus::Approved;

    // 5. Finalize artifact.
    let artifact = HitlArtifact::finalize(cp, all_votes, vec![], now);
    assert!(artifact.verify_integrity());
    let summary = DecisionSummary::from_artifact(&artifact);
    assert_eq!(summary.outcome, "approved");
    assert_eq!(summary.approval_count, 2);
}
