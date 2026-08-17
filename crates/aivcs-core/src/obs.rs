//! Structured observability hooks for AIVCS run lifecycle events.
//!
//! This module provides:
//! - Run-scoped tracing spans via `RunSpan` RAII guard
//! - Emission functions for key lifecycle events: start, event append, finish, gate evaluation
//!
//! Events are emitted at `info!` level (configurable via `AIVCS_LOG` env var).
//! For JSON output, set `AIVCS_LOG_FORMAT=json`.

use tracing::info;

/// RAII guard that enters a run-scoped tracing span for the duration of a run.
///
/// # Example
///
/// ```
/// use aivcs_core::obs::RunSpan;
/// let _span = RunSpan::enter("run-12345");
/// // Now all tracing calls are automatically associated with run_id = "run-12345"
/// ```
pub struct RunSpan {
    _span: tracing::span::EnteredSpan,
}

impl RunSpan {
    /// Create and enter a span tagged with the run_id.
    pub fn enter(run_id: &str) -> Self {
        let span = tracing::info_span!("aivcs.run", run_id = %run_id);
        Self {
            _span: span.entered(),
        }
    }
}

/// Emit event: run started with agent name.
///
/// # Example
///
/// ```
/// use aivcs_core::obs::emit_run_started;
/// emit_run_started("run-123", "my_agent");
/// // logs: event=run.started run_id=run-123 agent_name=my_agent
/// ```
pub fn emit_run_started(run_id: &str, agent_name: &str) {
    info!(event = "run.started", run_id = %run_id, agent_name = %agent_name);
}

/// Emit event: run finished with duration, total events, and success status.
pub fn emit_run_finished(run_id: &str, duration_ms: u64, total_events: u64, success: bool) {
    info!(
        event = "run.finished",
        run_id = %run_id,
        duration_ms = duration_ms,
        total_events = total_events,
        success = success,
    );
}

/// Emit event: a single event appended to the run.
pub fn emit_event_appended(run_id: &str, event_kind: &str, seq: u64) {
    info!(event = "run.event_appended", run_id = %run_id, kind = %event_kind, seq = seq);
}

/// Emit event: gate evaluation completed with pass rate and verdict.
pub fn emit_gate_evaluated(run_id: &str, pass_rate: f32, passed: bool) {
    info!(
        event = "gate.evaluated",
        run_id = %run_id,
        pass_rate = pass_rate,
        passed = passed,
    );
}

/// Emit event: run finalization error (warning level).
pub fn emit_run_finalize_error(run_id: &str, error: &dyn std::fmt::Display) {
    tracing::warn!(event = "run.finalize_error", run_id = %run_id, error = %error);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_span_create() {
        // Just ensure RunSpan::enter doesn't panic
        let _span = RunSpan::enter("test-run-id");
    }
}
