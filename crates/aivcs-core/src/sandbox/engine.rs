//! Policy evaluation engine — first-match-wins, default-deny.

use super::policy::ToolPolicySet;
use super::request::{PolicyVerdict, ToolRequest};

/// Evaluate a [`ToolRequest`] against a [`ToolPolicySet`].
///
/// Rules are checked in order. The first rule whose (role, capability) pair
/// matches determines the verdict. If no rule matches, the request is **denied**
/// (default-deny posture).
pub fn evaluate_tool_request(policy: &ToolPolicySet, request: &ToolRequest) -> PolicyVerdict {
    for rule in &policy.rules {
        if rule.matches(&request.requesting_role, &request.capability) {
            return rule.verdict();
        }
    }

    // Default-deny
    PolicyVerdict::Denied {
        reason: format!(
            "no policy rule matched role={} capability={:?}",
            request.requesting_role, request.capability,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_orchestration::roles::AgentRole;
    use crate::sandbox::capability::ToolCapability;
    use crate::sandbox::policy::{ToolPolicyRule, ToolPolicySet};

    fn make_request(role: AgentRole, capability: ToolCapability) -> ToolRequest {
        ToolRequest {
            tool_name: "test_tool".into(),
            capability,
            params: serde_json::Value::Null,
            requesting_role: role,
        }
    }

    #[test]
    fn test_default_deny_when_no_rules() {
        let policy = ToolPolicySet::empty();
        let req = make_request(AgentRole::Coder, ToolCapability::ShellExec);
        let v = evaluate_tool_request(&policy, &req);
        assert!(!v.is_allowed());
    }

    #[test]
    fn test_first_match_wins() {
        // Deny first, then allow — deny wins.
        let policy = ToolPolicySet::empty()
            .with_rule(ToolPolicyRule::Deny {
                role: AgentRole::Coder,
                capability: ToolCapability::ShellExec,
                reason: "denied first".into(),
            })
            .with_rule(ToolPolicyRule::Allow {
                role: AgentRole::Coder,
                capability: ToolCapability::ShellExec,
            });

        let req = make_request(AgentRole::Coder, ToolCapability::ShellExec);
        match evaluate_tool_request(&policy, &req) {
            PolicyVerdict::Denied { reason } => assert!(reason.contains("denied first")),
            other => panic!("expected Denied, got {:?}", other),
        }
    }

    #[test]
    fn test_standard_dev_coder_allowed_shell() {
        let policy = ToolPolicySet::standard_dev();
        let req = make_request(AgentRole::Coder, ToolCapability::ShellExec);
        assert!(evaluate_tool_request(&policy, &req).is_allowed());
    }

    #[test]
    fn test_standard_dev_reviewer_denied_shell() {
        let policy = ToolPolicySet::standard_dev();
        let req = make_request(AgentRole::Reviewer, ToolCapability::ShellExec);
        assert!(!evaluate_tool_request(&policy, &req).is_allowed());
    }

    #[test]
    fn test_standard_dev_all_roles_denied_http_fetch() {
        let policy = ToolPolicySet::standard_dev();
        for role in &[
            AgentRole::Planner,
            AgentRole::Coder,
            AgentRole::Reviewer,
            AgentRole::Tester,
            AgentRole::Fixer,
        ] {
            let req = make_request(role.clone(), ToolCapability::NetworkFetch);
            assert!(
                !evaluate_tool_request(&policy, &req).is_allowed(),
                "expected HttpFetch denied for {role}"
            );
        }
    }

    #[test]
    fn test_require_approval_verdict() {
        let policy = ToolPolicySet::empty().with_rule(ToolPolicyRule::RequireApproval {
            role: AgentRole::Coder,
            capability: ToolCapability::NetworkFetch,
            reason: "network access needs approval".into(),
        });
        let req = make_request(AgentRole::Coder, ToolCapability::NetworkFetch);
        match evaluate_tool_request(&policy, &req) {
            PolicyVerdict::RequiresApproval { reason } => {
                assert!(reason.contains("network access"));
            }
            other => panic!("expected RequiresApproval, got {:?}", other),
        }
    }
}
