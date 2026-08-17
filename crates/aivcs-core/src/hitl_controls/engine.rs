//! HITL evaluation engine — resolves checkpoints by collecting votes and applying policy.

use chrono::{DateTime, Utc};

use super::checkpoint::{ApprovalCheckpoint, CheckpointStatus};
use super::error::{HitlError, HitlResult};
use super::intervention::{Intervention, InterventionAction};
use super::vote::ApprovalVote;

/// Apply a vote to a checkpoint.
///
/// # Errors
///
/// Returns `HitlError::CheckpointNotFound` if IDs don't match.
/// Returns `HitlError::DuplicateVote` if the same voter has already voted.
/// Returns `HitlError::Expired` if the checkpoint has expired.
pub fn submit_vote(
    checkpoint: &mut ApprovalCheckpoint,
    vote: &ApprovalVote,
    existing_votes: &[ApprovalVote],
    now: DateTime<Utc>,
) -> HitlResult<()> {
    if vote.checkpoint_id != checkpoint.checkpoint_id {
        return Err(HitlError::CheckpointNotFound(vote.checkpoint_id.clone()));
    }

    if checkpoint.status.is_terminal() {
        return Err(HitlError::CheckpointNotFound(format!(
            "{} (already {})",
            checkpoint.checkpoint_id,
            if matches!(checkpoint.status, CheckpointStatus::Expired) {
                "expired"
            } else {
                "finalized"
            }
        )));
    }

    if checkpoint.is_expired_at(now) {
        checkpoint.status = CheckpointStatus::Expired;
        return Err(HitlError::Expired {
            timeout_secs: checkpoint
                .expires_at
                .map(|e| (e - checkpoint.created_at).num_seconds() as u64)
                .unwrap_or(0),
        });
    }

    if existing_votes.iter().any(|v| v.voter == vote.voter) {
        return Err(HitlError::DuplicateVote {
            voter: vote.voter.clone(),
            checkpoint_id: checkpoint.checkpoint_id.clone(),
        });
    }

    Ok(())
}

/// Evaluate whether a checkpoint should transition based on accumulated votes.
///
/// Returns the new status if a transition should happen, `None` otherwise.
pub fn evaluate_checkpoint(
    checkpoint: &ApprovalCheckpoint,
    votes: &[ApprovalVote],
    now: DateTime<Utc>,
) -> Option<CheckpointStatus> {
    if checkpoint.status.is_terminal() {
        return None;
    }

    // Check expiry first.
    if checkpoint.is_expired_at(now) {
        return Some(CheckpointStatus::Expired);
    }

    // Any rejection is an immediate block.
    for v in votes {
        if v.decision.is_blocking() {
            let reason = v
                .comment
                .clone()
                .unwrap_or_else(|| format!("rejected by {}", v.voter));
            return Some(CheckpointStatus::Rejected { reason });
        }
    }

    // Count approvals against the tier requirement.
    let approval_count = votes.iter().filter(|v| v.decision.is_approval()).count() as u32;
    let required = checkpoint.risk_tier.min_approvals();

    if approval_count >= required && required > 0 {
        return Some(CheckpointStatus::Approved);
    }

    // Low/medium risk with no explicit approval requirement — auto-approve.
    if !checkpoint.risk_tier.requires_approval() {
        return Some(CheckpointStatus::Approved);
    }

    None
}

