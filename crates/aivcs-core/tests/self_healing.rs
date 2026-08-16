use aivcs_core::{
    classify_failure, execute_recovery_loop, read_recovery_artifact, write_recovery_artifact,
    FailureClass, FailureSignal, RecoveryAction, RecoveryAttemptResult, RecoveryOutcome,
    RecoveryPolicy,
};
use tempfile::tempdir;

#[test]
fn failure_taxonomy_maps_common_failures() {
    let build = FailureSignal::new("build", "compilation failed: unresolved import");
    let test = FailureSignal::new("test", "assertion failed at suite");
    let runtime = FailureSignal::new("runtime", "thread panicked at src/main.rs:1");
    let integration = FailureSignal::new("integration", "contract handshake failed");
    let unknown = FailureSignal::new("misc", "unexpected condition");

    assert_eq!(classify_failure(&build), FailureClass::Build);
    assert_eq!(classify_failure(&test), FailureClass::Test);
    assert_eq!(classify_failure(&runtime), FailureClass::Runtime);
    assert_eq!(classify_failure(&integration), FailureClass::Integration);
    assert_eq!(classify_failure(&unknown), FailureClass::Unknown);
}

#[test]
fn bounded_loop_auto_remediates_common_build_failure() {
    let failure = FailureSignal::new("build", "compile error");
    let policy = RecoveryPolicy::default();

    let log = execute_recovery_loop("run-build", failure, policy, |attempt, action, _| {
        if attempt == 1 {
            assert_eq!(action, RecoveryAction::PatchForward);
            return RecoveryAttemptResult {
                success: true,
                next_failure: None,
            };
        }
        panic!("should recover on first patch-forward");
    });

    assert_eq!(log.outcome, RecoveryOutcome::Recovered);
    assert_eq!(log.attempts_used, 1);
    assert_eq!(log.decisions.len(), 1);
}

#[test]
fn flaky_retry_obeys_safety_bounds_then_escalates() {
    let mut flaky = FailureSignal::new("test", "intermittent timeout");
    flaky.flaky_hint = true;

    let policy = RecoveryPolicy {
        max_attempts: 3,
        max_flaky_retries: 1,
        allow_patch_forward: false,
        allow_rollback: false,
    };

    let log = execute_recovery_loop("run-flaky", flaky.clone(), policy, |_attempt, action, _| {
        assert_eq!(action, RecoveryAction::Retry);
        RecoveryAttemptResult {
            success: false,
            next_failure: Some(flaky.clone()),
        }
    });

    assert_eq!(log.outcome, RecoveryOutcome::Failed);
    assert_eq!(log.attempts_used, 2);
    assert_eq!(log.decisions[0].action, RecoveryAction::Retry);
    assert_eq!(log.decisions[1].action, RecoveryAction::Escalate);
}

#[test]
fn max_attempt_bound_is_enforced() {
    let failure = FailureSignal::new("build", "compile error");
    let policy = RecoveryPolicy {
        max_attempts: 2,
        max_flaky_retries: 0,
        allow_patch_forward: true,
        allow_rollback: false,
    };

    let mut calls = 0u32;
    let log = execute_recovery_loop(
        "run-bounded",
        failure.clone(),
        policy,
        |_attempt, action, _| {
            calls += 1;
            assert_eq!(action, RecoveryAction::PatchForward);
            RecoveryAttemptResult {
                success: false,
                next_failure: Some(failure.clone()),
            }
        },
    );

    assert_eq!(calls, 2);
    assert_eq!(log.outcome, RecoveryOutcome::Failed);
    assert_eq!(log.attempts_used, 2);
}

#[test]
fn recovery_artifact_is_auditable_and_digest_verified() {
    let failure = FailureSignal::new("build", "compile error");
    let log = execute_recovery_loop(
        "run-artifact",
        failure,
        RecoveryPolicy::default(),
        |_attempt, _action, _| RecoveryAttemptResult {
            success: true,
            next_failure: None,
        },
    );

    let dir = tempdir().expect("tempdir");
    let path = write_recovery_artifact(&log, dir.path()).expect("write artifact");
    assert!(path.exists());

    let loaded = read_recovery_artifact("run-artifact", dir.path()).expect("read artifact");
    assert_eq!(loaded.outcome, RecoveryOutcome::Recovered);
    assert_eq!(loaded.decisions.len(), 1);
}
