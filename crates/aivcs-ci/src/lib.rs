//! AIVCS CI - Continuous Integration via AIVCS
//!
//! Provides a CI pipeline orchestrator that:
//! - Executes Cargo stages (fmt, check, clippy, test)
//! - Records all executions as AIVCS runs
//! - Enables replay and gate evaluation

pub mod gate;
pub mod pipeline;
pub mod runner;
pub mod spec;
pub mod stage;

// Re-export key types
pub use gate::{CiGate, GateVerdict};
pub use pipeline::{CiPipeline, PipelineResult};
pub use runner::{CiRunner, StageResult};
pub use spec::CiSpec;
pub use stage::{BuiltinStage, StageConfig};
