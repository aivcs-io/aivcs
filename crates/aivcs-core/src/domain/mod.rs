//! Domain models for AIVCS.
//!
//! Canonical definitions for the core entities:
//! - `AgentSpec`: Immutable specification of an agent
//! - `Run`: Execution instance of an agent
//! - `EvalSuite`: Evaluation framework for testing agents
//! - `Release`: Deployment records

pub mod agent_spec;
pub mod ci;
pub mod ci_event;
pub mod digest;
pub mod error;
pub mod eval;
pub mod platform;
pub mod release;
pub mod run;
pub mod snapshot;
pub mod validation;

// Re-export main types and errors
pub use agent_spec::{AgentSpec, AgentSpecFields};
pub use error::{AivcsError, Result, ValidationError};
pub use eval::{
    DeterministicEvalRunner, EvalCaseResult, EvalRunReport, EvalSuite, EvalTestCase,
    EvalThresholds, ScorerConfig, ScorerType,
};
pub use platform::{EnvValidation, Platform};
pub use release::{Release, ReleaseEnvironment, ReleasePointer};
pub use run::{Event, EventKind, Run, RunStatus};
pub use snapshot::SnapshotMeta;
pub use validation::validate_run_event;
