//! Snapshot metadata linking agent state to a git commit.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata for a CAS-backed snapshot tied to a git commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotMeta {
    /// CAS digest (hex) of the stored state blob.
    pub cas_digest: String,

    /// Git HEAD SHA at time of snapshot.
    pub git_sha: String,

    /// Human-readable message.
    pub message: String,

    /// Author or agent that created the snapshot.
    pub author: String,

    /// Branch the snapshot was committed to.
    pub branch: String,

    /// When the snapshot was created.
    pub created_at: DateTime<Utc>,
}

impl SnapshotMeta {
    pub fn new(
        cas_digest: String,
        git_sha: String,
        message: String,
        author: String,
        branch: String,
    ) -> Self {
        Self {
            cas_digest,
            git_sha,
            message,
            author,
            branch,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_meta_serde_roundtrip() {
        let meta = SnapshotMeta::new(
            "abc123".to_string(),
            "deadbeef".repeat(5),
            "test snapshot".to_string(),
            "agent".to_string(),
            "main".to_string(),
        );
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SnapshotMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, deserialized);
    }

    #[test]
    fn snapshot_meta_contains_git_sha() {
        let sha = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string();
        let meta = SnapshotMeta::new(
            "digest".to_string(),
            sha.clone(),
            "msg".to_string(),
            "agent".to_string(),
            "main".to_string(),
        );
        assert_eq!(meta.git_sha, sha);
    }
}
