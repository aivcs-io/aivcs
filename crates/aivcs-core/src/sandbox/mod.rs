//! Sandbox: policy-controlled tool execution for autonomous agents.
//!
//! Provides a default-deny, first-match-wins policy engine that determines
//! whether a given role may invoke a particular tool capability.  Execution
//! controls (timeout, retry with backoff, circuit breaker) wrap the actual
//! tool invocation so that misbehaving tools cannot monopolize resources.
//!
//! # Modules
//!
//! - [`capability`] — `ToolCapability` enum (Shell, FileRead, …)
//! - [`request`]    — `ToolRequest` + `PolicyVerdict`
//! - [`policy`]     — `ToolPolicyRule`, `ToolPolicySet`, `standard_dev()`
//! - [`engine`]     — `evaluate_tool_request()` (first-match, default-deny)
//! - [`execution`]  — `SandboxConfig`, `CircuitBreaker`, `execute_with_controls()`
//! - [`error`]      — `SandboxError` / `SandboxResult`

pub mod capability;
pub mod engine;
pub mod error;
pub mod execution;
pub mod policy;
pub mod request;

pub use capability::ToolCapability;
pub use engine::evaluate_tool_request;
pub use error::{SandboxError, SandboxResult};
pub use execution::{execute_with_controls, CircuitBreaker, SandboxConfig, ToolExecutionResult};
pub use policy::{ToolPolicyRule, ToolPolicySet};
pub use request::{PolicyVerdict, ToolRequest};
