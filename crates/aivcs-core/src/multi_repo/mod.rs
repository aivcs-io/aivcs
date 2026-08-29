//! Multi-repo and CI/CD orchestration (EPIC9).
//!
//! Provides:
//! - [`graph::RepoDependencyGraph`] / [`graph::RepoExecutionPlan`] — topological repo ordering
//! - [`sequencer::ReleaseSequencer`] — cross-repo coordinated release sequencing
//! - [`aggregator::CiAggregator`] / [`aggregator::CiHealthReport`] — unified CI signal collection
//! - [`backport::BackportPolicy`] / [`backport::BackportExecutor`] — automated backport application
//! - [`health::CIHealthView`] — unified CI health view across repos
//! - [`model::CrossRepoGraph`] — cross-repo dependency model
//! - [`orchestrator::MultiRepoOrchestrator`] — multi-repo execution orchestrator
//! - [`provenance::ReleaseProvenance`] — release artifact provenance

pub mod aggregator;
pub mod audit;
pub mod backport;
pub mod error;
pub mod graph;
pub mod health;
pub mod model;
pub mod orchestrator;
pub mod provenance;
pub mod sequencer;

pub use aggregator::{CiAggregator, CiHealthReport, CiRunFetcher, RepoHealth, RepoHealthStatus};
pub use backport::{BackportExecutor, BackportOutcome, BackportPolicy, BackportTask};
pub use error::{MultiRepoError, MultiRepoResult};
pub use graph::{RepoDependencyGraph, RepoExecutionPlan, RepoNode, RepoStep};
pub use health::{CIHealthView, RepoCIStatus};
pub use model::{CrossRepoGraph, RepoDependency, RepoId};
pub use orchestrator::{MultiRepoExecutionPlan, MultiRepoOrchestrator};
pub use provenance::ReleaseProvenance;
pub use sequencer::{
    ReleaseSequencer, RepoReleaseStatus, RepoReleaser, SequenceItem, SequenceOutcome, SequencePlan,
};
