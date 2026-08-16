use std::collections::BTreeMap;

use aivcs_core::{
    default_role_templates, deterministic_role_order, merge_role_outputs, validate_handoff,
    validate_parallel_roles, AgentRole, HandoffValidationError, MergeConflictStrategy,
    ParallelPlanError, RoleHandoff, RoleOutput,
};

#[test]
fn planner_to_coder_handoff_validates_with_required_payload() {
    let templates = default_role_templates();
    let handoff = RoleHandoff {
        task_id: "task-1".to_string(),
        from: AgentRole::Planner,
        to: AgentRole::Coder,
        payload: BTreeMap::from([("task_plan".to_string(), "implement x".to_string())]),
    };

    let result = validate_handoff(&templates, &handoff);
    assert!(result.is_ok());
}

#[test]
fn handoff_rejects_missing_required_keys() {
    let templates = default_role_templates();
    let handoff = RoleHandoff {
        task_id: "task-2".to_string(),
        from: AgentRole::Planner,
        to: AgentRole::Coder,
        payload: BTreeMap::new(),
    };

    let result = validate_handoff(&templates, &handoff);
    assert_eq!(
        result,
        Err(HandoffValidationError::MissingRequiredKeys {
            role: AgentRole::Coder,
            keys: vec!["task_plan".to_string()],
        })
    );
}

#[test]
fn handoff_rejects_forbidden_route() {
    let templates = default_role_templates();
    let handoff = RoleHandoff {
        task_id: "task-3".to_string(),
        from: AgentRole::Tester,
        to: AgentRole::Planner,
        payload: BTreeMap::new(),
    };

    let result = validate_handoff(&templates, &handoff);
    assert_eq!(
        result,
        Err(HandoffValidationError::ForbiddenRoute {
            from: AgentRole::Tester,
            to: AgentRole::Planner,
        })
    );
}

#[test]
fn merge_fail_on_conflict_surfaces_conflict_and_keeps_existing() {
    let a = RoleOutput {
        role: AgentRole::Coder,
        step: 1,
        values: BTreeMap::from([("code_patch".to_string(), "v1".to_string())]),
    };
    let b = RoleOutput {
        role: AgentRole::Reviewer,
        step: 2,
        values: BTreeMap::from([("code_patch".to_string(), "v2".to_string())]),
    };

    let merged = merge_role_outputs(&[a, b], MergeConflictStrategy::FailOnConflict);
    assert_eq!(merged.values.get("code_patch"), Some(&"v1".to_string()));
    assert_eq!(merged.conflicts.len(), 1);
    assert_eq!(merged.conflicts[0].key, "code_patch");
}

#[test]
fn merge_prefer_role_priority_uses_fixer_value() {
    let coder = RoleOutput {
        role: AgentRole::Coder,
        step: 1,
        values: BTreeMap::from([("code_patch".to_string(), "initial".to_string())]),
    };
    let fixer = RoleOutput {
        role: AgentRole::Fixer,
        step: 2,
        values: BTreeMap::from([("code_patch".to_string(), "fixed".to_string())]),
    };

    let merged = merge_role_outputs(&[coder, fixer], MergeConflictStrategy::PreferRolePriority);
    assert_eq!(merged.values.get("code_patch"), Some(&"fixed".to_string()));
    assert_eq!(merged.conflicts.len(), 1);
}

#[test]
fn merge_equal_value_does_not_change_later_priority_outcome() {
    let planner = RoleOutput {
        role: AgentRole::Planner,
        step: 1,
        values: BTreeMap::from([("task_plan".to_string(), "same-plan".to_string())]),
    };
    let coder_same = RoleOutput {
        role: AgentRole::Coder,
        step: 2,
        values: BTreeMap::from([("task_plan".to_string(), "same-plan".to_string())]),
    };
    let reviewer_change = RoleOutput {
        role: AgentRole::Reviewer,
        step: 3,
        values: BTreeMap::from([("task_plan".to_string(), "reviewed-plan".to_string())]),
    };

    let merged = merge_role_outputs(
        &[planner, coder_same, reviewer_change],
        MergeConflictStrategy::PreferRolePriority,
    );

    // Reviewer should win on the final conflict regardless of an equal-value intermediate write.
    assert_eq!(
        merged.values.get("task_plan"),
        Some(&"reviewed-plan".to_string())
    );
    assert_eq!(merged.conflicts.len(), 1);
    assert_eq!(merged.conflicts[0].incoming_role, AgentRole::Reviewer);
}

