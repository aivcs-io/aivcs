//! Memory subsystem for agent context, indexing, and retention.
//!
//! Provides in-memory indexing of run traces, rationales, diffs, and snapshots
//! with tag/kind/time filtering, token-budgeted context assembly, and
//! configurable compaction policies.

pub mod context;
pub mod decision;
pub mod error;
pub mod index;
pub mod rationale;
pub mod retention;

pub use context::{assemble_context, ContextBudget, ContextItem, ContextWindow};
pub use decision::{DecisionRecorder, DecisionRecorderConfig};
pub use error::{MemoryError, MemoryResult};
pub use index::{IndexQuery, IndexResult, MemoryEntry, MemoryEntryKind, MemoryIndex};
pub use rationale::{DecisionRationale, RationaleEntry, RationaleOutcome};
pub use retention::{compact_index, CompactionPolicy, CompactionResult};
