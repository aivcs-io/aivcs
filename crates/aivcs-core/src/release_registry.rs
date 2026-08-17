use crate::domain::agent_spec::AgentSpec;
use crate::domain::error::{AivcsError, Result};
use oxidized_state::{
    ContentDigest, ReleaseMetadata, ReleaseRecord, ReleaseRegistry, StorageResult,
};

/// Validate an `AgentSpec` for promotion, returning the derived `ContentDigest`.
///
/// Checks (in order):
/// 1. All component digests are non-empty.
/// 2. `git_sha` is non-empty.
/// 3. `spec_digest` matches the recomputed digest over components.
/// 4. `spec_digest` is valid 64-char lowercase hex.
fn validate_spec_for_promote(spec: &AgentSpec) -> Result<ContentDigest> {
    if spec.graph_digest.is_empty() {
        return Err(AivcsError::InvalidAgentSpec(
            "graph_digest is empty".to_string(),
        ));
    }
    if spec.prompts_digest.is_empty() {
        return Err(AivcsError::InvalidAgentSpec(
            "prompts_digest is empty".to_string(),
        ));
    }
    if spec.tools_digest.is_empty() {
        return Err(AivcsError::InvalidAgentSpec(
            "tools_digest is empty".to_string(),
        ));
    }
    if spec.config_digest.is_empty() {
        return Err(AivcsError::InvalidAgentSpec(
            "config_digest is empty".to_string(),
        ));
    }
    if spec.git_sha.is_empty() {
        return Err(AivcsError::InvalidAgentSpec("git_sha is empty".to_string()));
    }

    spec.verify_digest()?;

    ContentDigest::try_from(spec.spec_digest.clone())
        .map_err(|e| AivcsError::InvalidAgentSpec(format!("spec_digest is not valid hex: {}", e)))
}

/// Thin API layer over a release registry backend.
pub struct ReleaseRegistryApi<R> {
    registry: R,
}

impl<R> ReleaseRegistryApi<R>
where
    R: ReleaseRegistry,
{
    pub fn new(registry: R) -> Self {
        Self { registry }
    }

    pub async fn promote(
        &self,
        name: &str,
        spec: &AgentSpec,
        promoted_by: &str,
        version_label: Option<String>,
        notes: Option<String>,
    ) -> Result<ReleaseRecord> {
        let content_digest = validate_spec_for_promote(spec)?;

        let metadata = ReleaseMetadata {
            version_label,
            promoted_by: promoted_by.to_string(),
            notes,
        };
        self.registry
            .promote(name, &content_digest, metadata)
            .await
            .map_err(|e| AivcsError::StorageError(e.to_string()))
    }

    pub async fn rollback(&self, name: &str) -> StorageResult<ReleaseRecord> {
        self.registry.rollback(name).await
    }

    pub async fn current(&self, name: &str) -> StorageResult<Option<ReleaseRecord>> {
        self.registry.current(name).await
    }

    pub async fn history(&self, name: &str) -> StorageResult<Vec<ReleaseRecord>> {
        self.registry.history(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_spec::AgentSpec;
    use oxidized_state::fakes::MemoryReleaseRegistry;

    fn make_spec(seed: &str) -> AgentSpec {
        AgentSpec::new(
            "abc123def456abc123def456abc123def456abc1".to_string(),
            format!("graph-{}", seed),
            format!("prompts-{}", seed),
            format!("tools-{}", seed),
            format!("config-{}", seed),
        )
        .expect("make_spec")
    }

    #[tokio::test]
    async fn promote_promote_rollback_keeps_append_only_history() {
        let api = ReleaseRegistryApi::new(MemoryReleaseRegistry::new());
        let name = "agent-registry";
        let spec1 = make_spec("v1");
        let spec2 = make_spec("v2");

        let first = api
            .promote(
                name,
                &spec1,
                "ci",
                Some("v1.0.0".to_string()),
                Some("first release".to_string()),
            )
            .await
            .expect("first promote");
        assert_eq!(first.spec_digest.as_str(), spec1.spec_digest);

        let second = api
            .promote(
                name,
                &spec2,
                "ci",
                Some("v1.1.0".to_string()),
                Some("second release".to_string()),
            )
            .await
            .expect("second promote");
        assert_eq!(second.spec_digest.as_str(), spec2.spec_digest);

        let rolled_back = api.rollback(name).await.expect("rollback");
        assert_eq!(rolled_back.spec_digest.as_str(), spec1.spec_digest);

        let current = api
            .current(name)
            .await
            .expect("current")
            .expect("current exists");
        assert_eq!(current.spec_digest.as_str(), spec1.spec_digest);

        let history = api.history(name).await.expect("history");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].spec_digest.as_str(), spec1.spec_digest);
        assert_eq!(history[1].spec_digest.as_str(), spec2.spec_digest);
        assert_eq!(history[2].spec_digest.as_str(), spec1.spec_digest);
    }
}
