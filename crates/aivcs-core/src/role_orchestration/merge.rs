//! Role output merge and conflict strategy.
//!
//! When `Reviewer` and `Tester` execute in parallel their outputs must be
//! reconciled before `Fixer` is invoked. This module defines the merge
//! strategy and conflict surface type.

use serde::{Deserialize, Serialize};

use crate::role_orchestration::{
    error::{RoleError, RoleResult},
    roles::{AgentRole, HandoffToken, RoleOutput},
};

/// A conflict detected when merging two role outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleConflict {
    /// Which aspect of the outputs conflicts.
    pub aspect: String,
    pub from_role_a: AgentRole,
    pub value_a: serde_json::Value,
    pub from_role_b: AgentRole,
    pub value_b: serde_json::Value,
    /// Human-readable path to resolution, surfaced to the caller.
    pub remediation: String,
}

/// Result of merging two parallel role outputs.
#[derive(Debug, Clone)]
pub struct MergedRoleOutput {
    /// A clean merged output when all conflicts were resolved.
    pub resolved: Option<RoleOutput>,
    /// Conflicts that require human or LLM arbitration.
    pub conflicts: Vec<RoleConflict>,
    /// Number of conflicts that were auto-resolved.
    pub auto_resolved_count: usize,
}

impl MergedRoleOutput {
    /// `true` only when no unresolved conflicts remain.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Merge two `HandoffToken`s produced by parallel role runs.
///
/// Only the `Reviewer` + `Tester` pair is supported; all other combinations
/// return [`RoleError::ConflictDetected`].
///
/// **Conflict rules:**
/// - Reviewer `approved: true` but Tester `passed: false` → unresolvable conflict.
///
/// **Auto-resolution rules:**
/// - Reviewer `requires_fix: true` but Tester `passed: true` → auto-resolved in favour
///   of Reviewer (more conservative).
/// - Both agree → clean merge.
///
/// Both tokens are integrity-verified before any merging takes place.
pub fn merge_parallel_outputs(
    token_a: &HandoffToken,
    token_b: &HandoffToken,
) -> RoleResult<MergedRoleOutput> {
    token_a
        .verify()
        .map_err(|e| RoleError::InvalidHandoffToken {
            reason: e.to_string(),
        })?;
    token_b
        .verify()
        .map_err(|e| RoleError::InvalidHandoffToken {
            reason: e.to_string(),
        })?;

    match (&token_a.output, &token_b.output) {
        (
            RoleOutput::Review {
                approved,
                requires_fix,
                comments,
            },
            RoleOutput::TestReport {
                passed,
                failed_cases,
                ..
            },
        ) => merge_review_and_test(*approved, *requires_fix, comments, *passed, failed_cases),

        // Symmetric: swap A and B if roles are reversed.
        (
            RoleOutput::TestReport {
                passed,
                failed_cases,
                ..
            },
            RoleOutput::Review {
                approved,
                requires_fix,
                comments,
            },
        ) => merge_review_and_test(*approved, *requires_fix, comments, *passed, failed_cases),

        _ => Err(RoleError::ConflictDetected {
            description: format!(
                "cannot merge outputs from {} and {} — only Reviewer+Tester pair is supported",
                token_a.from_role, token_b.from_role
            ),
        }),
    }
}

fn merge_review_and_test(
    approved: bool,
    requires_fix: bool,
    comments: &[String],
    passed: bool,
    failed_cases: &[String],
) -> RoleResult<MergedRoleOutput> {
    let mut conflicts = Vec::new();
    let mut auto_resolved_count = 0;

    // Rule 1: tests fail and reviewer approved -> unresolvable conflict.
    if approved && !passed {
        conflicts.push(RoleConflict {
            aspect: "approval_vs_test_result".to_string(),
            from_role_a: AgentRole::Reviewer,
            value_a: serde_json::json!({ "approved": true }),
            from_role_b: AgentRole::Tester,
            value_b: serde_json::json!({ "passed": false, "failed_cases": failed_cases }),
            remediation: "Reviewer approved code that does not pass all tests. \
                          Invoke Fixer with the diagnostic_digest before re-running Tester."
                .to_string(),
        });
    }

    // Rule 2: both reviewer and tests indicate non-ready outcome -> conflict.
    if !approved && !passed {
        conflicts.push(RoleConflict {
            aspect: "review_rejected_and_tests_failed".to_string(),
            from_role_a: AgentRole::Reviewer,
            value_a: serde_json::json!({ "approved": false, "requires_fix": requires_fix }),
            from_role_b: AgentRole::Tester,
            value_b: serde_json::json!({ "passed": false, "failed_cases": failed_cases }),
            remediation:
                "Both review and tests rejected the change. Invoke Fixer before rerunning Reviewer/Tester."
                    .to_string(),
        });
    }

    // Rule 2: reviewer requires fix but tests pass → auto-resolve (trust Reviewer).
    if requires_fix && passed && conflicts.is_empty() {
        auto_resolved_count += 1;
    }

    // Clean merge: no unresolved conflicts.
    if conflicts.is_empty() {
        let (resolved_approved, resolved_requires_fix) = if requires_fix && passed {
            (false, true)
        } else if !approved && passed {
            // Tests can override an outright review rejection when no "requires_fix" is set.
            auto_resolved_count += 1;
            (true, false)
        } else {
            (approved, requires_fix)
        };

        let resolved = Some(RoleOutput::Review {
            approved: resolved_approved,
            requires_fix: resolved_requires_fix,
            comments: comments.to_vec(),
        });
        return Ok(MergedRoleOutput {
            resolved,
            conflicts,
            auto_resolved_count,
        });
    }

    Ok(MergedRoleOutput {
        resolved: None,
        conflicts,
        auto_resolved_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_orchestration::roles::HandoffToken;

    fn review_token(approved: bool, requires_fix: bool) -> HandoffToken {
        HandoffToken::new(RoleOutput::Review {
            approved,
            comments: vec!["comment".to_string()],
            requires_fix,
        })
        .unwrap()
    }

    fn test_token(passed: bool, failed: Vec<&str>) -> HandoffToken {
        HandoffToken::new(RoleOutput::TestReport {
            passed,
            total_cases: 5,
            failed_cases: failed.into_iter().map(String::from).collect(),
            diagnostic_digest: None,
        })
        .unwrap()
    }

    #[test]
    fn test_merge_reviewer_approved_and_tests_passed_is_clean() {
        let result =
            merge_parallel_outputs(&review_token(true, false), &test_token(true, vec![])).unwrap();
        assert!(result.is_clean());
        assert!(result.resolved.is_some());
        assert_eq!(result.auto_resolved_count, 0);
    }

    #[test]
    fn test_merge_reviewer_approved_but_tests_failed_surfaces_conflict() {
        let result = merge_parallel_outputs(
            &review_token(true, false),
            &test_token(false, vec!["test_x"]),
        )
        .unwrap();
        assert!(!result.is_clean());
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].aspect, "approval_vs_test_result");
        assert!(result.resolved.is_none());
    }

    #[test]
    fn test_merge_reviewer_requires_fix_but_tests_passed_auto_resolves() {
        let result =
            merge_parallel_outputs(&review_token(false, true), &test_token(true, vec![])).unwrap();
        assert!(result.is_clean());
        assert_eq!(result.auto_resolved_count, 1);
        match result.resolved {
            Some(RoleOutput::Review {
                approved,
                requires_fix,
                ..
            }) => {
                assert!(!approved);
                assert!(requires_fix);
            }
            _ => panic!("expected resolved review output"),
        }
    }

    #[test]
    fn test_merge_conflict_includes_remediation_message() {
        let result =
            merge_parallel_outputs(&review_token(true, false), &test_token(false, vec!["t1"]))
                .unwrap();
        assert!(!result.conflicts[0].remediation.is_empty());
        assert!(result.conflicts[0]
            .remediation
            .contains("diagnostic_digest"));
    }

    #[test]
    fn test_merge_mismatched_role_pair_returns_error() {
        let plan_token = HandoffToken::new(RoleOutput::Plan {
            task_breakdown: vec!["step1".to_string()],
            estimated_steps: 1,
            required_state_pointers: vec![],
        })
        .unwrap();
        let code_token = HandoffToken::new(RoleOutput::Code {
            patch_digest: "abc123".to_string(),
            files_modified: vec![],
            notes: None,
        })
        .unwrap();

        let result = merge_parallel_outputs(&plan_token, &code_token);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RoleError::ConflictDetected { .. }
        ));
    }

    #[test]
    fn test_role_conflict_is_serializable() {
        let conflict = RoleConflict {
            aspect: "test_aspect".to_string(),
            from_role_a: AgentRole::Reviewer,
            value_a: serde_json::json!({"approved": true}),
            from_role_b: AgentRole::Tester,
            value_b: serde_json::json!({"passed": false}),
            remediation: "fix it".to_string(),
        };
        let json = serde_json::to_string(&conflict).unwrap();
        let back: RoleConflict = serde_json::from_str(&json).unwrap();
        assert_eq!(conflict, back);
    }

    #[test]
    fn test_merge_symmetric_test_then_review_is_equivalent() {
        // token_b = review, token_a = test (reversed order)
        let merged_ab =
            merge_parallel_outputs(&review_token(true, false), &test_token(true, vec![])).unwrap();
        let merged_ba =
            merge_parallel_outputs(&test_token(true, vec![]), &review_token(true, false)).unwrap();
        assert_eq!(merged_ab.is_clean(), merged_ba.is_clean());
        assert_eq!(merged_ab.conflicts.len(), merged_ba.conflicts.len());
    }

    #[test]
    fn test_merge_reviewer_rejected_and_tests_failed_is_conflict() {
        let result =
            merge_parallel_outputs(&review_token(false, false), &test_token(false, vec!["t1"]))
                .unwrap();
        assert!(!result.is_clean());
        assert!(result.resolved.is_none());
        assert!(result
            .conflicts
            .iter()
            .any(|c| c.aspect == "review_rejected_and_tests_failed"));
    }

    #[test]
    fn test_merge_reviewer_rejected_but_tests_passed_uses_test_signal() {
        let result =
            merge_parallel_outputs(&review_token(false, false), &test_token(true, vec![])).unwrap();
        assert!(result.is_clean());
        match result.resolved {
            Some(RoleOutput::Review {
                approved,
                requires_fix,
                ..
            }) => {
                assert!(approved);
                assert!(!requires_fix);
            }
            _ => panic!("expected resolved review output"),
        }
    }
}