/// Apply an intervention to a checkpoint.
///
/// Returns the updated checkpoint status after the intervention.
pub fn apply_intervention(
    checkpoint: &mut ApprovalCheckpoint,
    intervention: &Intervention,
) -> HitlResult<()> {
    match &intervention.action {
        InterventionAction::Pause => {
            checkpoint.status = CheckpointStatus::Paused;
        }
        InterventionAction::Continue => {
            if !matches!(checkpoint.status, CheckpointStatus::Paused) {
                return Err(HitlError::InterventionFailed(
                    "can only continue from paused state".into(),
                ));
            }
            checkpoint.status = CheckpointStatus::Pending;
        }
        InterventionAction::Abort { reason } => {
            checkpoint.status = CheckpointStatus::Rejected {
                reason: format!("aborted: {reason}"),
            };
        }
        InterventionAction::Edit { .. } => {
            // Edit keeps checkpoint paused — operator must explicitly continue.
            checkpoint.status = CheckpointStatus::Paused;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitl_controls::checkpoint::ExplainabilitySummary;
    use crate::hitl_controls::risk::RiskTier;
    use crate::hitl_controls::vote::VoteDecision;
    use uuid::Uuid;

    fn sample_explanation() -> ExplainabilitySummary {
        ExplainabilitySummary {
            action_description: "test action".into(),
            changes_summary: "test changes".into(),
            flag_reason: "test".into(),
        }
    }

    fn make_checkpoint(tier: RiskTier, timeout: Option<u64>) -> ApprovalCheckpoint {
        ApprovalCheckpoint::new(
            "test-checkpoint",
            Uuid::new_v4(),
            tier,
            sample_explanation(),
            timeout,
            Utc::now(),
        )
    }

    fn make_vote(voter: &str, cp_id: &str, decision: VoteDecision) -> ApprovalVote {
        ApprovalVote::new(voter, cp_id, decision, None, Utc::now())
    }

    #[test]
    fn test_submit_vote_ok() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let vote = make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve);
        assert!(submit_vote(&mut cp, &vote, &[], Utc::now()).is_ok());
    }

    #[test]
    fn test_submit_vote_wrong_checkpoint() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let vote = make_vote("alice", "wrong-id", VoteDecision::Approve);
        let err = submit_vote(&mut cp, &vote, &[], Utc::now()).unwrap_err();
        assert!(matches!(err, HitlError::CheckpointNotFound(_)));
    }

    #[test]
    fn test_submit_vote_duplicate() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let vote = make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve);
        let err = submit_vote(&mut cp, &vote, std::slice::from_ref(&vote), Utc::now()).unwrap_err();
        assert!(matches!(err, HitlError::DuplicateVote { .. }));
    }

    #[test]
    fn test_evaluate_low_risk_auto_approves() {
        let cp = make_checkpoint(RiskTier::Low, None);
        let status = evaluate_checkpoint(&cp, &[], Utc::now());
        assert_eq!(status, Some(CheckpointStatus::Approved));
    }

    #[test]
    fn test_evaluate_high_risk_needs_approval() {
        let cp = make_checkpoint(RiskTier::High, None);
        let status = evaluate_checkpoint(&cp, &[], Utc::now());
        assert_eq!(status, None); // Not enough approvals yet.
    }

    #[test]
    fn test_evaluate_high_risk_one_approval() {
        let cp = make_checkpoint(RiskTier::High, None);
        let votes = vec![make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve)];
        let status = evaluate_checkpoint(&cp, &votes, Utc::now());
        assert_eq!(status, Some(CheckpointStatus::Approved));
    }

    #[test]
    fn test_evaluate_critical_needs_two() {
        let cp = make_checkpoint(RiskTier::Critical, None);

        // One approval is not enough.
        let votes = vec![make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve)];
        assert_eq!(evaluate_checkpoint(&cp, &votes, Utc::now()), None);

        // Two approvals suffice.
        let votes = vec![
            make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve),
            make_vote("bob", &cp.checkpoint_id, VoteDecision::Approve),
        ];
        assert_eq!(
            evaluate_checkpoint(&cp, &votes, Utc::now()),
            Some(CheckpointStatus::Approved)
        );
    }

    #[test]
    fn test_evaluate_rejection_overrides() {
        let cp = make_checkpoint(RiskTier::High, None);
        let votes = vec![
            make_vote("alice", &cp.checkpoint_id, VoteDecision::Approve),
            make_vote("bob", &cp.checkpoint_id, VoteDecision::Reject),
        ];
        let status = evaluate_checkpoint(&cp, &votes, Utc::now());
        assert!(matches!(status, Some(CheckpointStatus::Rejected { .. })));
    }

    #[test]
    fn test_evaluate_expired() {
        let now = Utc::now();
        let cp = ApprovalCheckpoint::new(
            "expire-test",
            Uuid::new_v4(),
            RiskTier::High,
            sample_explanation(),
            Some(1),
            now,
        );
        let future = now + chrono::Duration::seconds(2);
        let status = evaluate_checkpoint(&cp, &[], future);
        assert_eq!(status, Some(CheckpointStatus::Expired));
    }

    #[test]
    fn test_apply_intervention_pause() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let iv = Intervention::new(
            cp.run_id,
            Some(cp.checkpoint_id.clone()),
            "ops",
            InterventionAction::Pause,
            None,
            Utc::now(),
        );
        apply_intervention(&mut cp, &iv).unwrap();
        assert_eq!(cp.status, CheckpointStatus::Paused);
    }

    #[test]
    fn test_apply_intervention_continue_from_paused() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        cp.status = CheckpointStatus::Paused;
        let iv = Intervention::new(
            cp.run_id,
            Some(cp.checkpoint_id.clone()),
            "ops",
            InterventionAction::Continue,
            None,
            Utc::now(),
        );
        apply_intervention(&mut cp, &iv).unwrap();
        assert_eq!(cp.status, CheckpointStatus::Pending);
    }

    #[test]
    fn test_apply_intervention_continue_from_pending_fails() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let iv = Intervention::new(
            cp.run_id,
            Some(cp.checkpoint_id.clone()),
            "ops",
            InterventionAction::Continue,
            None,
            Utc::now(),
        );
        let err = apply_intervention(&mut cp, &iv).unwrap_err();
        assert!(matches!(err, HitlError::InterventionFailed(_)));
    }

    #[test]
    fn test_apply_intervention_abort() {
        let mut cp = make_checkpoint(RiskTier::High, None);
        let iv = Intervention::new(
            cp.run_id,
            Some(cp.checkpoint_id.clone()),
            "ops",
            InterventionAction::Abort {
                reason: "wrong deploy".into(),
            },
            None,
            Utc::now(),
        );
        apply_intervention(&mut cp, &iv).unwrap();
        assert!(matches!(cp.status, CheckpointStatus::Rejected { .. }));
    }
}
