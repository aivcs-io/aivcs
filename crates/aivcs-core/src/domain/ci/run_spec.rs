//! CI run specification and digest computation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::digest;
use crate::domain::error::{AivcsError, Result};

/// Default per-stage timeout in milliseconds (5 minutes).
pub const DEFAULT_STAGE_TIMEOUT_MS: u64 = 300_000;
/// Default total timeout in milliseconds (20 minutes).
pub const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 1_200_000;

/// What triggered a CI run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CITrigger {
    /// Manually triggered by user or agent.
    Manual,
    /// Triggered before merge (PR check).
    PreMerge,
    /// Triggered after a commit lands.
    PostCommit,
    /// Triggered on a schedule.
    Scheduled,
}

/// Specification for a CI run, including stages, budgets, and trigger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CIRunSpec {
    /// Unique identifier for this run spec.
    pub run_id: Uuid,

    /// SHA256 hex digest computed from the spec fields.
    pub spec_digest: String,

    /// Git commit SHA for this CI run.
    pub git_sha: String,

    /// Ordered stages to execute (e.g. ["fmt", "clippy", "test"]).
    pub stages: Vec<String>,

    /// What triggered this run.
    pub trigger: CITrigger,

    /// Per-stage timeout in milliseconds.
    pub stage_timeout_ms: u64,

    /// Maximum total duration in milliseconds.
    pub total_timeout_ms: u64,

    /// When this spec was created.
    pub created_at: DateTime<Utc>,
}

/// Input fields for computing CI run spec digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CIRunSpecFields {
    pub git_sha: String,
    pub stages: Vec<String>,
    pub trigger: CITrigger,
    pub stage_timeout_ms: u64,
    pub total_timeout_ms: u64,
}

impl CIRunSpec {
    /// Create a new CI run spec with computed digest.
    pub fn new(git_sha: String, stages: Vec<String>, trigger: CITrigger) -> Result<Self> {
        Self::new_with_timeouts(
            git_sha,
            stages,
            trigger,
            DEFAULT_STAGE_TIMEOUT_MS,
            DEFAULT_TOTAL_TIMEOUT_MS,
        )
    }

    /// Create a new CI run spec with explicit timeout budgets.
    pub fn new_with_timeouts(
        git_sha: String,
        stages: Vec<String>,
        trigger: CITrigger,
        stage_timeout_ms: u64,
        total_timeout_ms: u64,
    ) -> Result<Self> {
        if git_sha.is_empty() {
            return Err(AivcsError::InvalidCIRunSpec(
                "git_sha cannot be empty".to_string(),
            ));
        }
        if stages.is_empty() {
            return Err(AivcsError::InvalidCIRunSpec(
                "stages cannot be empty".to_string(),
            ));
        }
        if stage_timeout_ms == 0 {
            return Err(AivcsError::InvalidCIRunSpec(
                "stage_timeout_ms must be > 0".to_string(),
            ));
        }
        if total_timeout_ms == 0 {
            return Err(AivcsError::InvalidCIRunSpec(
                "total_timeout_ms must be > 0".to_string(),
            ));
        }
        if total_timeout_ms < stage_timeout_ms {
            return Err(AivcsError::InvalidCIRunSpec(
                "total_timeout_ms must be >= stage_timeout_ms".to_string(),
            ));
        }

        let fields = CIRunSpecFields {
            git_sha: git_sha.clone(),
            stages: stages.clone(),
            trigger,
            stage_timeout_ms,
            total_timeout_ms,
        };

        let spec_digest = Self::compute_digest(&fields)?;

        Ok(Self {
            run_id: Uuid::new_v4(),
            spec_digest,
            git_sha,
            stages,
            trigger,
            stage_timeout_ms,
            total_timeout_ms,
            created_at: Utc::now(),
        })
    }

    /// Compute stable SHA256 digest from canonical JSON (RFC 8785-compliant).
    pub fn compute_digest(fields: &CIRunSpecFields) -> Result<String> {
        let json = serde_json::to_value(fields)?;
        digest::compute_digest(&json)
    }

