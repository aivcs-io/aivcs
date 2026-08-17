//! Core role vocabulary: `AgentRole`, `RoleOutput`, `HandoffToken`, `RoleTemplate`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::error::{AivcsError, Result};

/// The five role archetypes in a multi-agent collaboration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Coder,
    Reviewer,
    Tester,
    Fixer,
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AgentRole::Planner => "planner",
            AgentRole::Coder => "coder",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Tester => "tester",
            AgentRole::Fixer => "fixer",
        };
        write!(f, "{s}")
    }
}

/// Typed output produced by a completed role.
///
/// Each variant carries the fields required by the next role in the pipeline.
/// The `serde(tag = "kind")` discriminant is included in the handoff digest, so
/// any field change causes a digest mismatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoleOutput {
    Plan {
        task_breakdown: Vec<String>,
        estimated_steps: u32,
        /// RFC 6901 JSON pointers downstream roles must read.
        required_state_pointers: Vec<String>,
    },
    Code {
        patch_digest: String,
        files_modified: Vec<String>,
        notes: Option<String>,
    },
    Review {
        approved: bool,
        comments: Vec<String>,
        /// If true, `Fixer` must be invoked before `Tester`.
        requires_fix: bool,
    },
    TestReport {
        passed: bool,
        total_cases: u32,
        failed_cases: Vec<String>,
        /// Diagnostic blob stored in CAS; retrieved by `Fixer`.
        diagnostic_digest: Option<String>,
    },
    Fix {
        patch_digest: String,
        resolved_issues: Vec<String>,
    },
}

impl RoleOutput {
    /// The role that produces this output variant.
    pub fn producing_role(&self) -> AgentRole {
        match self {
            RoleOutput::Plan { .. } => AgentRole::Planner,
            RoleOutput::Code { .. } => AgentRole::Coder,
            RoleOutput::Review { .. } => AgentRole::Reviewer,
            RoleOutput::TestReport { .. } => AgentRole::Tester,
            RoleOutput::Fix { .. } => AgentRole::Fixer,
        }
    }
}

/// A validated, content-addressed handoff token passed between roles.
///
/// The `output_digest` is a SHA-256 hex string of `serde_json::to_vec(&output)`.
/// Calling [`HandoffToken::verify`] re-derives the digest and returns
/// [`AivcsError::DigestMismatch`] if the output has been altered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffToken {
    pub token_id: Uuid,
    pub from_role: AgentRole,
    pub output: RoleOutput,
    /// SHA-256 hex digest of `serde_json::to_vec(&output)`.
    pub output_digest: String,
}

impl HandoffToken {
    /// Construct a token, computing and embedding the digest.
    pub fn new(output: RoleOutput) -> Result<Self> {
        use sha2::Digest as _;
        let bytes = serde_json::to_vec(&output)?;
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        Ok(Self {
            token_id: Uuid::new_v4(),
            from_role: output.producing_role(),
            output,
            output_digest: digest,
        })
    }

    /// Verify the embedded digest still matches the output payload.
    ///
    /// Returns `AivcsError::DigestMismatch` if the token has been tampered with.
    pub fn verify(&self) -> Result<()> {
        use sha2::Digest as _;
        let bytes = serde_json::to_vec(&self.output)?;
        let computed = hex::encode(sha2::Sha256::digest(&bytes));
        if computed != self.output_digest {
            return Err(AivcsError::DigestMismatch {
                expected: self.output_digest.clone(),
                actual: computed,
            });
        }
        Ok(())
    }
}

/// Template describing what a role accepts as input and what it produces.
///
/// Templates are static definitions — they do not execute.
#[derive(Debug, Clone)]
pub struct RoleTemplate {
    pub role: AgentRole,
    /// Roles whose `HandoffToken` this role may consume.
    pub accepts_from: Vec<AgentRole>,
    /// Human-readable description (used in logs and CI output).
    pub description: &'static str,
}

