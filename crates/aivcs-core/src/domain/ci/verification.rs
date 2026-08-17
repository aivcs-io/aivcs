//! Verification link types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Links a CI run to its verification status.
///
/// A verification link records whether a CI run (or a repair's rerun)
/// has been verified and can be promoted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationLink {
    /// The CI run that was verified.
    pub ci_run_id: Uuid,

    /// Agent spec digest at the time of verification.
    pub spec_digest: String,

    /// Git SHA at the time of verification.
    pub git_sha: String,

    /// Whether verification passed.
    pub verified: bool,

    /// When verification completed.
    pub verified_at: Option<DateTime<Utc>>,

    /// Optional verification run ID (the rerun that confirmed the fix).
    pub verification_run_id: Option<Uuid>,
}

impl VerificationLink {
    /// Create a new pending verification link.
    pub fn new(ci_run_id: Uuid, spec_digest: String, git_sha: String) -> Self {
        Self {
            ci_run_id,
            spec_digest,
            git_sha,
            verified: false,
            verified_at: None,
            verification_run_id: None,
        }
    }

    /// Mark as verified in-place.
    pub fn verify(&mut self, verification_run_id: Uuid) {
        self.verified = true;
        self.verified_at = Some(Utc::now());
        self.verification_run_id = Some(verification_run_id);
    }

    /// Return a verified copy, useful for builder-style chaining.
    pub fn into_verified(mut self, verification_run_id: Uuid) -> Self {
        self.verify(verification_run_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verification_link_serde_roundtrip() {
        let link = VerificationLink::new(
            Uuid::new_v4(),
            "spec-digest-abc".to_string(),
            "abc123".to_string(),
        );

        let json = serde_json::to_string(&link).expect("serialize");
        let deserialized: VerificationLink = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(link, deserialized);
    }

    #[test]
    fn test_verification_link_new_defaults() {
        let link = VerificationLink::new(Uuid::new_v4(), "spec".to_string(), "sha".to_string());
        assert!(!link.verified);
        assert!(link.verified_at.is_none());
        assert!(link.verification_run_id.is_none());
    }

    #[test]
    fn test_verification_link_verify() {
        let mut link = VerificationLink::new(Uuid::new_v4(), "spec".to_string(), "sha".to_string());

        let verify_run = Uuid::new_v4();
        link.verify(verify_run);

        assert!(link.verified);
        assert!(link.verified_at.is_some());
        assert_eq!(link.verification_run_id, Some(verify_run));
    }

    #[test]
    fn test_verified_link_serde_roundtrip() {
        let link = VerificationLink::new(Uuid::new_v4(), "spec".to_string(), "sha".to_string())
            .into_verified(Uuid::new_v4());

        let json = serde_json::to_string(&link).expect("serialize");
        let deserialized: VerificationLink = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(link, deserialized);
    }
}
