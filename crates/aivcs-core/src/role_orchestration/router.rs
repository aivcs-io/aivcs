//! Task decomposition and role routing.
//!
//! Validates that a proposed handoff sequence is permitted by the role templates
//! and builds an [`ExecutionPlan`] with parallelizability annotations.

use crate::role_orchestration::{
    error::{RoleError, RoleResult},
    roles::{AgentRole, RoleTemplate},
};

/// A single step in an orchestration execution plan.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleStep {
    /// 0-indexed position in the plan.
    pub position: usize,
    /// Role assigned to this step.
    pub role: AgentRole,
    /// Roles from which this step may receive a handoff token.
    pub accepts_from: Vec<AgentRole>,
    /// Whether this step may execute concurrently with adjacent parallelizable steps.
    pub parallelizable: bool,
}

/// An ordered, validated orchestration plan for a task.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// Human-readable task title.
    pub task_title: String,
    /// Validated, ordered sequence of role steps.
    pub steps: Vec<RoleStep>,
}

impl ExecutionPlan {
    /// Partition the steps into sequential groups.
    ///
    /// Adjacent steps with `parallelizable = true` are grouped together.
    /// Non-parallelizable steps form singleton groups.
    pub fn parallel_groups(&self) -> Vec<Vec<&RoleStep>> {
        let mut groups: Vec<Vec<&RoleStep>> = Vec::new();
        let mut current: Vec<&RoleStep> = Vec::new();

        for step in &self.steps {
            if step.parallelizable {
                current.push(step);
            } else {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
                groups.push(vec![step]);
            }
        }
        if !current.is_empty() {
            groups.push(current);
        }
        groups
    }
}

/// Validate that each consecutive role pair in `proposed_sequence` is permitted
/// by `templates`.
///
/// For each window `(from, to)`, checks that `templates` contains a template for
/// `to` and that `to.accepts_from` contains `from`.
///
/// Returns [`RoleError::UnauthorizedHandoff`] on the first violation.
pub fn validate_handoff_sequence(
    proposed_sequence: &[AgentRole],
    templates: &[RoleTemplate],
) -> RoleResult<()> {
    let is_parallel = |r: &AgentRole| matches!(r, AgentRole::Reviewer | AgentRole::Tester);

    for window in proposed_sequence.windows(2) {
        let from = &window[0];
        let to = &window[1];

        // Reviewer/Tester are sibling-parallel in the standard flow and do not
        // hand off to each other directly.
        if is_parallel(from) && is_parallel(to) {
            continue;
        }

        let template = templates.iter().find(|t| &t.role == to).ok_or_else(|| {
            RoleError::UnauthorizedHandoff {
                role: to.to_string(),
                from: from.to_string(),
            }
        })?;

        if !template.accepts_from.contains(from) {
            return Err(RoleError::UnauthorizedHandoff {
                role: to.to_string(),
                from: from.to_string(),
            });
        }
    }
    Ok(())
}

