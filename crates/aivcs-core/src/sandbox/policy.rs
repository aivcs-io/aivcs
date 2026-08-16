//! Policy rules and policy sets for sandbox tool authorization.

use serde::{Deserialize, Serialize};

use crate::role_orchestration::roles::AgentRole;

use super::capability::ToolCapability;
use super::request::PolicyVerdict;

/// A single policy rule that matches a (role, capability) pair and yields a verdict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPolicyRule {
    /// Allow a specific role to use a specific capability.
    Allow {
        role: AgentRole,
        capability: ToolCapability,
    },
    /// Deny a specific role from using a specific capability.
    Deny {
        role: AgentRole,
        capability: ToolCapability,
        reason: String,
    },
    /// Require approval for a specific role + capability combination.
    RequireApproval {
        role: AgentRole,
        capability: ToolCapability,
        reason: String,
    },
}

impl ToolPolicyRule {
    /// Returns `true` if this rule matches the given (role, capability) pair.
    pub fn matches(&self, role: &AgentRole, capability: &ToolCapability) -> bool {
        match self {
            ToolPolicyRule::Allow {
                role: r,
                capability: c,
            }
            | ToolPolicyRule::Deny {
                role: r,
                capability: c,
                ..
            }
            | ToolPolicyRule::RequireApproval {
                role: r,
                capability: c,
                ..
            } => r == role && c == capability,
        }
    }

    /// The verdict this rule produces when it matches.
    pub fn verdict(&self) -> PolicyVerdict {
        match self {
            ToolPolicyRule::Allow { .. } => PolicyVerdict::Allowed,
            ToolPolicyRule::Deny { reason, .. } => PolicyVerdict::Denied {
                reason: reason.clone(),
            },
            ToolPolicyRule::RequireApproval { reason, .. } => PolicyVerdict::RequiresApproval {
                reason: reason.clone(),
            },
        }
    }
}

/// An ordered set of policy rules evaluated first-match-wins.
///
/// If no rule matches, the default verdict is **Denied** (default-deny posture).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolPolicySet {
    pub rules: Vec<ToolPolicyRule>,
}

impl ToolPolicySet {
    /// Create an empty policy set (everything denied by default).
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    /// Append a rule and return `self` (builder pattern).
    pub fn with_rule(mut self, rule: ToolPolicyRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// The standard developer-mode policy.
    ///
    /// | Role     | FileRead | FileWrite | GitRead | GitWrite | Shell | HttpFetch |
    /// |----------|----------|-----------|---------|----------|-------|-----------|
    /// | Planner  |    ✓     |     ✗     |    ✓    |    ✗     |   ✗   |     ✗     |
    /// | Coder    |    ✓     |     ✓     |    ✓    |    ✓     |   ✓   |     ✗     |
    /// | Reviewer |    ✓     |     ✗     |    ✓    |    ✗     |   ✗   |     ✗     |
    /// | Tester   |    ✓     |     ✗     |    ✓    |    ✗     |   ✓   |     ✗     |
    /// | Fixer    |    ✓     |     ✓     |    ✓    |    ✓     |   ✓   |     ✗     |
    pub fn standard_dev() -> Self {
        let mut rules = Vec::new();

        // Helper: allow a role a set of capabilities.
        let allow = |rules: &mut Vec<ToolPolicyRule>, role: AgentRole, caps: &[ToolCapability]| {
            for cap in caps {
                rules.push(ToolPolicyRule::Allow {
                    role: role.clone(),
                    capability: cap.clone(),
                });
            }
        };

        let read_only = &[ToolCapability::FileRead, ToolCapability::GitRead];

        // Planner — read-only
        allow(&mut rules, AgentRole::Planner, read_only);

        // Coder — full read/write + shell
        allow(
            &mut rules,
            AgentRole::Coder,
            &[
                ToolCapability::FileRead,
                ToolCapability::FileWrite,
                ToolCapability::GitRead,
                ToolCapability::GitWrite,
                ToolCapability::ShellExec,
            ],
        );

        // Reviewer — read-only
        allow(&mut rules, AgentRole::Reviewer, read_only);

        // Tester — read + shell
        allow(
            &mut rules,
            AgentRole::Tester,
            &[
                ToolCapability::FileRead,
                ToolCapability::GitRead,
                ToolCapability::ShellExec,
            ],
        );

        // Fixer — full read/write + shell (same as Coder)
        allow(
            &mut rules,
            AgentRole::Fixer,
            &[
                ToolCapability::FileRead,
                ToolCapability::FileWrite,
                ToolCapability::GitRead,
                ToolCapability::GitWrite,
                ToolCapability::ShellExec,
            ],
        );

        Self { rules }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_matches_correct_pair() {
        let rule = ToolPolicyRule::Allow {
            role: AgentRole::Coder,
            capability: ToolCapability::ShellExec,
        };
        assert!(rule.matches(&AgentRole::Coder, &ToolCapability::ShellExec));
        assert!(!rule.matches(&AgentRole::Reviewer, &ToolCapability::ShellExec));
        assert!(!rule.matches(&AgentRole::Coder, &ToolCapability::NetworkFetch));
    }

    #[test]
    fn test_deny_rule_verdict() {
        let rule = ToolPolicyRule::Deny {
            role: AgentRole::Planner,
            capability: ToolCapability::ShellExec,
            reason: "planners cannot shell".into(),
        };
        assert!(rule.matches(&AgentRole::Planner, &ToolCapability::ShellExec));
        match rule.verdict() {
            PolicyVerdict::Denied { reason } => {
                assert!(reason.contains("planners cannot shell"));
            }
            other => panic!("expected Denied, got {:?}", other),
        }
    }

    #[test]
    fn test_standard_dev_has_rules() {
        let policy = ToolPolicySet::standard_dev();
        // 2 (planner) + 5 (coder) + 2 (reviewer) + 3 (tester) + 5 (fixer) = 17
        assert_eq!(policy.rules.len(), 17);
    }

    #[test]
    fn test_with_rule_appends() {
        let policy = ToolPolicySet::empty().with_rule(ToolPolicyRule::Allow {
            role: AgentRole::Coder,
            capability: ToolCapability::NetworkFetch,
        });
        assert_eq!(policy.rules.len(), 1);
    }

    #[test]
    fn test_policy_serde_roundtrip() {
        let policy = ToolPolicySet::standard_dev();
        let json = serde_json::to_string(&policy).unwrap();
        let back: ToolPolicySet = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }
}