    /// Verify that spec_digest matches computed digest.
    pub fn verify_digest(&self) -> Result<()> {
        let fields = CIRunSpecFields {
            git_sha: self.git_sha.clone(),
            stages: self.stages.clone(),
            trigger: self.trigger,
            stage_timeout_ms: self.stage_timeout_ms,
            total_timeout_ms: self.total_timeout_ms,
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
    fn test_ci_run_spec_serde_roundtrip() {
        let spec = CIRunSpec::new(
            "abc123".to_string(),
            vec!["fmt".to_string(), "clippy".to_string(), "test".to_string()],
            CITrigger::PreMerge,
        )
        .expect("create spec");

        let json = serde_json::to_string(&spec).expect("serialize");
        let deserialized: CIRunSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_ci_run_spec_digest_stable() {
        let fields1 = CIRunSpecFields {
            git_sha: "abc123".to_string(),
            stages: vec!["fmt".to_string(), "test".to_string()],
            trigger: CITrigger::Manual,
            stage_timeout_ms: DEFAULT_STAGE_TIMEOUT_MS,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
        };
        let fields2 = fields1.clone();

        let d1 = CIRunSpec::compute_digest(&fields1).expect("digest 1");
        let d2 = CIRunSpec::compute_digest(&fields2).expect("digest 2");
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_ci_run_spec_digest_changes_on_mutation() {
        let fields1 = CIRunSpecFields {
            git_sha: "abc123".to_string(),
            stages: vec!["fmt".to_string()],
            trigger: CITrigger::Manual,
            stage_timeout_ms: DEFAULT_STAGE_TIMEOUT_MS,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
        };
        let fields2 = CIRunSpecFields {
            git_sha: "abc123".to_string(),
            stages: vec!["fmt".to_string(), "test".to_string()],
            trigger: CITrigger::Manual,
            stage_timeout_ms: DEFAULT_STAGE_TIMEOUT_MS,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
        };

        let d1 = CIRunSpec::compute_digest(&fields1).expect("digest 1");
        let d2 = CIRunSpec::compute_digest(&fields2).expect("digest 2");
        assert_ne!(d1, d2);
    }

    #[test]
    fn test_ci_run_spec_verify_digest() {
        let spec = CIRunSpec::new(
            "abc123".to_string(),
            vec!["fmt".to_string()],
            CITrigger::PostCommit,
        )
        .expect("create spec");

        assert!(spec.verify_digest().is_ok());
    }

    #[test]
    fn test_ci_run_spec_rejects_empty_git_sha() {
        let result = CIRunSpec::new("".to_string(), vec!["fmt".to_string()], CITrigger::Manual);
        assert!(matches!(result, Err(AivcsError::InvalidCIRunSpec(_))));
    }

    #[test]
    fn test_ci_run_spec_rejects_empty_stages() {
        let result = CIRunSpec::new("abc123".to_string(), vec![], CITrigger::Manual);
        assert!(matches!(result, Err(AivcsError::InvalidCIRunSpec(_))));
    }

    #[test]
    fn test_ci_run_spec_accepts_custom_timeouts() {
        let spec = CIRunSpec::new_with_timeouts(
            "abc123".to_string(),
            vec!["fmt".to_string(), "test".to_string()],
            CITrigger::Manual,
            60_000,
            600_000,
        )
        .expect("create spec");
        assert_eq!(spec.stage_timeout_ms, 60_000);
        assert_eq!(spec.total_timeout_ms, 600_000);
    }

    #[test]
    fn test_ci_trigger_serde() {
        let triggers = [
            CITrigger::Manual,
            CITrigger::PreMerge,
            CITrigger::PostCommit,
            CITrigger::Scheduled,
        ];
        for trigger in &triggers {
            let json = serde_json::to_string(trigger).expect("serialize");
            let deserialized: CITrigger = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*trigger, deserialized);
        }
    }
}
