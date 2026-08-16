//! Error types for the memory subsystem.

/// Errors produced by memory operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("duplicate entry id: {id}")]
    DuplicateEntry { id: String },

    #[error("entry not found: {id}")]
    EntryNotFound { id: String },

    #[error("context budget exceeded: requested {requested} tokens, available {available}")]
    BudgetExceeded { requested: usize, available: usize },

    #[error("invalid query: {0}")]
    InvalidQuery(String),

    #[error("compaction failed: {0}")]
    CompactionFailed(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("domain error: {0}")]
    Domain(String),
}

/// Result type for memory operations.
pub type MemoryResult<T> = std::result::Result<T, MemoryError>;
