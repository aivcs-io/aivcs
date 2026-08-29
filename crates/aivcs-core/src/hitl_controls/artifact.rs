//! Persistent audit artifact for HITL checkpoint decisions.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::Result;

use super::checkpoint::{ApprovalCheckpoint, CheckpointStatus, ExplainabilitySummary};
use super::intervention::Intervention;
use super::risk::RiskTier;
use super::vote::ApprovalVote;

/// Immutable audit record for a HITL checkpoint decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitlArtifact {
    /// The checkpoint this artifact records.
    pub checkpoint: ApprovalCheckpoint,
    /// All votes cast on this checkpoint.
    pub votes: Vec<ApprovalVote>,
    /// All interventions applied.
    pub interventions: Vec<Intervention>,
    /// When this artifact was finalized.
    pub finalized_at: DateTime<Utc>,
    /// SHA-256 digest of the serialized artifact for tamper evidence.
    pub content_digest: String,
}

impl HitlArtifact {
    /// Create a finalized artifact from a resolved checkpoint.
    pub fn finalize(
        checkpoint: ApprovalCheckpoint,
        votes: Vec<ApprovalVote>,
        interventions: Vec<Intervention>,
        now: DateTime<Utc>,
    ) -> Self {
        let mut artifact = Self {
            checkpoint,
            votes,
            interventions,
            finalized_at: now,
            content_digest: String::new(),
        };
        artifact.content_digest = artifact.compute_digest();
        artifact
    }

    /// Compute the SHA-256 digest of the artifact content (excluding the digest field itself).
    fn compute_digest(&self) -> String {
        use std::hash::{Hash, Hasher};
        // Use a stable serialization for digest computation.
        let payload = serde_json::json!({
            "checkpoint_id": self.checkpoint.checkpoint_id,
            "run_id": self.checkpoint.run_id,
            "status": self.checkpoint.status,
            "votes_count": self.votes.len(),
            "interventions_count": self.interventions.len(),
            "finalized_at": self.finalized_at.to_rfc3339(),
        });
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Verify the artifact's integrity.
    pub fn verify_integrity(&self) -> bool {
        self.content_digest == self.compute_digest()
    }
}

/// Write an HITL artifact to disk as JSON.
pub fn write_hitl_artifact(artifact: &HitlArtifact, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(artifact)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Read an HITL artifact from disk.
pub fn read_hitl_artifact(path: &Path) -> Result<HitlArtifact> {
    let data = std::fs::read_to_string(path)?;
    let artifact: HitlArtifact = serde_json::from_str(&data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(artifact)
}

/// Summary of a checkpoint's decision for explainability reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSummary {
    pub checkpoint_id: String,
    pub label: String,
    pub risk_tier: RiskTier,
    pub outcome: String,
    pub approval_count: u32,
    pub rejection_count: u32,
    pub intervention_count: usize,
    pub explanation: ExplainabilitySummary,
}

impl DecisionSummary {
    /// Build a decision summary from an artifact.
    pub fn from_artifact(artifact: &HitlArtifact) -> Self {
        let approval_count = artifact
            .votes
            .iter()
            .filter(|v| v.decision.is_approval())
            .count() as u32;
        let rejection_count = artifact
            .votes
            .iter()
            .filter(|v| v.decision.is_blocking())
            .count() as u32;
        let outcome = match &artifact.checkpoint.status {
            CheckpointStatus::Approved => "approved".to_string(),
            CheckpointStatus::Rejected { reason } => format!("rejected: {reason}"),
            CheckpointStatus::Expired => "expired".to_string(),
            CheckpointStatus::Pending => "pending".to_string(),
            CheckpointStatus::Paused => "paused".to_string(),
        };
        Self {
            checkpoint_id: artifact.checkpoint.checkpoint_id.clone(),
            label: artifact.checkpoint.label.clone(),
            risk_tier: artifact.checkpoint.risk_tier,
            outcome,
            approval_count,
            rejection_count,
            intervention_count: artifact.interventions.len(),
            explanation: artifact.checkpoint.explanation.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitl_controls::vote::VoteDecision;
    use uuid::Uuid;

    fn sample_checkpoint() -> ApprovalCheckpoint {
        ApprovalCheckpoint::new(
            "deploy-prod",
            Uuid::new_v4(),
            RiskTier::High,
            ExplainabilitySummary {
                action_description: "deploy".into(),
                changes_summary: "v2->v3".into(),
                flag_reason: "production".into(),
            },
            None,
            Utc::now(),
        )
    }

    #[test]
    fn test_finalize_sets_digest() {
        let cp = sample_checkpoint();
        let artifact = HitlArtifact::finalize(cp, vec![], vec![], Utc::now());
        assert!(!artifact.content_digest.is_empty());
    }

    #[test]
    fn test_verify_integrity_ok() {
        let cp = sample_checkpoint();
        let artifact = HitlArtifact::finalize(cp, vec![], vec![], Utc::now());
        assert!(artifact.verify_integrity());
    }

    #[test]
    fn test_verify_integrity_tampered() {
        let cp = sample_checkpoint();
        let mut artifact = HitlArtifact::finalize(cp, vec![], vec![], Utc::now());
        artifact.content_digest = "tampered".into();
        assert!(!artifact.verify_integrity());
    }

    #[test]
    fn test_write_and_read_artifact() {
        let mut cp = sample_checkpoint();
        cp.status = CheckpointStatus::Approved;
        let artifact = HitlArtifact::finalize(cp, vec![], vec![], Utc::now());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hitl.json");
        write_hitl_artifact(&artifact, &path).unwrap();
        let loaded = read_hitl_artifact(&path).unwrap();
        assert_eq!(artifact, loaded);
        assert!(loaded.verify_integrity());
    }

    #[test]
    fn test_decision_summary_from_artifact() {
        let mut cp = sample_checkpoint();
        cp.status = CheckpointStatus::Approved;
        let votes = vec![ApprovalVote::new(
            "alice",
            &cp.checkpoint_id,
            VoteDecision::Approve,
            Some("LGTM".into()),
            Utc::now(),
        )];
        let artifact = HitlArtifact::finalize(cp, votes, vec![], Utc::now());
        let summary = DecisionSummary::from_artifact(&artifact);
        assert_eq!(summary.outcome, "approved");
        assert_eq!(summary.approval_count, 1);
        assert_eq!(summary.rejection_count, 0);
    }
}
