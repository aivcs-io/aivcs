//! Agent Role Orchestration — EPIC2.
//!
//! Enables multi-agent collaboration (planner / coder / reviewer / tester / fixer)
//! with explicit, content-addressed contracts between roles.
//!
//! # Module layout
//!
//! - [`roles`] — `AgentRole`, `RoleOutput`, `HandoffToken`, `RoleTemplate`
//! - [`error`] — `RoleError`, `RoleResult`
//! - [`router`] — `validate_handoff_sequence`, `build_execution_plan`, `ExecutionPlan`
//! - [`merge`] — `merge_parallel_outputs`, `MergedRoleOutput`, `RoleConflict`
//! - [`executor`] — `execute_roles_parallel`, `ParallelRoleConfig`, `RoleRunResult`

pub mod error;
pub mod executor;
pub mod merge;
pub mod roles;
pub mod router;