/// Build an [`ExecutionPlan`] from a task title and desired role sequence.
///
/// Validates each consecutive pair in `sequence` against `templates`, **except**
/// when both roles in a pair are `parallelizable` (i.e. `Reviewer` / `Tester`).
/// Parallel siblings both receive their handoff from the step that immediately
/// precedes the parallel group, not from each other.
///
/// Returns [`RoleError::EmptyDecomposition`] when `sequence` is empty.
pub fn build_execution_plan(
    task_title: &str,
    sequence: Vec<AgentRole>,
    templates: &[RoleTemplate],
) -> RoleResult<ExecutionPlan> {
    if sequence.is_empty() {
        return Err(RoleError::EmptyDecomposition {
            role: "none".to_string(),
        });
    }

    // Validate consecutive pairs, skipping sibling-parallel handoffs.
    let is_parallel = |r: &AgentRole| matches!(r, AgentRole::Reviewer | AgentRole::Tester);
    for window in sequence.windows(2) {
        let from = &window[0];
        let to = &window[1];
        // Both are parallel siblings — they don't hand off to each other.
        if is_parallel(from) && is_parallel(to) {
            continue;
        }
        let template = templates.iter().find(|t| &t.role == to).ok_or_else(|| {
            RoleError::UnauthorizedHandoff {
                role: to.to_string(),
                from: from.to_string(),
            }
        })?;
        if !template.accepts_from.contains(from) {
            return Err(RoleError::UnauthorizedHandoff {
                role: to.to_string(),
                from: from.to_string(),
            });
        }
    }

    let steps = sequence
        .into_iter()
        .enumerate()
        .map(|(pos, role)| {
            let accepts_from = templates
                .iter()
                .find(|t| t.role == role)
                .map(|t| t.accepts_from.clone())
                .unwrap_or_default();
            let parallelizable = is_parallel(&role);
            RoleStep {
                position: pos,
                role,
                accepts_from,
                parallelizable,
            }
        })
        .collect();

    Ok(ExecutionPlan {
        task_title: task_title.to_string(),
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_orchestration::roles::RoleTemplate;

    fn templates() -> Vec<RoleTemplate> {
        RoleTemplate::standard_pipeline()
    }

    #[test]
    fn test_standard_sequence_passes_validation() {
        // Standard route includes Reviewer + Tester sibling-parallel pair.
        let seq = vec![
            AgentRole::Planner,
            AgentRole::Coder,
            AgentRole::Reviewer,
            AgentRole::Tester,
            AgentRole::Fixer,
        ];
        assert!(validate_handoff_sequence(&seq, &templates()).is_ok());
    }

    #[test]
    fn test_unauthorized_handoff_coder_to_planner_is_rejected() {
        let seq = vec![AgentRole::Coder, AgentRole::Planner];
        let result = validate_handoff_sequence(&seq, &templates());
        assert!(result.is_err());
        match result.unwrap_err() {
            RoleError::UnauthorizedHandoff { role, from } => {
                assert_eq!(role, "planner");
                assert_eq!(from, "coder");
            }
            other => panic!("Expected UnauthorizedHandoff, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_sequence_returns_error() {
        let result = build_execution_plan("task", vec![], &templates());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RoleError::EmptyDecomposition { .. }
        ));
    }

    #[test]
    fn test_execution_plan_parallel_groups_groups_reviewer_and_tester() {
        // Coder → Reviewer + Tester: Reviewer and Tester are parallel siblings.
        // The plan skips sibling-to-sibling validation.
        let plan = build_execution_plan(
            "test task",
            vec![AgentRole::Coder, AgentRole::Reviewer, AgentRole::Tester],
            &templates(),
        )
        .unwrap();

        let groups = plan.parallel_groups();
        // Coder is singleton; Reviewer + Tester form one parallel group
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].role, AgentRole::Coder);
        assert_eq!(groups[1].len(), 2);
    }

    #[test]
    fn test_build_plan_preserves_role_order() {
        // Planner → Coder → Reviewer + Tester (parallel siblings)
        let seq = vec![
            AgentRole::Planner,
            AgentRole::Coder,
            AgentRole::Reviewer,
            AgentRole::Tester,
        ];
        let plan = build_execution_plan("ordered task", seq.clone(), &templates()).unwrap();

        assert_eq!(plan.steps.len(), 4);
        for (i, step) in plan.steps.iter().enumerate() {
            assert_eq!(step.position, i);
            assert_eq!(step.role, seq[i]);
        }
    }

    #[test]
    fn test_single_role_plan_is_valid() {
        // A single-role plan has no windows to validate, so it's always valid
        let plan = build_execution_plan("solo", vec![AgentRole::Planner], &templates()).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(!plan.steps[0].parallelizable);
    }

    #[test]
    fn test_validate_handoff_sequence_skips_parallel_sibling_edge() {
        // Coder -> Reviewer and Tester -> Fixer are validated; Reviewer -> Tester
        // sibling edge is intentionally skipped.
        let seq = vec![
            AgentRole::Coder,
            AgentRole::Reviewer,
            AgentRole::Tester,
            AgentRole::Fixer,
        ];
        assert!(validate_handoff_sequence(&seq, &templates()).is_ok());
    }
}
