//! Multi-repo orchestrator: dependency-order execution and rollout gating.
//!
//! EPIC9: Cross-repo changes execute in dependency order; downstream breakage blocks rollout.

use crate::domain::error::{AivcsError, Result};
use crate::multi_repo::health::{CIHealthView, RepoCIStatus};
use crate::multi_repo::model::{CrossRepoGraph, RepoId};

/// Execution plan for a multi-repo rollout: repos in dependency order.
#[derive(Debug, Clone)]
pub struct MultiRepoExecutionPlan {
    /// Repos in order: run first (dependencies) to last (dependents).
    pub order: Vec<RepoId>,
}

/// Multi-repo orchestrator: builds execution plans and checks rollout gates.
pub struct MultiRepoOrchestrator;

impl MultiRepoOrchestrator {
    /// Build an execution plan so that cross-repo changes run in dependency order.
    pub fn execution_plan(graph: &CrossRepoGraph) -> Result<MultiRepoExecutionPlan> {
        let order = graph.execution_order().map_err(AivcsError::MultiRepo)?;
        Ok(MultiRepoExecutionPlan { order })
    }

    /// Returns true if rollout is allowed: all repos in the health view passed.
    /// Downstream breakage (any failed) blocks rollout.
    pub fn rollout_allowed(health: &CIHealthView) -> bool {
        health.can_rollout()
    }

    /// Returns an error if rollout is blocked (any failed or still running).
    pub fn check_rollout_gate(health: &CIHealthView) -> Result<()> {
        if health.rollout_blocked() {
            let status = health.overall_status();
            return Err(AivcsError::MultiRepo(format!(
                "rollout blocked: overall CI status is {:?}",
                status
            )));
        }
        Ok(())
    }

    /// Consolidate per-repo CI results into a single health view for an objective.
    pub fn consolidate_health(objective_id: &str, repos: Vec<RepoCIStatus>) -> CIHealthView {
        CIHealthView::new(objective_id, repos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ci::CIStatus;
    use crate::multi_repo::model::RepoDependency;

    fn repo_status(name: &str, status: CIStatus) -> RepoCIStatus {
        RepoCIStatus {
            repo: RepoId::new(name),
            status,
            run_id: Some(uuid::Uuid::new_v4()),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_execution_plan_order() {
        let a = RepoId::new("a");
        let b = RepoId::new("b");
        let c = RepoId::new("c");
        let graph = CrossRepoGraph::new(
            vec![a.clone(), b.clone(), c.clone()],
            vec![
                RepoDependency {
                    dependent: c.clone(),
                    dependency: b.clone(),
                },
                RepoDependency {
                    dependent: b.clone(),
                    dependency: a.clone(),
                },
            ],
        );
        let plan = MultiRepoOrchestrator::execution_plan(&graph).expect("plan");
        assert_eq!(plan.order.len(), 3);
        assert_eq!(plan.order[0].name, "a");
        assert_eq!(plan.order[1].name, "b");
        assert_eq!(plan.order[2].name, "c");
    }

    #[test]
    fn test_execution_plan_cycle_errors() {
        let a = RepoId::new("a");
        let b = RepoId::new("b");
        let graph = CrossRepoGraph::new(
            vec![a.clone(), b.clone()],
            vec![
                RepoDependency {
                    dependent: b.clone(),
                    dependency: a.clone(),
                },
                RepoDependency {
                    dependent: a.clone(),
                    dependency: b.clone(),
                },
            ],
        );
        let res = MultiRepoOrchestrator::execution_plan(&graph);
        assert!(matches!(res, Err(AivcsError::MultiRepo(_))));
    }

    #[test]
    fn test_rollout_allowed_all_pass() {
        let health = MultiRepoOrchestrator::consolidate_health(
            "obj-1",
            vec![
                repo_status("org/a", CIStatus::Passed),
                repo_status("org/b", CIStatus::Passed),
            ],
        );
        assert!(MultiRepoOrchestrator::rollout_allowed(&health));
        assert!(MultiRepoOrchestrator::check_rollout_gate(&health).is_ok());
    }

    #[test]
    fn test_rollout_blocked_on_fail() {
        let health = MultiRepoOrchestrator::consolidate_health(
            "obj-1",
            vec![
                repo_status("org/a", CIStatus::Passed),
                repo_status("org/b", CIStatus::Failed),
            ],
        );
        assert!(!MultiRepoOrchestrator::rollout_allowed(&health));
        assert!(MultiRepoOrchestrator::check_rollout_gate(&health).is_err());
    }
}
