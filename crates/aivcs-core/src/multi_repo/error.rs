//! Error types for multi-repo and CI/CD orchestration.

use thiserror::Error;

/// Errors produced by the multi-repo orchestration layer.
#[derive(Debug, Error)]
pub enum MultiRepoError {
    /// A dependency cycle was detected in the repo graph.
    #[error("dependency cycle detected involving repos: {repos:?}")]
    DependencyCycle { repos: Vec<String> },

    /// A referenced repo was not found in the graph.
    #[error("repo not found in graph: {repo}")]
    RepoNotFound { repo: String },

    /// Release sequencing failed for a repo.
    #[error("release sequencing failed for repo {repo}: {reason}")]
    SequencingFailed { repo: String, reason: String },

    /// CI aggregation failed for an objective.
    #[error("CI aggregation error for objective '{objective}': {detail}")]
    AggregationError { objective: String, detail: String },

    /// A backport operation failed.
    #[error("backport failed for commit {commit_sha} → branch {target_branch}: {reason}")]
    BackportFailed {
        commit_sha: String,
        target_branch: String,
        reason: String,
    },

    /// A provenance artifact operation failed.
    #[error("provenance artifact error: {0}")]
    Provenance(String),

    /// A storage / persistence layer error.
    #[error("storage error: {0}")]
    Storage(String),

    /// Bubbled-up domain error.
    #[error("domain error: {0}")]
    Domain(#[from] crate::domain::error::AivcsError),
}

/// Convenience result alias.
pub type MultiRepoResult<T> = std::result::Result<T, MultiRepoError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_cycle_error_displays_repo_names() {
        let err = MultiRepoError::DependencyCycle {
            repos: vec!["org/a".to_string(), "org/b".to_string()],
        };
        let msg = err.to_string();
        assert!(msg.contains("org/a"));
        assert!(msg.contains("org/b"));
    }

    #[test]
    fn test_backport_failed_error_displays_commit_and_branch() {
        let err = MultiRepoError::BackportFailed {
            commit_sha: "abc123".to_string(),
            target_branch: "release/1.0".to_string(),
            reason: "conflict in foo.rs".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("abc123"));
        assert!(msg.contains("release/1.0"));
        assert!(msg.contains("conflict"));
    }

    #[test]
    fn test_repo_not_found_error_displays_repo() {
        let err = MultiRepoError::RepoNotFound {
            repo: "org/missing".to_string(),
        };
        assert!(err.to_string().contains("org/missing"));
    }
}
