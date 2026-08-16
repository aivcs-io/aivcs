//! Release and promotion tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Deployment environment for a release.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReleaseEnvironment {
    Dev,
    Staging,
    Production,
}

/// A release of an agent into a specific environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Release {
    /// Unique identifier for this release.
    pub release_id: Uuid,

    /// Name of the agent being released.
    pub agent_name: String,

    /// Digest of the AgentSpec being released.
    pub spec_digest: String,

    /// Digest of the tools configuration.
    pub tools_digest: String,

    /// Digest of the graph definition.
    pub graph_digest: String,

    /// Semantic version of the release.
    pub version: String,

    /// Target environment.
    pub environment: ReleaseEnvironment,

    /// When the release was promoted.
    pub promoted_at: DateTime<Utc>,

    /// User/system that promoted this release.
    pub promoted_by: String,

    /// Additional metadata (changelog, notes, etc.).
    pub metadata: serde_json::Value,
}

impl Release {
    /// Create a new release.
    pub fn new(
        agent_name: String,
        spec_digest: String,
        tools_digest: String,
        graph_digest: String,
        version: String,
        environment: ReleaseEnvironment,
        promoted_by: String,
    ) -> Self {
        Self {
            release_id: Uuid::new_v4(),
            agent_name,
            spec_digest,
            tools_digest,
            graph_digest,
            version,
            environment,
            promoted_at: Utc::now(),
            promoted_by,
            metadata: serde_json::json!({}),
        }
    }
}

/// Pointer to the current and previous release for an agent in an environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleasePointer {
    /// Name of the agent.
    pub agent_name: String,

    /// Digest of the currently deployed spec.
    pub current_spec_digest: String,

    /// Digest of the previously deployed spec (for rollback).
    pub previous_spec_digest: Option<String>,

    /// When the current release was deployed.
    pub last_updated: DateTime<Utc>,
}

impl ReleasePointer {
    /// Create a new release pointer.
    pub fn new(agent_name: String, current_spec_digest: String) -> Self {
        Self {
            agent_name,
            current_spec_digest,
            previous_spec_digest: None,
            last_updated: Utc::now(),
        }
    }

    /// Promote a new spec, moving current to previous.
    pub fn promote(&mut self, new_spec_digest: String) {
        self.previous_spec_digest = Some(self.current_spec_digest.clone());
        self.current_spec_digest = new_spec_digest;
        self.last_updated = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_serde_roundtrip() {
        let release = Release::new(
            "my_agent".to_string(),
            "spec_digest_123".to_string(),
            "tools_digest_abc".to_string(),
            "graph_digest_xyz".to_string(),
            "1.2.3".to_string(),
            ReleaseEnvironment::Production,
            "github-ci".to_string(),
        );

        let json = serde_json::to_string(&release).expect("serialize");
        let deserialized: Release = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(release, deserialized);
    }

    #[test]
    fn test_release_environment_serde() {
        let envs = [
            ReleaseEnvironment::Dev,
            ReleaseEnvironment::Staging,
            ReleaseEnvironment::Production,
        ];

        for env in &envs {
            let json = serde_json::to_string(env).expect("serialize");
            let deserialized: ReleaseEnvironment =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*env, deserialized);
        }
    }

    #[test]
    fn test_release_pointer_previous_is_none() {
        let pointer = ReleasePointer::new("my_agent".to_string(), "spec_digest_abc".to_string());

        assert_eq!(pointer.agent_name, "my_agent");
        assert_eq!(pointer.current_spec_digest, "spec_digest_abc");
        assert!(pointer.previous_spec_digest.is_none());
    }

    #[test]
    fn test_release_pointer_promote() {
        let mut pointer = ReleasePointer::new("my_agent".to_string(), "spec_digest_v1".to_string());

        pointer.promote("spec_digest_v2".to_string());

        assert_eq!(pointer.current_spec_digest, "spec_digest_v2");
        assert_eq!(
            pointer.previous_spec_digest,
            Some("spec_digest_v1".to_string())
        );
    }

    #[test]
    fn test_release_pointer_serde_roundtrip() {
        let pointer =
            ReleasePointer::new("my_agent".to_string(), "spec_digest_current".to_string());

        let json = serde_json::to_string(&pointer).expect("serialize");
        let deserialized: ReleasePointer = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(pointer, deserialized);
    }

    #[test]
    fn test_release_environment_all_variants() {
        let dev_json = serde_json::to_string(&ReleaseEnvironment::Dev).expect("serialize");
        let staging_json = serde_json::to_string(&ReleaseEnvironment::Staging).expect("serialize");
        let prod_json = serde_json::to_string(&ReleaseEnvironment::Production).expect("serialize");

        assert!(dev_json.contains("DEV"));
        assert!(staging_json.contains("STAGING"));
        assert!(prod_json.contains("PRODUCTION"));
    }
}
