//! Verification and self-healing orchestration primitives.
//!
//! This module provides:
//! - failure taxonomy classification
//! - bounded auto-repair loop decisions
//! - auditable recovery artifacts with digest verification

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{AivcsError, Result};
use oxidized_state::storage_traits::ContentDigest;

/// Coarse failure taxonomy used by the recovery planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Build,
    Test,
    Runtime,
    Integration,
    Unknown,
}

/// Structured failure signal from verification/runtime stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureSignal {
    pub stage: String,
    pub message: String,
    pub exit_code: Option<i32>,
    pub flaky_hint: bool,
}

impl FailureSignal {
    pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
            exit_code: None,
            flaky_hint: false,
        }
    }
}

/// Recovery action selected per attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Retry,
    PatchForward,
    Rollback,
    Escalate,
}

/// Recovery loop final state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcome {
    Recovered,
    Failed,
}

/// Bounded recovery policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    pub max_attempts: u32,
    pub max_flaky_retries: u32,
    pub allow_patch_forward: bool,
    pub allow_rollback: bool,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            max_flaky_retries: 1,
            allow_patch_forward: true,
            allow_rollback: true,
        }
    }
}

/// One auditable decision in the recovery timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub attempt: u32,
    pub failure_class: FailureClass,
    pub action: RecoveryAction,
    pub rationale: String,
}

/// Result from one repair attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAttemptResult {
    pub success: bool,
    pub next_failure: Option<FailureSignal>,
}

/// Full recovery log for artifacts/audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryLog {
    pub run_id: String,
    pub policy: RecoveryPolicy,
    pub initial_failure: FailureSignal,
    pub decisions: Vec<RecoveryDecision>,
    pub outcome: RecoveryOutcome,
    pub attempts_used: u32,
    pub final_failure: Option<FailureSignal>,
    pub evaluated_at: DateTime<Utc>,
}

/// Classify a failure into a coarse category.
pub fn classify_failure(signal: &FailureSignal) -> FailureClass {
    let stage = signal.stage.to_lowercase();
    let msg = signal.message.to_lowercase();

    if stage.contains("build")
        || stage.contains("compile")
        || msg.contains("compil")
        || msg.contains("linker error")
    {
        return FailureClass::Build;
    }
    if stage.contains("test")
        || msg.contains("assertion")
        || msg.contains("test failed")
        || msg.contains("snapshot mismatch")
    {
        return FailureClass::Test;
    }
    if stage.contains("runtime")
        || msg.contains("panic")
        || msg.contains("segmentation fault")
        || msg.contains("null pointer")
    {
        return FailureClass::Runtime;
    }
    if stage.contains("integration")
        || msg.contains("contract")
        || msg.contains("handshake")
        || msg.contains("dependency unavailable")
    {
        return FailureClass::Integration;
    }

    FailureClass::Unknown
}

fn decide_action(
    class: FailureClass,
    signal: &FailureSignal,
    policy: &RecoveryPolicy,
    flaky_retries_used: u32,
) -> (RecoveryAction, String) {
    if class == FailureClass::Test
        && signal.flaky_hint
        && flaky_retries_used < policy.max_flaky_retries
    {
        return (
            RecoveryAction::Retry,
            "flaky signal detected; bounded retry permitted".to_string(),
        );
    }

    if policy.allow_patch_forward && matches!(class, FailureClass::Build | FailureClass::Test) {
        return (
            RecoveryAction::PatchForward,
            "build/test failure; patch-forward is enabled".to_string(),
        );
    }

    if policy.allow_rollback && matches!(class, FailureClass::Runtime | FailureClass::Integration) {
        return (
            RecoveryAction::Rollback,
            "runtime/integration failure; rollback is enabled".to_string(),
        );
    }

    (
        RecoveryAction::Escalate,
        "no safe automated action available under policy".to_string(),
    )
}

/// Run a bounded, policy-controlled recovery loop.
pub fn execute_recovery_loop<F>(
    run_id: &str,
    initial_failure: FailureSignal,
    policy: RecoveryPolicy,
    mut apply_action: F,
) -> RecoveryLog
where
    F: FnMut(u32, RecoveryAction, &FailureSignal) -> RecoveryAttemptResult,
{
    let mut current = initial_failure.clone();
    let mut decisions = Vec::new();
    let mut flaky_retries_used = 0u32;
    let mut attempts_used = 0u32;

    for attempt in 1..=policy.max_attempts {
        attempts_used = attempt;
        let class = classify_failure(&current);
        let (action, rationale) = decide_action(class, &current, &policy, flaky_retries_used);
        if action == RecoveryAction::Retry && current.flaky_hint {
            flaky_retries_used += 1;
        }

        decisions.push(RecoveryDecision {
            attempt,
            failure_class: class,
            action,
            rationale,
        });

        if action == RecoveryAction::Escalate {
            return RecoveryLog {
                run_id: run_id.to_string(),
                policy,
                initial_failure,
                decisions,
                outcome: RecoveryOutcome::Failed,
                attempts_used,
                final_failure: Some(current),
                evaluated_at: Utc::now(),
            };
        }

        let attempt_result = apply_action(attempt, action, &current);
        if attempt_result.success {
            return RecoveryLog {
                run_id: run_id.to_string(),
                policy,
                initial_failure,
                decisions,
                outcome: RecoveryOutcome::Recovered,
                attempts_used,
                final_failure: None,
                evaluated_at: Utc::now(),
            };
        }

        if let Some(next) = attempt_result.next_failure {
            current = next;
        }
    }

    RecoveryLog {
        run_id: run_id.to_string(),
        policy,
        initial_failure,
        decisions,
        outcome: RecoveryOutcome::Failed,
        attempts_used,
        final_failure: Some(current),
        evaluated_at: Utc::now(),
    }
}

/// Persist `<dir>/<run_id>/recovery.json` and `<dir>/<run_id>/recovery.digest`.
pub fn write_recovery_artifact(log: &RecoveryLog, dir: &Path) -> Result<PathBuf> {
    let run_dir = dir.join(&log.run_id);
    std::fs::create_dir_all(&run_dir)?;

    let artifact_path = run_dir.join("recovery.json");
    let digest_path = run_dir.join("recovery.digest");
    let json = serde_json::to_vec_pretty(log)?;
    let digest = ContentDigest::from_bytes(&json).as_str().to_string();

    std::fs::write(&artifact_path, &json)?;
    std::fs::write(&digest_path, digest.as_bytes())?;

    Ok(artifact_path)
}

/// Read and verify `<dir>/<run_id>/recovery.json` integrity.
pub fn read_recovery_artifact(run_id: &str, dir: &Path) -> Result<RecoveryLog> {
    let run_dir = dir.join(run_id);
    let artifact_path = run_dir.join("recovery.json");
    let digest_path = run_dir.join("recovery.digest");

    let json = std::fs::read(&artifact_path)?;
    let digest = std::fs::read_to_string(&digest_path)?;
    let actual = ContentDigest::from_bytes(&json).as_str().to_string();
    if digest.trim() != actual {
        return Err(AivcsError::DigestMismatch {
            expected: digest.trim().to_string(),
            actual,
        });
    }

    Ok(serde_json::from_slice(&json)?)
}
