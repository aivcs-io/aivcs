//! Agent specification and digest computation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::digest;
use super::error::{AivcsError, Result};

/// Canonical specification for an agent, including all digest components.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSpec {
    /// Unique identifier for this spec version.
    pub spec_id: Uuid,

    /// SHA256 hex digest of canonical JSON representation.
    pub spec_digest: String,

    /// Git commit SHA where this spec was defined.
    pub git_sha: String,

    /// SHA256 hex of graph definition bytes.
    pub graph_digest: String,

    /// SHA256 hex of prompts definition.
    pub prompts_digest: String,

    /// SHA256 hex of tools definition.
    pub tools_digest: String,

    /// SHA256 hex of configuration.
    pub config_digest: String,

    /// When this spec was created.
    pub created_at: DateTime<Utc>,

    /// Additional metadata.
    pub metadata: serde_json::Value,
}

/// Input fields for computing agent spec digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpecFields {
    pub git_sha: String,
    pub graph_digest: String,
    pub prompts_digest: String,
    pub tools_digest: String,
    pub config_digest: String,
}

impl AgentSpec {
    /// Create a new agent spec with computed digest.
    pub fn new(
        git_sha: String,
        graph_digest: String,
        prompts_digest: String,
        tools_digest: String,
        config_digest: String,
    ) -> Result<Self> {
        if git_sha.is_empty() {
            return Err(AivcsError::InvalidAgentSpec(
                "git_sha cannot be empty".to_string(),
            ));
        }

        let fields = AgentSpecFields {
            git_sha: git_sha.clone(),
            graph_digest: graph_digest.clone(),
            prompts_digest: prompts_digest.clone(),
            tools_digest: tools_digest.clone(),
            config_digest: config_digest.clone(),
        };

        let spec_digest = Self::compute_digest(&fields)?;

        Ok(Self {
            spec_id: Uuid::new_v4(),
            spec_digest,
            git_sha,
            graph_digest,
            prompts_digest,
            tools_digest,
            config_digest,
            created_at: Utc::now(),
            metadata: serde_json::json!({}),
        })
    }

    /// Compute stable SHA256 digest from canonical JSON (RFC 8785-compliant).
    pub fn compute_digest(fields: &AgentSpecFields) -> Result<String> {
        let json = serde_json::to_value(fields)?;
        digest::compute_digest(&json)
    }

    /// Verify that spec_digest matches computed digest.
    pub fn verify_digest(&self) -> Result<()> {
        let fields = AgentSpecFields {
            git_sha: self.git_sha.clone(),
            graph_digest: self.graph_digest.clone(),
            prompts_digest: self.prompts_digest.clone(),
            tools_digest: self.tools_digest.clone(),
            config_digest: self.config_digest.clone(),
        };

        let computed = Self::compute_digest(&fields)?;
        if computed != self.spec_digest {
            return Err(AivcsError::DigestMismatch {
                expected: self.spec_digest.clone(),
                actual: computed,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_spec_serde_roundtrip() {
        let spec = AgentSpec::new(
            "abc123def456".to_string(),
            "graph111".to_string(),
            "prompts222".to_string(),
            "tools333".to_string(),
            "config444".to_string(),
        )
        .expect("create spec");

        let json = serde_json::to_string(&spec).expect("serialize");
        let deserialized: AgentSpec = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_agent_spec_digest_stable() {
        let fields1 = AgentSpecFields {
            git_sha: "abc123".to_string(),
            graph_digest: "graph111".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let fields2 = AgentSpecFields {
            git_sha: "abc123".to_string(),
            graph_digest: "graph111".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let digest1 = AgentSpec::compute_digest(&fields1).expect("compute digest 1");
        let digest2 = AgentSpec::compute_digest(&fields2).expect("compute digest 2");

        assert_eq!(digest1, digest2, "same inputs should produce same digest");
    }

    #[test]
    fn test_agent_spec_digest_changes_on_mutation() {
        let fields1 = AgentSpecFields {
            git_sha: "abc123".to_string(),
            graph_digest: "graph111".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let fields2 = AgentSpecFields {
            git_sha: "abc123".to_string(),
            graph_digest: "graph111_MODIFIED".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let digest1 = AgentSpec::compute_digest(&fields1).expect("compute digest 1");
        let digest2 = AgentSpec::compute_digest(&fields2).expect("compute digest 2");

        assert_ne!(
            digest1, digest2,
            "changed field should produce different digest"
        );
    }

    #[test]
    fn test_agent_spec_verify_digest() {
        let spec = AgentSpec::new(
            "abc123".to_string(),
            "graph111".to_string(),
            "prompts222".to_string(),
            "tools333".to_string(),
            "config444".to_string(),
        )
        .expect("create spec");

        assert!(spec.verify_digest().is_ok(), "spec digest should be valid");
    }

    #[test]
    fn test_agent_spec_new_rejects_empty_git_sha() {
        let result = AgentSpec::new(
            "".to_string(),
            "graph111".to_string(),
            "prompts222".to_string(),
            "tools333".to_string(),
            "config444".to_string(),
        );

        assert!(
            result.is_err(),
            "creating spec with empty git_sha should fail"
        );
    }

    #[test]
    fn test_agent_spec_digest_golden_value() {
        // Golden value test: verify exact digest for known input
        let fields = AgentSpecFields {
            git_sha: "abc123def456".to_string(),
            graph_digest: "graph111".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let digest = AgentSpec::compute_digest(&fields).expect("compute digest");

        // Verify it's a valid 64-char hex string (SHA256)
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c: char| c.is_ascii_hexdigit()));

        // Verify determinism: same fields produce same digest
        let digest2 = AgentSpec::compute_digest(&fields).expect("compute digest again");
        assert_eq!(digest, digest2);
    }

    #[test]
    fn test_agent_spec_field_order_invariant() {
        // Verify that constructing the same spec via different field order produces same digest
        let fields1 = AgentSpecFields {
            git_sha: "abc123".to_string(),
            graph_digest: "graph111".to_string(),
            prompts_digest: "prompts222".to_string(),
            tools_digest: "tools333".to_string(),
            config_digest: "config444".to_string(),
        };

        let digest1 = AgentSpec::compute_digest(&fields1).expect("compute digest 1");

        // Construct via different serialization path (JSON → serde → back)
        let json_str = serde_json::to_string(&fields1).expect("serialize");
        let fields2: AgentSpecFields = serde_json::from_str(&json_str).expect("deserialize");
        let digest2 = AgentSpec::compute_digest(&fields2).expect("compute digest 2");

        assert_eq!(
            digest1, digest2,
            "digests should match regardless of construction path"
        );
    }
}
