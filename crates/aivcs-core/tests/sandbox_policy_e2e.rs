//! End-to-end policy evaluation tests for the sandbox module.

use aivcs_core::role_orchestration::roles::AgentRole;
use aivcs_core::sandbox::capability::ToolCapability;
use aivcs_core::sandbox::engine::evaluate_tool_request;
use aivcs_core::sandbox::policy::{ToolPolicyRule, ToolPolicySet};
use aivcs_core::sandbox::request::{PolicyVerdict, ToolRequest};

fn make_request(role: AgentRole, capability: ToolCapability) -> ToolRequest {
    ToolRequest {
        tool_name: "test_tool".into(),
        capability,
        params: serde_json::Value::Null,
        requesting_role: role,
    }
}

// -------------------------------------------------------------------------
// Standard dev policy matrix tests
// -------------------------------------------------------------------------

#[test]
fn test_standard_dev_coder_has_full_write_access() {
    let policy = ToolPolicySet::standard_dev();
    for cap in &[
        ToolCapability::FileRead,
        ToolCapability::FileWrite,
        ToolCapability::GitRead,
        ToolCapability::GitWrite,
        ToolCapability::ShellExec,
    ] {
        let req = make_request(AgentRole::Coder, cap.clone());
        assert!(
            evaluate_tool_request(&policy, &req).is_allowed(),
            "Coder should be allowed {cap}"
        );
    }
}

#[test]
fn test_standard_dev_planner_read_only() {
    let policy = ToolPolicySet::standard_dev();

    // Allowed
    for cap in &[ToolCapability::FileRead, ToolCapability::GitRead] {
        let req = make_request(AgentRole::Planner, cap.clone());
        assert!(
            evaluate_tool_request(&policy, &req).is_allowed(),
            "Planner should be allowed {cap}"
        );
    }

    // Denied
    for cap in &[
        ToolCapability::FileWrite,
        ToolCapability::GitWrite,
        ToolCapability::ShellExec,
        ToolCapability::NetworkFetch,
    ] {
        let req = make_request(AgentRole::Planner, cap.clone());
        assert!(
            !evaluate_tool_request(&policy, &req).is_allowed(),
            "Planner should be denied {cap}"
        );
    }
}

#[test]
fn test_standard_dev_reviewer_read_only() {
    let policy = ToolPolicySet::standard_dev();
    let req = make_request(AgentRole::Reviewer, ToolCapability::FileWrite);
    assert!(!evaluate_tool_request(&policy, &req).is_allowed());

    let req = make_request(AgentRole::Reviewer, ToolCapability::FileRead);
    assert!(evaluate_tool_request(&policy, &req).is_allowed());
}

#[test]
fn test_standard_dev_tester_has_shell_but_no_write() {
    let policy = ToolPolicySet::standard_dev();

    let req = make_request(AgentRole::Tester, ToolCapability::ShellExec);
    assert!(evaluate_tool_request(&policy, &req).is_allowed());

    let req = make_request(AgentRole::Tester, ToolCapability::FileWrite);
    assert!(!evaluate_tool_request(&policy, &req).is_allowed());

    let req = make_request(AgentRole::Tester, ToolCapability::GitWrite);
    assert!(!evaluate_tool_request(&policy, &req).is_allowed());
}

#[test]
fn test_standard_dev_fixer_matches_coder() {
    let policy = ToolPolicySet::standard_dev();
    for cap in &[
        ToolCapability::FileRead,
        ToolCapability::FileWrite,
        ToolCapability::GitRead,
        ToolCapability::GitWrite,
        ToolCapability::ShellExec,
    ] {
        let req = make_request(AgentRole::Fixer, cap.clone());
        assert!(
            evaluate_tool_request(&policy, &req).is_allowed(),
            "Fixer should be allowed {cap}"
        );
    }
}

// -------------------------------------------------------------------------
// Custom policy tests
// -------------------------------------------------------------------------

#[test]
fn test_custom_deny_overrides_standard_allow() {
    // Start with standard dev, then prepend a deny rule for Coder + Shell
    let mut policy = ToolPolicySet::standard_dev();
    policy.rules.insert(
        0,
        ToolPolicyRule::Deny {
            role: AgentRole::Coder,
            capability: ToolCapability::ShellExec,
            reason: "shell disabled for this project".into(),
        },
    );

    let req = make_request(AgentRole::Coder, ToolCapability::ShellExec);
    let verdict = evaluate_tool_request(&policy, &req);
    assert!(!verdict.is_allowed());
    match verdict {
        PolicyVerdict::Denied { reason } => {
            assert!(reason.contains("shell disabled"));
        }
        other => panic!("expected Denied, got {:?}", other),
    }
}

#[test]
fn test_require_approval_for_http_fetch() {
    let policy = ToolPolicySet::empty().with_rule(ToolPolicyRule::RequireApproval {
        role: AgentRole::Coder,
        capability: ToolCapability::NetworkFetch,
        reason: "network access requires human approval".into(),
    });

    let req = make_request(AgentRole::Coder, ToolCapability::NetworkFetch);
    match evaluate_tool_request(&policy, &req) {
        PolicyVerdict::RequiresApproval { reason } => {
            assert!(reason.contains("human approval"));
        }
        other => panic!("expected RequiresApproval, got {:?}", other),
    }
}

#[test]
fn test_empty_policy_denies_everything() {
    let policy = ToolPolicySet::empty();
    let req = make_request(AgentRole::Coder, ToolCapability::FileRead);
    let verdict = evaluate_tool_request(&policy, &req);
    assert!(!verdict.is_allowed());
    match verdict {
        PolicyVerdict::Denied { reason } => {
            assert!(reason.contains("no policy rule matched"));
        }
        other => panic!("expected default Denied, got {:?}", other),
    }
}

#[test]
fn test_custom_capability_denied_by_default() {
    let policy = ToolPolicySet::standard_dev();
    let req = make_request(AgentRole::Coder, ToolCapability::Custom("deploy".into()));
    assert!(!evaluate_tool_request(&policy, &req).is_allowed());
}

#[test]
fn test_policy_serde_roundtrip_e2e() {
    let policy = ToolPolicySet::standard_dev()
        .with_rule(ToolPolicyRule::RequireApproval {
            role: AgentRole::Coder,
            capability: ToolCapability::NetworkFetch,
            reason: "needs approval".into(),
        })
        .with_rule(ToolPolicyRule::Deny {
            role: AgentRole::Tester,
            capability: ToolCapability::Custom("deploy".into()),
            reason: "no deploy in test".into(),
        });

    let json = serde_json::to_string_pretty(&policy).unwrap();
    let restored: ToolPolicySet = serde_json::from_str(&json).unwrap();
    assert_eq!(policy, restored);

    // Verify the restored policy produces the same verdicts
    let req = make_request(AgentRole::Coder, ToolCapability::NetworkFetch);
    let v1 = evaluate_tool_request(&policy, &req);
    let v2 = evaluate_tool_request(&restored, &req);
    assert_eq!(v1, v2);
}
