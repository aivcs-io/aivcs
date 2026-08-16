//! `aivcs-actions` — the **AIVCS Actions v1 contract** (Phase 1 of
//! FR_SANDLOT_BACKED_AIVCS_ACTIONS, code-governance#1119 / #1276).
//!
//! This crate is the versioned unit the whole automation path shares
//! (contract-versioning-baseline): the public [`AutomationIntent`] envelope, the
//! monotonic run-lifecycle [`RunState`] machine, and the fail-closed
//! [`validate_intent`] admission rules apps-middle-ware owns.
//!
//! It is deliberately **transport- and executor-free** — no axum, no data-mesh,
//! no sandlot job types — so each component depends on the contract, never the
//! other way round:
//!
//! - **apps-middle-ware** authenticates a submission and calls [`validate_intent`];
//! - **Propel** authorizes an admitted intent against its policy;
//! - **Sandlot** derives its own job from an admitted intent and drives it
//!   through [`RunState`] (rechecking the signed admission decision);
//! - the durable data-mesh run record and Sandlot's active store both enforce
//!   transitions via [`RunState::can_transition_to`].
//!
//! Later phases (shadow mode, dogfood, OCI-build migration, per-repo cutover) are
//! separate follow-ups per the FR; this crate is only the contract they build on.

mod admission;
mod intent;
mod state;

pub use admission::{validate_intent, AdmissionConfig, AdmissionError};
pub use intent::{
    ApprovalClass, AutomationIntent, BuildParameters, CiParameters, IntentKind, IntentParameters,
    IntentSource, PolicyContext, ReconcileParameters, RiskClass, SCHEMA_VERSION,
};
pub use state::RunState;
