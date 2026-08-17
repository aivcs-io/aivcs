//! Error types for the HITL controls module.

/// Errors produced by the human-in-the-loop controls layer.
#[derive(Debug, thiserror::Error)]
pub enum HitlError {
    #[error("approval request expired after {timeout_secs}s")]
    Expired { timeout_secs: u64 },

    #[error("approval rejected by {reviewer}: {reason}")]
    Rejected { reviewer: String, reason: String },

    #[error("duplicate vote from {voter} on checkpoint {checkpoint_id}")]
    DuplicateVote {
        voter: String,
        checkpoint_id: String,
    },

    #[error("checkpoint not found: {0}")]
    CheckpointNotFound(String),

    #[error("intervention failed: {0}")]
    InterventionFailed(String),

    #[error("invalid policy configuration: {0}")]
    InvalidPolicy(String),

    #[error("domain error: {0}")]
    Domain(#[from] crate::domain::error::AivcsError),
}

/// Result type for HITL operations.
pub type HitlResult<T> = std::result::Result<T, HitlError>;
