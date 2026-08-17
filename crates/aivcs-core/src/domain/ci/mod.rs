//! CI domain model types for AIVCS.
//!
//! Core entities for the CI engine:
//! - `CIRunSpec`: Specification for a CI run (stages, trigger, budgets)
//! - `CIResult`: Outcome of a CI run with per-stage results
//! - `Diagnostic`: Normalized diagnostic from CI stage output
//! - `RepairPlan`: Bounded repair strategy with patch proposals
//! - `VerificationLink`: Links a verified CI run to a commit

pub mod diagnostic;
pub mod repair;
pub mod result;
pub mod run_spec;
pub mod verification;

pub use diagnostic::{Diagnostic, DiagnosticSource, Severity};
pub use repair::{PatchCommit, RepairPlan, RepairStrategy};
pub use result::{CIResult, CIStageResult, CIStatus};
pub use run_spec::{CIRunSpec, CIRunSpecFields, CITrigger};
pub use verification::VerificationLink;
