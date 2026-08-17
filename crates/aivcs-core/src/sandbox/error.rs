//! Error types for the sandbox module.

/// Errors produced by the sandbox layer.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("policy denied: {reason}")]
    PolicyDenied { reason: String },

    #[error("tool execution timed out after {elapsed_ms}ms (limit {limit_ms}ms)")]
    Timeout { elapsed_ms: u64, limit_ms: u64 },

    #[error("tool execution failed after {attempts} attempt(s): {reason}")]
    ExecutionFailed { attempts: u32, reason: String },

    #[error(
        "circuit breaker open: {consecutive_failures} consecutive failures (threshold {threshold})"
    )]
    CircuitBreakerOpen {
        consecutive_failures: u32,
        threshold: u32,
    },

    #[error("invalid sandbox configuration: {0}")]
    InvalidConfig(String),

    #[error("domain error: {0}")]
    Domain(#[from] crate::domain::error::AivcsError),
}

/// Result type for sandbox operations.
pub type SandboxResult<T> = std::result::Result<T, SandboxError>;
