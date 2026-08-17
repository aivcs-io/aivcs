//! Approval checkpoints — the core unit of HITL gating.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::risk::RiskTier;

/// An approval checkpoint inserted into a run or pipeline.
///
/// When a checkpoint is reached during execution, the system pauses and
/// waits for the required approvals before proceeding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCheckpoint {
    /// Unique identifier for this checkpoint.
    pub checkpoint_id: String,
    /// Human-readable label describing what is being gated.
    pub label: String,
    /// The run this checkpoint belongs to.
    pub run_id: Uuid,
    /// Risk tier determining the approval requirements.
    pub risk_tier: RiskTier,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// Deadline after which the checkpoint expires (auto-reject).
    pub expires_at: Option<DateTime<Utc>>,
    /// Current status of the checkpoint.
    pub status: CheckpointStatus,
    /// Explanation of what this action does and why it needs approval.
    pub explanation: ExplainabilitySummary,
}

/// Status of an approval checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointStatus {
    /// Waiting for required approvals.
    Pending,
    /// All required approvals received — execution may continue.
    Approved,
    /// Rejected by a reviewer.
    Rejected { reason: String },
    /// The checkpoint expired without sufficient approvals.
    Expired,
    /// Execution was paused by an operator intervention.
    Paused,
}

impl CheckpointStatus {
    /// Whether the checkpoint allows execution to proceed.
    pub fn allows_proceed(&self) -> bool {
        matches!(self, Self::Approved)
    }

    /// Whether the checkpoint is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Approved | Self::Rejected { .. } | Self::Expired)
    }
}

/// Explainability summary attached to every checkpoint.
///
/// Provides context for what the gated action does and why it was flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainabilitySummary {
    /// Brief description of the action being gated.
    pub action_description: String,
    /// What changed compared to the previous state.
    pub changes_summary: String,
    /// Why this action was flagged for review.
    pub flag_reason: String,
}

impl ApprovalCheckpoint {
    /// Create a new pending checkpoint.
    pub fn new(
        label: impl Into<String>,
        run_id: Uuid,
        risk_tier: RiskTier,
        explanation: ExplainabilitySummary,
        timeout_secs: Option<u64>,
        now: DateTime<Utc>,
    ) -> Self {
        let expires_at = timeout_secs.map(|s| now + chrono::Duration::seconds(s as i64));
        Self {
            checkpoint_id: Uuid::new_v4().to_string(),
            label: label.into(),
            run_id,
            risk_tier,
            created_at: now,
            expires_at,
            status: CheckpointStatus::Pending,
            explanation,
        }
    }

    /// Check whether this checkpoint has expired at the given time.
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|exp| now >= exp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_explanation() -> ExplainabilitySummary {
        ExplainabilitySummary {
            action_description: "deploy to production".into(),
            changes_summary: "bumps API version from v2 to v3".into(),
            flag_reason: "production deploy is high-risk".into(),
        }
    }

    #[test]
    fn test_new_checkpoint_is_pending() {
        let cp = ApprovalCheckpoint::new(
            "deploy-prod",
            Uuid::new_v4(),
            RiskTier::High,
            sample_explanation(),
            Some(300),
            Utc::now(),
        );
        assert_eq!(cp.status, CheckpointStatus::Pending);
        assert!(cp.expires_at.is_some());
    }

    #[test]
    fn test_checkpoint_status_allows_proceed() {
        assert!(CheckpointStatus::Approved.allows_proceed());
        assert!(!CheckpointStatus::Pending.allows_proceed());
        assert!(!CheckpointStatus::Expired.allows_proceed());
        assert!(!CheckpointStatus::Rejected {
            reason: "no".into()
        }
        .allows_proceed());
    }

    #[test]
    fn test_checkpoint_status_is_terminal() {
        assert!(!CheckpointStatus::Pending.is_terminal());
        assert!(!CheckpointStatus::Paused.is_terminal());
        assert!(CheckpointStatus::Approved.is_terminal());
        assert!(CheckpointStatus::Expired.is_terminal());
        assert!(CheckpointStatus::Rejected { reason: "x".into() }.is_terminal());
    }

    #[test]
    fn test_is_expired_at() {
        let now = Utc::now();
        let cp = ApprovalCheckpoint::new(
            "test",
            Uuid::new_v4(),
            RiskTier::High,
            sample_explanation(),
            Some(60),
            now,
        );
        assert!(!cp.is_expired_at(now));
        assert!(cp.is_expired_at(now + chrono::Duration::seconds(61)));
    }

    #[test]
    fn test_no_expiry_never_expires() {
        let cp = ApprovalCheckpoint::new(
            "test",
            Uuid::new_v4(),
            RiskTier::Critical,
            sample_explanation(),
            None,
            Utc::now(),
        );
        assert!(!cp.is_expired_at(Utc::now() + chrono::Duration::days(365)));
    }

    #[test]
    fn test_serde_roundtrip() {
        let cp = ApprovalCheckpoint::new(
            "deploy-staging",
            Uuid::new_v4(),
            RiskTier::High,
            sample_explanation(),
            Some(120),
            Utc::now(),
        );
        let json = serde_json::to_string(&cp).unwrap();
        let back: ApprovalCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(cp, back);
    }
}