#[test]
fn merge_equal_value_preserves_original_owner_for_priority_tie_breaking() {
    let fixer_first = RoleOutput {
        role: AgentRole::Fixer,
        step: 1,
        values: BTreeMap::from([("code_patch".to_string(), "same".to_string())]),
    };
    let coder_equal = RoleOutput {
        role: AgentRole::Coder,
        step: 2,
        values: BTreeMap::from([("code_patch".to_string(), "same".to_string())]),
    };
    let reviewer_conflict = RoleOutput {
        role: AgentRole::Reviewer,
        step: 3,
        values: BTreeMap::from([("code_patch".to_string(), "review".to_string())]),
    };

    let merged = merge_role_outputs(
        &[fixer_first, coder_equal, reviewer_conflict],
        MergeConflictStrategy::PreferRolePriority,
    );

    // Fixer owns the key from the first write and should remain owner on equal writes.
    assert_eq!(merged.values.get("code_patch"), Some(&"same".to_string()));
    assert_eq!(merged.conflicts.len(), 1);
}

#[test]
fn parallel_role_plan_rejects_duplicate_role() {
    let result = validate_parallel_roles(&[AgentRole::Coder, AgentRole::Coder]);
    assert_eq!(
        result,
        Err(ParallelPlanError::DuplicateRole(AgentRole::Coder))
    );
}

#[test]
fn deterministic_role_order_is_reproducible() {
    let first = deterministic_role_order(&[
        AgentRole::Reviewer,
        AgentRole::Planner,
        AgentRole::Fixer,
        AgentRole::Coder,
    ]);
    let second = deterministic_role_order(&[
        AgentRole::Coder,
        AgentRole::Fixer,
        AgentRole::Planner,
        AgentRole::Reviewer,
    ]);

    assert_eq!(first, second);
    assert_eq!(
        first,
        vec![
            AgentRole::Fixer,
            AgentRole::Reviewer,
            AgentRole::Coder,
            AgentRole::Planner
        ]
    );
}

#[test]
fn end_to_end_plan_code_review_test_fix_flow_is_deterministic() {
    let templates = default_role_templates();

    let planner_to_coder = RoleHandoff {
        task_id: "task-e2e".to_string(),
        from: AgentRole::Planner,
        to: AgentRole::Coder,
        payload: BTreeMap::from([("task_plan".to_string(), "implement feature".to_string())]),
    };
    let coder_to_reviewer = RoleHandoff {
        task_id: "task-e2e".to_string(),
        from: AgentRole::Coder,
        to: AgentRole::Reviewer,
        payload: BTreeMap::from([("code_patch".to_string(), "patch-v1".to_string())]),
    };
    let reviewer_to_tester = RoleHandoff {
        task_id: "task-e2e".to_string(),
        from: AgentRole::Reviewer,
        to: AgentRole::Tester,
        payload: BTreeMap::from([("code_patch".to_string(), "patch-v1".to_string())]),
    };
    let tester_to_fixer = RoleHandoff {
        task_id: "task-e2e".to_string(),
        from: AgentRole::Tester,
        to: AgentRole::Fixer,
        payload: BTreeMap::from([("code_patch".to_string(), "patch-v1".to_string())]),
    };

    assert!(validate_handoff(&templates, &planner_to_coder).is_ok());
    assert!(validate_handoff(&templates, &coder_to_reviewer).is_ok());
    assert!(validate_handoff(&templates, &reviewer_to_tester).is_ok());
    assert!(validate_handoff(&templates, &tester_to_fixer).is_ok());

    let outputs = vec![
        RoleOutput {
            role: AgentRole::Planner,
            step: 1,
            values: BTreeMap::from([("task_plan".to_string(), "implement feature".to_string())]),
        },
        RoleOutput {
            role: AgentRole::Coder,
            step: 2,
            values: BTreeMap::from([("code_patch".to_string(), "patch-v1".to_string())]),
        },
        RoleOutput {
            role: AgentRole::Reviewer,
            step: 3,
            values: BTreeMap::from([("review_notes".to_string(), "needs fix".to_string())]),
        },
        RoleOutput {
            role: AgentRole::Tester,
            step: 4,
            values: BTreeMap::from([("test_report".to_string(), "1 failing".to_string())]),
        },
        RoleOutput {
            role: AgentRole::Fixer,
            step: 5,
            values: BTreeMap::from([
                ("code_patch".to_string(), "patch-v2-fixed".to_string()),
                ("fix_notes".to_string(), "fixed failing test".to_string()),
            ]),
        },
    ];

    let merged_a = merge_role_outputs(&outputs, MergeConflictStrategy::PreferRolePriority);
    let mut reversed = outputs.clone();
    reversed.reverse();
    let merged_b = merge_role_outputs(&reversed, MergeConflictStrategy::PreferRolePriority);

    // Merge result should be reproducible independent of input order.
    assert_eq!(merged_a.values, merged_b.values);
    assert_eq!(
        merged_a.values.get("code_patch"),
        Some(&"patch-v2-fixed".to_string())
    );
    assert!(merged_a.values.contains_key("task_plan"));
    assert!(merged_a.values.contains_key("review_notes"));
    assert!(merged_a.values.contains_key("test_report"));
    assert!(merged_a.values.contains_key("fix_notes"));
}
