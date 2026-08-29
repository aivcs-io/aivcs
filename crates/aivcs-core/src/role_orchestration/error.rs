//! Error types for role orchestration.

/// Errors produced by the role orchestration layer.
#[derive(Debug, thiserror::Error)]
pub enum RoleError {
    #[error("role {role} does not accept handoffs from {from}")]
    UnauthorizedHandoff { role: String, from: String },

    #[error("handoff token integrity check failed: {reason}")]
    InvalidHandoffToken { reason: String },

    #[error("task decomposition produced no subtasks for role {role}")]
    EmptyDecomposition { role: String },

    #[error("role conflict: {description}")]
    ConflictDetected { description: String },

    #[error("parallel role execution error: {detail}")]
    ParallelExecutionFailed { detail: String },

    #[error("domain error: {0}")]
    Domain(#[from] crate::domain::error::AivcsError),
}

/// Result type for role orchestration operations.
pub type RoleResult<T> = std::result::Result<T, RoleError>;
