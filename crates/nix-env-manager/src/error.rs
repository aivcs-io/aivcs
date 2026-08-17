//! Error types for nix-env-manager

use thiserror::Error;

/// Errors that can occur in the Nix environment manager
#[derive(Error, Debug)]
pub enum NixError {
    /// Nix command not found
    #[error("Nix is not installed or not in PATH")]
    NixNotFound,

    /// Nix command execution failed
    #[error("Nix command failed: {0}")]
    NixCommandFailed(String),

    /// Flake not found
    #[error("Flake not found at path: {0}")]
    FlakeNotFound(String),

    /// Invalid flake.lock format
    #[error("Invalid flake.lock format: {0}")]
    InvalidFlakeLock(String),

    /// Attic not configured
    #[error("Attic is not configured")]
    AtticNotConfigured,

    /// Attic command failed
    #[error("Attic command failed: {0}")]
    AtticCommandFailed(String),

    /// Environment not found in cache
    #[error("Environment not found in cache: {0}")]
    EnvironmentNotCached(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parsing error
    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    /// HTTP error (for Attic API)
    #[error("HTTP error: {0}")]
    Http(String),

    /// Hash computation error
    #[error("Hash computation failed: {0}")]
    HashError(String),
}

impl From<reqwest::Error> for NixError {
    fn from(err: reqwest::Error) -> Self {
        NixError::Http(err.to_string())
    }
}
