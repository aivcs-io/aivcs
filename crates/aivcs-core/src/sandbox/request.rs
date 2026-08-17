//! Tool request and policy verdict types.

use serde::{Deserialize, Serialize};

use crate::role_orchestration::roles::AgentRole;

use super::capability::ToolCapability;

/// A request to invoke a tool, submitted by a role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolRequest {
    /// Human-readable tool name (e.g. "bash", "git_commit").
    pub tool_name: String,
    /// The capability this tool exercises.
    pub capability: ToolCapability,
    /// Opaque parameters forwarded to the tool executor.
    pub params: serde_json::Value,
    /// The role that is requesting the tool invocation.
    pub requesting_role: AgentRole,
}

/// Outcome of evaluating a tool request against a policy set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyVerdict {
    /// The request is allowed.
    Allowed,
    /// The request is denied with a reason.
    Denied { reason: String },
    /// The request requires explicit human approval before proceeding.
    RequiresApproval { reason: String },
}

impl PolicyVerdict {
    /// Returns `true` when the verdict is `Allowed`.
    pub fn is_allowed(&self) -> bool {
        matches!(self, PolicyVerdict::Allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_verdict_is_allowed() {
        assert!(PolicyVerdict::Allowed.is_allowed());
        assert!(!PolicyVerdict::Denied {
            reason: "nope".into()
        }
        .is_allowed());
        assert!(!PolicyVerdict::RequiresApproval {
            reason: "ask".into()
        }
        .is_allowed());
    }

    #[test]
    fn test_tool_request_serde_roundtrip() {
        let req = ToolRequest {
            tool_name: "bash".into(),
            capability: ToolCapability::ShellExec,
            params: serde_json::json!({"cmd": "ls"}),
            requesting_role: AgentRole::Coder,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ToolRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }
}
