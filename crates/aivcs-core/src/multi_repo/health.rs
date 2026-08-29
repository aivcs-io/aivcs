//! Unified CI/release health view across repos.
//!
//! EPIC9: CI signals consolidate per objective.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::ci::CIStatus;
use crate::multi_repo::model::RepoId;

/// Per-repo CI status for a single objective (e.g. a release plan).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoCIStatus {
    pub repo: RepoId,
    pub status: CIStatus,
    pub run_id: Option<uuid::Uuid>,
    pub updated_at: DateTime<Utc>,
}

/// Consolidated CI health for a multi-repo objective.
/// Downstream breakage blocks rollout when any repo has failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CIHealthView {
    /// Objective identifier (e.g. plan id or release track name).
    pub objective_id: String,
    /// Per-repo statuses.
    pub repos: Vec<RepoCIStatus>,
    /// When this view was computed.
    pub computed_at: DateTime<Utc>,
}

impl CIHealthView {
    pub fn new(objective_id: impl Into<String>, repos: Vec<RepoCIStatus>) -> Self {
        Self {
            objective_id: objective_id.into(),
            computed_at: Utc::now(),
            repos,
        }
    }

    /// Overall status: Passed only if all repos passed.
    pub fn overall_status(&self) -> CIStatus {
        if self.repos.is_empty() {
            return CIStatus::Pending;
        }
        let any_failed = self.repos.iter().any(|r| r.status == CIStatus::Failed);
        let any_running = self.repos.iter().any(|r| r.status == CIStatus::Running);
        let any_pending = self.repos.iter().any(|r| r.status == CIStatus::Pending);
        if any_failed {
            CIStatus::Failed
        } else if any_running || any_pending {
            CIStatus::Running
        } else {
            CIStatus::Passed
        }
    }

    /// True if rollout is blocked (any repo failed or still running/pending).
    pub fn rollout_blocked(&self) -> bool {
        self.overall_status() != CIStatus::Passed
    }

    /// True if all repos passed; rollout can proceed.
    pub fn can_rollout(&self) -> bool {
        self.overall_status() == CIStatus::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_status(repo: &str, status: CIStatus) -> RepoCIStatus {
        RepoCIStatus {
            repo: RepoId::new(repo),
            status,
            run_id: Some(uuid::Uuid::new_v4()),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_health_all_pass() {
        let view = CIHealthView::new(
            "release-1",
            vec![
                repo_status("org/a", CIStatus::Passed),
                repo_status("org/b", CIStatus::Passed),
            ],
        );
        assert_eq!(view.overall_status(), CIStatus::Passed);
        assert!(!view.rollout_blocked());
        assert!(view.can_rollout());
    }

    #[test]
    fn test_health_one_fail_blocks() {
        let view = CIHealthView::new(
            "release-1",
            vec![
                repo_status("org/a", CIStatus::Passed),
                repo_status("org/b", CIStatus::Failed),
            ],
        );
        assert_eq!(view.overall_status(), CIStatus::Failed);
        assert!(view.rollout_blocked());
        assert!(!view.can_rollout());
    }

    #[test]
    fn test_health_running_blocked() {
        let view = CIHealthView::new(
            "release-1",
            vec![
                repo_status("org/a", CIStatus::Passed),
                repo_status("org/b", CIStatus::Running),
            ],
        );
        assert_eq!(view.overall_status(), CIStatus::Running);
        assert!(view.rollout_blocked());
        assert!(!view.can_rollout());
    }

    #[test]
    fn test_health_empty_pending() {
        let view = CIHealthView::new("release-1", vec![]);
        assert_eq!(view.overall_status(), CIStatus::Pending);
        assert!(view.rollout_blocked());
    }
}
