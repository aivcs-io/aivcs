//! CI specification and identity.

use aivcs_core::domain::agent_spec::AgentSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// CI pipeline specification.
///
/// Defines the identity and configuration of a CI run,
/// which is converted to an `AgentSpec` for stable run linking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiSpec {
    /// Workspace root path.
    pub workspace_path: PathBuf,

    /// SHA-256 digest of ordered stage names (deterministic).
    pub stages_digest: String,

    /// Git commit SHA where execution occurred.
    pub git_sha: String,

    /// Toolchain hash from `rustup show` output.
    pub toolchain_hash: String,
}

impl CiSpec {
    /// Create a new CI specification.
    pub fn new(
        workspace_path: PathBuf,
        stages: &[String],
        git_sha: String,
        toolchain_hash: String,
    ) -> Self {
        let stages_digest = compute_stages_digest(stages);
        Self {
            workspace_path,
            stages_digest,
            git_sha,
            toolchain_hash,
        }
    }

    /// Convert to an AIVCS AgentSpec for run identity.
    pub fn to_agent_spec(&self) -> anyhow::Result<AgentSpec> {
        // Compute digests for CI components
        let graph_digest = compute_component_digest(b"ci");
        let prompts_digest = compute_component_digest(self.stages_digest.as_bytes());
        let tools_digest = compute_component_digest(self.git_sha.as_bytes());
        let config_digest = compute_component_digest(self.toolchain_hash.as_bytes());

        AgentSpec::new(
            self.git_sha.clone(),
            graph_digest,
            prompts_digest,
            tools_digest,
            config_digest,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create AgentSpec: {}", e))
    }
}

/// Compute deterministic digest of ordered stage names.
fn compute_stages_digest(stages: &[String]) -> String {
    let mut hasher = Sha256::new();
    for stage in stages {
        hasher.update(stage.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

/// Compute digest of a component.
fn compute_component_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ci_spec_new() {
        let stages = vec!["fmt".to_string(), "check".to_string()];
        let spec = CiSpec::new(
            PathBuf::from("."),
            &stages,
            "abc123".to_string(),
            "rustc_hash_xyz".to_string(),
        );

        assert_eq!(spec.workspace_path, PathBuf::from("."));
        assert_eq!(spec.git_sha, "abc123");
        assert_eq!(spec.toolchain_hash, "rustc_hash_xyz");
        assert!(!spec.stages_digest.is_empty());
    }

    #[test]
    fn test_stages_digest_deterministic() {
        let stages1 = vec!["fmt".to_string(), "check".to_string()];
        let stages2 = vec!["fmt".to_string(), "check".to_string()];

        let digest1 = compute_stages_digest(&stages1);
        let digest2 = compute_stages_digest(&stages2);

        assert_eq!(digest1, digest2);
    }

    #[test]
    fn test_stages_digest_order_sensitive() {
        let stages1 = vec!["fmt".to_string(), "check".to_string()];
        let stages2 = vec!["check".to_string(), "fmt".to_string()];

        let digest1 = compute_stages_digest(&stages1);
        let digest2 = compute_stages_digest(&stages2);

        assert_ne!(digest1, digest2);
    }

    #[test]
    fn test_ci_spec_to_agent_spec() {
        let stages = vec!["fmt".to_string(), "check".to_string()];
        let spec = CiSpec::new(
            PathBuf::from("."),
            &stages,
            "abc123".to_string(),
            "rustc_hash".to_string(),
        );

        let agent_spec = spec.to_agent_spec().expect("to_agent_spec failed");
        assert_eq!(agent_spec.git_sha, "abc123");
        assert!(!agent_spec.spec_digest.is_empty());
    }
}
