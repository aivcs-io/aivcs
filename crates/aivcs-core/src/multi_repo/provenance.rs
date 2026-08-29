//! Release provenance: link release artifacts to plan/run.
//!
//! EPIC9: Release artifacts map to plan/run provenance.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::multi_repo::model::RepoId;

/// Links a release artifact to the run and repo that produced it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseProvenance {
    /// Repository that produced this release.
    pub repo: RepoId,
    /// CI/run identifier (e.g. run_id or pipeline run id).
    pub run_id: uuid::Uuid,
    /// Git SHA at release time.
    pub git_sha: String,
    /// Agent/spec digest at release time.
    pub spec_digest: String,
    /// Plan or objective id this release belongs to (optional).
    pub plan_id: Option<String>,
    /// When the release was recorded.
    pub recorded_at: DateTime<Utc>,
}

impl ReleaseProvenance {
    pub fn new(
        repo: RepoId,
        run_id: uuid::Uuid,
        git_sha: String,
        spec_digest: String,
        plan_id: Option<String>,
    ) -> Self {
        Self {
            repo,
            run_id,
            git_sha,
            spec_digest,
            plan_id,
            recorded_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provenance_roundtrip() {
        let p = ReleaseProvenance::new(
            RepoId::new("stevedores-org/aivcs"),
            uuid::Uuid::new_v4(),
            "abc123".to_string(),
            "digest-spec".to_string(),
            Some("plan-1".to_string()),
        );
        let json = serde_json::to_string(&p).expect("serialize");
        let back: ReleaseProvenance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p.repo, back.repo);
        assert_eq!(p.run_id, back.run_id);
        assert_eq!(p.git_sha, back.git_sha);
        assert_eq!(p.spec_digest, back.spec_digest);
        assert_eq!(p.plan_id, back.plan_id);
    }
}
