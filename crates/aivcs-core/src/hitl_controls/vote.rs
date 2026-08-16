//! Approval votes for HITL checkpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single approval or rejection vote on a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalVote {
    /// Who cast this vote (operator identifier).
    pub voter: String,
    /// The checkpoint this vote applies to.
    pub checkpoint_id: String,
    /// The decision.
    pub decision: VoteDecision,
    /// When the vote was cast.
    pub voted_at: DateTime<Utc>,
    /// Optional comment from the reviewer.
    pub comment: Option<String>,
}

/// The decision of a single vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteDecision {
    /// Approve the gated action.
    Approve,
    /// Reject the gated action with explanation.
    Reject,
    /// Request changes before re-evaluation.
    RequestChanges,
}

impl VoteDecision {
    /// Whether this vote counts as an approval.
    pub fn is_approval(self) -> bool {
        matches!(self, Self::Approve)
    }

    /// Whether this vote blocks the checkpoint.
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::Reject)
    }
}

impl ApprovalVote {
    /// Create a new vote.
    pub fn new(
        voter: impl Into<String>,
        checkpoint_id: impl Into<String>,
        decision: VoteDecision,
        comment: Option<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            voter: voter.into(),
            checkpoint_id: checkpoint_id.into(),
            decision,
            voted_at: now,
            comment,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vote_decision_is_approval() {
        assert!(VoteDecision::Approve.is_approval());
        assert!(!VoteDecision::Reject.is_approval());
        assert!(!VoteDecision::RequestChanges.is_approval());
    }

    #[test]
    fn test_vote_decision_is_blocking() {
        assert!(!VoteDecision::Approve.is_blocking());
        assert!(VoteDecision::Reject.is_blocking());
        assert!(!VoteDecision::RequestChanges.is_blocking());
    }

    #[test]
    fn test_serde_roundtrip() {
        let vote = ApprovalVote::new(
            "alice",
            "cp-123",
            VoteDecision::Approve,
            Some("LGTM".into()),
            Utc::now(),
        );
        let json = serde_json::to_string(&vote).unwrap();
        let back: ApprovalVote = serde_json::from_str(&json).unwrap();
        assert_eq!(vote, back);
    }
}
