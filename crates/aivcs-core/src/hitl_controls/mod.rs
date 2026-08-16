//! Human-in-the-Loop (HITL) Controls for AIVCS.
//!
//! Provides robust oversight controls without stalling autonomous throughput:
//!
//! - **Risk-tiered approval checkpoints** — actions are classified by risk tier
//!   (Low / Medium / High / Critical) with escalating approval requirements.
//! - **Pause / edit / continue intervention controls** — operators can pause
//!   execution, modify parameters, and resume without state loss.
//! - **Explainability summaries** — every checkpoint carries a structured
//!   explanation of what changed and why the action was flagged.
//! - **Immutable audit artifacts** — all votes, interventions, and decisions
//!   are recorded with tamper-evident digests.

pub mod artifact;
pub mod checkpoint;
pub mod engine;
pub mod error;
pub mod intervention;
pub mod policy;
pub mod risk;
pub mod vote;

pub use artifact::{read_hitl_artifact, write_hitl_artifact, DecisionSummary, HitlArtifact};
pub use checkpoint::{ApprovalCheckpoint, CheckpointStatus, ExplainabilitySummary};
pub use engine::{apply_intervention, evaluate_checkpoint, submit_vote};
pub use error::{HitlError, HitlResult};
pub use intervention::{Intervention, InterventionAction};
pub use policy::{ApprovalPolicy, ApprovalRule};
pub use risk::RiskTier;
pub use vote::{ApprovalVote, VoteDecision};