impl RoleTemplate {
    /// Returns the canonical set of five templates for a standard pipeline.
    pub fn standard_pipeline() -> Vec<RoleTemplate> {
        vec![
            RoleTemplate {
                role: AgentRole::Planner,
                accepts_from: vec![],
                description: "Decomposes a task into an ordered step plan",
            },
            RoleTemplate {
                role: AgentRole::Coder,
                accepts_from: vec![AgentRole::Planner, AgentRole::Fixer],
                description: "Implements the plan or applies a fix",
            },
            RoleTemplate {
                role: AgentRole::Reviewer,
                accepts_from: vec![AgentRole::Coder],
                description: "Reviews code output and gates merge readiness",
            },
            RoleTemplate {
                role: AgentRole::Tester,
                accepts_from: vec![AgentRole::Coder, AgentRole::Fixer],
                description: "Executes the test suite and produces a TestReport",
            },
            RoleTemplate {
                role: AgentRole::Fixer,
                accepts_from: vec![AgentRole::Reviewer, AgentRole::Tester],
                description: "Resolves review comments or test failures",
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_output() -> RoleOutput {
        RoleOutput::Review {
            approved: true,
            comments: vec!["LGTM".to_string()],
            requires_fix: false,
        }
    }

    #[test]
    fn test_handoff_token_digest_is_stable_for_identical_output() {
        let out_a = review_output();
        let out_b = review_output();

        let token_a = HandoffToken::new(out_a).unwrap();
        let token_b = HandoffToken::new(out_b).unwrap();

        // Same output → same digest (different token_id is fine)
        assert_eq!(token_a.output_digest, token_b.output_digest);
    }

    #[test]
    fn test_handoff_token_verify_rejects_tampered_output() {
        let output = review_output();
        let mut token = HandoffToken::new(output).unwrap();

        // Tamper: replace the output with a different one
        token.output = RoleOutput::Review {
            approved: false,
            comments: vec!["not LGTM".to_string()],
            requires_fix: true,
        };

        let result = token.verify();
        assert!(result.is_err());
        match result.unwrap_err() {
            AivcsError::DigestMismatch { .. } => {}
            other => panic!("Expected DigestMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_handoff_token_verify_passes_for_untampered_token() {
        let token = HandoffToken::new(review_output()).unwrap();
        assert!(token.verify().is_ok());
    }

    #[test]
    fn test_role_output_producing_role_matches_variant() {
        assert_eq!(
            RoleOutput::Plan {
                task_breakdown: vec![],
                estimated_steps: 0,
                required_state_pointers: vec![],
            }
            .producing_role(),
            AgentRole::Planner
        );
        assert_eq!(
            RoleOutput::Code {
                patch_digest: "abc".to_string(),
                files_modified: vec![],
                notes: None,
            }
            .producing_role(),
            AgentRole::Coder
        );
        assert_eq!(review_output().producing_role(), AgentRole::Reviewer);
        assert_eq!(
            RoleOutput::TestReport {
                passed: true,
                total_cases: 0,
                failed_cases: vec![],
                diagnostic_digest: None,
            }
            .producing_role(),
            AgentRole::Tester
        );
        assert_eq!(
            RoleOutput::Fix {
                patch_digest: "def".to_string(),
                resolved_issues: vec![],
            }
            .producing_role(),
            AgentRole::Fixer
        );
    }

    #[test]
    fn test_standard_pipeline_has_five_templates() {
        let templates = RoleTemplate::standard_pipeline();
        assert_eq!(templates.len(), 5);

        let roles: Vec<&AgentRole> = templates.iter().map(|t| &t.role).collect();
        assert!(roles.contains(&&AgentRole::Planner));
        assert!(roles.contains(&&AgentRole::Coder));
        assert!(roles.contains(&&AgentRole::Reviewer));
        assert!(roles.contains(&&AgentRole::Tester));
        assert!(roles.contains(&&AgentRole::Fixer));
    }

    #[test]
    fn test_coder_accepts_from_planner_and_fixer() {
        let templates = RoleTemplate::standard_pipeline();
        let coder = templates
            .iter()
            .find(|t| t.role == AgentRole::Coder)
            .unwrap();
        assert!(coder.accepts_from.contains(&AgentRole::Planner));
        assert!(coder.accepts_from.contains(&AgentRole::Fixer));
    }
}
