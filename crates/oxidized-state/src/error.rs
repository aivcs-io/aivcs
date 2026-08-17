//! Error types for oxidized-state

use thiserror::Error;

/// Errors that can occur in the state persistence layer
#[derive(Error, Debug)]
pub enum StateError {
    /// Database connection error
    #[error("Database connection failed: {0}")]
    Connection(String),

    /// Database query error
    #[error("Database query failed: {0}")]
    Query(String),

    /// Serialization error
    #[error("Serialization failed: {0}")]
    Serialization(String),

    /// Deserialization error
    #[error("Deserialization failed: {0}")]
    Deserialization(String),

    /// Commit not found
    #[error("Commit not found: {0}")]
    CommitNotFound(String),

    /// Invalid commit ID format
    #[error("Invalid commit ID: {0}")]
    InvalidCommitId(String),

    /// Transaction failed
    #[error("Transaction failed: {0}")]
    Transaction(String),

    /// Schema setup error
    #[error("Schema setup failed: {0}")]
    SchemaSetup(String),
}

/// Errors for the storage trait abstractions (CasStore, RunLedger, ReleaseRegistry)
#[derive(Error, Debug)]
pub enum StorageError {
    /// Content not found in CAS
    #[error("content not found: {digest}")]
    NotFound { digest: String },

    /// Run not found in ledger
    #[error("run not found: {run_id}")]
    RunNotFound { run_id: String },

    /// Run is not in a valid state for the requested operation
    #[error("run {run_id} is {status}, expected {expected}")]
    InvalidRunState {
        run_id: String,
        status: String,
        expected: String,
    },

    /// Release not found in registry
    #[error("release not found: {name}")]
    ReleaseNotFound { name: String },

    /// No previous release to roll back to
    #[error("no previous release for '{name}' to roll back to")]
    NoPreviousRelease { name: String },

    /// Invalid digest string (not valid 64-char hex)
    #[error("invalid digest: {digest}")]
    InvalidDigest { digest: String },

    /// Data integrity violation
    #[error("integrity error: expected {expected}, got {actual}")]
    IntegrityError { expected: String, actual: String },

    /// Backend I/O error
    #[error("storage backend error: {0}")]
    Backend(String),

    /// Serialization/deserialization error
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for StateError {
    fn from(err: serde_json::Error) -> Self {
        StateError::Serialization(err.to_string())
    }
}
