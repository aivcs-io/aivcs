//! Agent role orchestration primitives.
//!
//! This module provides deterministic role contracts for multi-agent workflows:
//! - role templates and allowed handoffs
//! - handoff validation
//! - deterministic merge with conflict surfacing
//! - state-safe parallel role scheduling guards

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Canonical orchestration roles for plan -> code -> review -> test -> fix flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Coder,
    Reviewer,
    Tester,
    Fixer,
}

impl AgentRole {
    fn merge_priority(self) -> u8 {
        match self {
            AgentRole::Fixer => 0,
            AgentRole::Reviewer => 1,
            AgentRole::Tester => 2,
            AgentRole::Coder => 3,
            AgentRole::Planner => 4,
        }
    }
}

/// Role contract used for validation and deterministic orchestration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleTemplate {
    pub role: AgentRole,
    pub required_input_keys: BTreeSet<String>,
    pub produced_output_keys: BTreeSet<String>,
    pub allowed_handoffs: BTreeSet<AgentRole>,
}

/// Handoff payload between two roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleHandoff {
    pub task_id: String,
    pub from: AgentRole,
    pub to: AgentRole,
    pub payload: BTreeMap<String, String>,
}

/// A single role output fragment for merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleOutput {
    pub role: AgentRole,
    pub step: u32,
    pub values: BTreeMap<String, String>,
}

/// Merge conflict with explicit remediation context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    pub key: String,
    pub existing_role: AgentRole,
    pub incoming_role: AgentRole,
    pub existing_value: String,
    pub incoming_value: String,
}

/// Strategy for conflicting role outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeConflictStrategy {
    /// Keep original value and surface conflict.
    FailOnConflict,
    /// Resolve conflict by static role priority while surfacing conflict.
    PreferRolePriority,
}

/// Merge result with deterministic output and surfaced conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOutcome {
    pub values: BTreeMap<String, String>,
    pub conflicts: Vec<MergeConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffValidationError {
    MissingTemplate(AgentRole),
    ForbiddenRoute { from: AgentRole, to: AgentRole },
    MissingRequiredKeys { role: AgentRole, keys: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParallelPlanError {
    DuplicateRole(AgentRole),
}

/// Default role templates for the canonical software delivery loop.
pub fn default_role_templates() -> BTreeMap<AgentRole, RoleTemplate> {
    let mut map = BTreeMap::new();

    map.insert(
        AgentRole::Planner,
        RoleTemplate {
            role: AgentRole::Planner,
            required_input_keys: BTreeSet::new(),
            produced_output_keys: ["task_plan".to_string()].into_iter().collect(),
            allowed_handoffs: [AgentRole::Coder, AgentRole::Tester].into_iter().collect(),
        },
    );
    map.insert(
        AgentRole::Coder,
        RoleTemplate {
            role: AgentRole::Coder,
            required_input_keys: ["task_plan".to_string()].into_iter().collect(),
            produced_output_keys: ["code_patch".to_string()].into_iter().collect(),
            allowed_handoffs: [AgentRole::Reviewer, AgentRole::Tester]
                .into_iter()
                .collect(),
        },
    );
    map.insert(
        AgentRole::Reviewer,
        RoleTemplate {
            role: AgentRole::Reviewer,
            required_input_keys: ["code_patch".to_string()].into_iter().collect(),
            produced_output_keys: ["review_notes".to_string()].into_iter().collect(),
            allowed_handoffs: [AgentRole::Fixer, AgentRole::Tester].into_iter().collect(),
        },
    );
    map.insert(
        AgentRole::Tester,
        RoleTemplate {
            role: AgentRole::Tester,
            required_input_keys: ["code_patch".to_string()].into_iter().collect(),
            produced_output_keys: ["test_report".to_string()].into_iter().collect(),
            allowed_handoffs: [AgentRole::Fixer, AgentRole::Reviewer]
                .into_iter()
                .collect(),
        },
    );
    map.insert(
        AgentRole::Fixer,
        RoleTemplate {
            role: AgentRole::Fixer,
            required_input_keys: ["code_patch".to_string()].into_iter().collect(),
            produced_output_keys: ["code_patch".to_string(), "fix_notes".to_string()]
                .into_iter()
                .collect(),
            allowed_handoffs: BTreeSet::new(),
        },
    );

    map
}

/// Validate a role handoff against template contracts.
pub fn validate_handoff(
    templates: &BTreeMap<AgentRole, RoleTemplate>,
    handoff: &RoleHandoff,
) -> Result<(), HandoffValidationError> {
    let from_template = templates
        .get(&handoff.from)
        .ok_or(HandoffValidationError::MissingTemplate(handoff.from))?;
    let to_template = templates
        .get(&handoff.to)
        .ok_or(HandoffValidationError::MissingTemplate(handoff.to))?;

    if !from_template.allowed_handoffs.contains(&handoff.to) {
        return Err(HandoffValidationError::ForbiddenRoute {
            from: handoff.from,
            to: handoff.to,
        });
    }

    let mut missing = Vec::new();
    for key in &to_template.required_input_keys {
        if !handoff.payload.contains_key(key) {
            missing.push(key.clone());
        }
    }
    if !missing.is_empty() {
        return Err(HandoffValidationError::MissingRequiredKeys {
            role: handoff.to,
            keys: missing,
        });
    }

    Ok(())
}

/// Merge role outputs deterministically and surface conflicts.
pub fn merge_role_outputs(outputs: &[RoleOutput], strategy: MergeConflictStrategy) -> MergeOutcome {
    let mut ordered = outputs.to_vec();
    ordered.sort_by_key(|o| (o.step, o.role));

    let mut values = BTreeMap::<String, String>::new();
    let mut owners = BTreeMap::<String, AgentRole>::new();
    let mut conflicts = Vec::<MergeConflict>::new();

    for output in ordered {
        for (key, incoming_value) in output.values {
            match values.get(&key) {
                None => {
                    values.insert(key.clone(), incoming_value.clone());
                    owners.insert(key, output.role);
                }
                Some(existing_value) if existing_value == &incoming_value => {
                    // Preserve the original owner on equal-value writes for stable tie-breaking.
                }
                Some(existing_value) => {
                    let existing_role = *owners.get(&key).unwrap_or(&output.role);
                    conflicts.push(MergeConflict {
                        key: key.clone(),
                        existing_role,
                        incoming_role: output.role,
                        existing_value: existing_value.clone(),
                        incoming_value: incoming_value.clone(),
                    });

                    if matches!(strategy, MergeConflictStrategy::PreferRolePriority)
                        && output.role.merge_priority() < existing_role.merge_priority()
                    {
                        values.insert(key.clone(), incoming_value);
                        owners.insert(key, output.role);
                    }
                }
            }
        }
    }

    MergeOutcome { values, conflicts }
}

/// Validate that a parallel role plan is state-safe (no duplicate role writers).
pub fn validate_parallel_roles(roles: &[AgentRole]) -> Result<(), ParallelPlanError> {
    let mut seen = BTreeSet::new();
    for role in roles {
        if !seen.insert(*role) {
            return Err(ParallelPlanError::DuplicateRole(*role));
        }
    }
    Ok(())
}

/// Deterministic role ordering for reproducible parallel scheduling.
pub fn deterministic_role_order(roles: &[AgentRole]) -> Vec<AgentRole> {
    let mut ordered = roles.to_vec();
    ordered.sort_by_key(|r| (r.merge_priority(), *r));
    ordered
}
