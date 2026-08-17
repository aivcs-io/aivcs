//! Integration tests for CI pipeline with MemoryRunLedger.

use aivcs_ci::{CiGate, CiPipeline, CiSpec, StageConfig};
use oxidized_state::fakes::MemoryRunLedger;
use oxidized_state::{RunId, RunLedger};
use std::path::PathBuf;
use std::sync::Arc;

/// Test: successful pipeline execution (fmt + check both pass)
#[tokio::test]
async fn test_successful_pipeline() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let stages = vec![
        StageConfig::custom(
            "echo_test".to_string(),
            vec!["echo".to_string(), "hello".to_string()],
            60,
        ),
        StageConfig::custom(
            "echo_test2".to_string(),
            vec!["echo".to_string(), "world".to_string()],
            60,
        ),
    ];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["echo_test".to_string(), "echo_test2".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline failed");

    // Check results
    assert!(result.success, "Pipeline should succeed");
    assert_eq!(result.passed_count(), 2, "Both stages should pass");
    assert_eq!(result.failed_count(), 0, "No stages should fail");
    assert!(!result.run_id.is_empty(), "Run ID should be set");

    // Verify run was recorded
    let run_id = RunId(result.run_id);
    let run: oxidized_state::RunRecord = ledger.get_run(&run_id).await.expect("Failed to get run");
    assert!(run.summary.is_some(), "Run should have summary");
    let summary = run.summary.unwrap();
    assert!(summary.success, "Summary should mark success");
    assert_eq!(
        summary.total_events, 4,
        "Should have 4 events (2 tool_called + 2 tool_returned)"
    );
}

/// Test: failed stage captured with error info
#[tokio::test]
async fn test_failed_stage_captured() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let stages = vec![StageConfig::custom(
        "false_test".to_string(),
        vec!["false".to_string()],
        60,
    )];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["false_test".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline failed");

    // Check results
    assert!(!result.success, "Pipeline should fail");
    assert_eq!(result.passed_count(), 0, "No stages should pass");
    assert_eq!(result.failed_count(), 1, "One stage should fail");

    // Verify run was recorded as failed
    let run_id = RunId(result.run_id);
    let run: oxidized_state::RunRecord = ledger.get_run(&run_id).await.expect("Failed to get run");
    assert!(run.summary.is_some(), "Run should have summary");
    let summary = run.summary.unwrap();
    assert!(!summary.success, "Summary should mark failure");

    // Verify events include tool_failed
    let events: Vec<oxidized_state::RunEvent> = ledger
        .get_events(&run_id)
        .await
        .expect("Failed to get events");

    let has_tool_failed = events.iter().any(|e| e.kind == "tool_failed");
    assert!(has_tool_failed, "Should have tool_failed event");
}

/// Test: gate evaluation detects failures
#[tokio::test]
async fn test_gate_evaluation_with_failure() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let stages = vec![StageConfig::custom(
        "fail_test".to_string(),
        vec!["false".to_string()],
        60,
    )];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["fail_test".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline failed");

    // Get events and evaluate gate
    let run_id = RunId(result.run_id);
    let events: Vec<oxidized_state::RunEvent> = ledger
        .get_events(&run_id)
        .await
        .expect("Failed to get events");

    let verdict = CiGate::evaluate(&events);
    assert!(!verdict.passed, "Gate should fail for failed stages");
    assert!(!verdict.violations.is_empty(), "Should have violations");
}

/// Test: disabled stage is skipped
#[tokio::test]
async fn test_disabled_stage_skipped() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let stages = vec![
        StageConfig::custom(
            "echo_test".to_string(),
            vec!["echo".to_string(), "hello".to_string()],
            60,
        ),
        StageConfig::custom("skip_me".to_string(), vec!["false".to_string()], 60).disabled(),
    ];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["echo_test".to_string(), "skip_me".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline failed");

    // Check that only enabled stage was executed
    assert!(
        result.success,
        "Pipeline should succeed (disabled stage not run)"
    );
    assert_eq!(result.stages.len(), 1, "Only one stage should be executed");
    assert_eq!(result.passed_count(), 1, "One stage should pass");

    // Verify run only has events for enabled stage
    let run_id = oxidized_state::RunId(result.run_id);
    let events = ledger
        .get_events(&run_id)
        .await
        .expect("Failed to get events");

    // Should have 2 events: one tool_called + one tool_returned
    assert_eq!(
        events.len(),
        2,
        "Should have 2 events (disabled stage not run)"
    );
}

/// Test: gate passes for all successful stages
#[tokio::test]
async fn test_gate_passes_for_success() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let stages = vec![
        StageConfig::custom(
            "test1".to_string(),
            vec!["echo".to_string(), "pass1".to_string()],
            60,
        ),
        StageConfig::custom(
            "test2".to_string(),
            vec!["echo".to_string(), "pass2".to_string()],
            60,
        ),
    ];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["test1".to_string(), "test2".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline failed");

    // Get events and evaluate gate
    let run_id = RunId(result.run_id);
    let events: Vec<oxidized_state::RunEvent> = ledger
        .get_events(&run_id)
        .await
        .expect("Failed to get events");

    let verdict = CiGate::evaluate(&events);
    assert!(verdict.passed, "Gate should pass for all successful stages");
    assert!(verdict.violations.is_empty(), "Should have no violations");
}

/// Test: stage execution error (e.g. spawn failure) is recorded as ToolFailed and pipeline continues.
/// CRR PR#166 follow-up: ensures error path records events and StageResult with exit_code = -1.
#[tokio::test]
async fn test_pipeline_execution_error_recorded_as_tool_failed() {
    let ledger = Arc::new(MemoryRunLedger::new());

    // Use a non-existent executable so execute_stage returns Err (spawn failure)
    let stages = vec![StageConfig::custom(
        "exec_error_stage".to_string(),
        vec!["/nonexistent-binary-that-does-not-exist".to_string()],
        5,
    )];

    let ci_spec = CiSpec::new(
        PathBuf::from("."),
        &["exec_error_stage".to_string()],
        "abc123".to_string(),
        "rustc_hash".to_string(),
    );

    let result = CiPipeline::run(ledger.clone(), &ci_spec, stages)
        .await
        .expect("pipeline run should not fail");

    assert!(!result.success, "Pipeline should report failure");
    assert_eq!(result.stages.len(), 1, "One stage should be recorded");
    let stage = &result.stages[0];
    assert_eq!(
        stage.exit_code, -1,
        "Execution error should use exit_code -1"
    );
    assert!(!stage.success, "Stage should be marked failed");

    let run_id = RunId(result.run_id);
    let events = ledger
        .get_events(&run_id)
        .await
        .expect("Failed to get events");

    assert_eq!(events.len(), 2, "Should have tool_called + tool_failed");
    assert_eq!(events[0].kind, "tool_called");
    assert_eq!(events[1].kind, "tool_failed");
    assert_eq!(
        events[1].payload["exit_code"].as_i64(),
        Some(-1),
        "tool_failed event should have exit_code -1"
    );
}
