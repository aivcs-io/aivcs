//! Domain-level error taxonomy for AIVCS.

/// Errors produced by event payload validation.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("unknown event kind: {kind}")]
    UnknownEventKind { kind: String },

    #[error("event kind {kind} missing required payload field: {field}")]
    MissingPayloadField { kind: String, field: String },

    #[error("event kind must not be empty")]
    EmptyKind,
}

/// AIVCS domain errors.
#[derive(Debug, thiserror::Error)]
pub enum AivcsError {
    #[error("invalid agent spec: {0}")]
    InvalidAgentSpec(String),

    #[error("invalid CI run spec: {0}")]
    InvalidCIRunSpec(String),

    #[error("run not found: {0}")]
    RunNotFound(uuid::Uuid),

    #[error("eval suite not found: {0}")]
    EvalSuiteNotFound(uuid::Uuid),

    #[error("release conflict: {0}")]
    ReleaseConflict(String),

    #[error("digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    #[error("git error: {0}")]
    GitError(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("storage error: {0}")]
    StorageError(String),

    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("multi-repo error: {0}")]
    MultiRepo(String),
}

/// Result type for AIVCS domain operations.
pub type Result<T> = std::result::Result<T, AivcsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aivcs_error_display() {
        let err = AivcsError::InvalidAgentSpec("missing git_sha".to_string());
        assert!(err.to_string().contains("invalid agent spec"));

        let err = AivcsError::InvalidCIRunSpec("stages cannot be empty".to_string());
        assert!(err.to_string().contains("invalid CI run spec"));

        let id = uuid::Uuid::new_v4();
        let err = AivcsError::RunNotFound(id);
        assert!(err.to_string().contains("run not found"));
    }

    #[test]
    fn test_digest_mismatch_error() {
        let err = AivcsError::DigestMismatch {
            expected: "abc123".to_string(),
            actual: "def456".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("abc123"));
        assert!(msg.contains("def456"));
    }

    #[test]
    fn test_storage_error() {
        let err = AivcsError::StorageError("database connection failed".to_string());
        assert!(err.to_string().contains("storage error"));
        assert!(err.to_string().contains("database connection failed"));
    }
}
