//! End-to-end execution control tests for the sandbox module.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aivcs_core::role_orchestration::roles::AgentRole;
use aivcs_core::sandbox::capability::ToolCapability;
use aivcs_core::sandbox::engine::evaluate_tool_request;
use aivcs_core::sandbox::error::SandboxError;
use aivcs_core::sandbox::execution::{
    execute_with_controls, CircuitBreaker, SandboxConfig, ToolExecutionResult,
};
use aivcs_core::sandbox::policy::ToolPolicySet;
use aivcs_core::sandbox::request::ToolRequest;

// -------------------------------------------------------------------------
// execute_with_controls tests
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_execute_success_first_attempt() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 2,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(5));

    let result = execute_with_controls(&cfg, &breaker, || async {
        Ok(serde_json::json!({"data": 42}))
    })
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(result.attempts, 1);
    assert_eq!(result.output.unwrap(), serde_json::json!({"data": 42}));
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_execute_retries_then_succeeds() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 3,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(10));
    let counter = Arc::new(AtomicU32::new(0));

    let result = execute_with_controls(&cfg, &breaker, {
        let counter = counter.clone();
        move || {
            let counter = counter.clone();
            async move {
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n < 2 {
                    Err("transient".to_string())
                } else {
                    Ok(serde_json::json!({"recovered": true}))
                }
            }
        }
    })
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(result.attempts, 3);
}

#[tokio::test]
async fn test_execute_exhausted_retries() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 1,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(10));

    let result = execute_with_controls(&cfg, &breaker, || async {
        Err::<serde_json::Value, _>("permanent failure".to_string())
    })
    .await
    .unwrap();

    assert!(!result.success);
    assert_eq!(result.attempts, 2); // 1 initial + 1 retry
    assert!(result.error.unwrap().contains("permanent failure"));
}

#[tokio::test]
async fn test_execute_timeout() {
    let cfg = SandboxConfig {
        timeout_ms: 50,
        max_retries: 0,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(10));

    let result = execute_with_controls(&cfg, &breaker, || async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(serde_json::json!({"never": "returned"}))
    })
    .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        SandboxError::Timeout {
            elapsed_ms,
            limit_ms,
        } => {
            assert_eq!(elapsed_ms, 50);
            assert_eq!(limit_ms, 50);
        }
        other => panic!("expected Timeout, got {:?}", other),
    }
}

#[tokio::test]
async fn test_circuit_breaker_open_blocks_execution() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 0,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(2));
    breaker.record_failure();
    breaker.record_failure();
    assert!(breaker.is_open());

    let result = execute_with_controls(&cfg, &breaker, || async {
        Ok(serde_json::json!({"should": "not run"}))
    })
    .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        SandboxError::CircuitBreakerOpen { .. } => {}
        other => panic!("expected CircuitBreakerOpen, got {:?}", other),
    }
}

#[tokio::test]
async fn test_circuit_breaker_resets_after_success() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 0,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(3));
    breaker.record_failure();
    breaker.record_failure();
    assert!(!breaker.is_open());

    let result = execute_with_controls(&cfg, &breaker, || async {
        Ok(serde_json::json!({"ok": true}))
    })
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(breaker.failure_count(), 0);
}

#[tokio::test]
async fn test_zero_retries_runs_once() {
    let cfg = SandboxConfig {
        timeout_ms: 1000,
        max_retries: 0,
        backoff_base_ms: 10,
    };
    let breaker = Arc::new(CircuitBreaker::new(10));
    let counter = Arc::new(AtomicU32::new(0));

    let result = execute_with_controls(&cfg, &breaker, {
        let counter = counter.clone();
        move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
                Err::<serde_json::Value, _>("fail".to_string())
            }
        }
    })
    .await
    .unwrap();

    assert!(!result.success);
    assert_eq!(result.attempts, 1);
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

// -------------------------------------------------------------------------
// Combined policy + execution test
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_policy_check_then_execute() {
    let policy = ToolPolicySet::standard_dev();
    let request = ToolRequest {
        tool_name: "read_file".into(),
        capability: ToolCapability::FileRead,
        params: serde_json::json!({"path": "/tmp/test.txt"}),
        requesting_role: AgentRole::Reviewer,
    };

    // Step 1: Policy check
    let verdict = evaluate_tool_request(&policy, &request);
    assert!(verdict.is_allowed());

    // Step 2: Execute
    let cfg = SandboxConfig::default();
    let breaker = Arc::new(CircuitBreaker::new(5));
    let result = execute_with_controls(&cfg, &breaker, || async {
        Ok(serde_json::json!({"content": "hello world"}))
    })
    .await
    .unwrap();

    assert!(result.success);
    assert_eq!(
        result.output.unwrap(),
        serde_json::json!({"content": "hello world"})
    );
}

// -------------------------------------------------------------------------
// Serde roundtrip
// -------------------------------------------------------------------------

#[test]
fn test_execution_result_serde_roundtrip() {
    let result = ToolExecutionResult {
        success: true,
        attempts: 2,
        output: Some(serde_json::json!({"data": "ok"})),
        error: None,
    };
    let json = serde_json::to_string(&result).unwrap();
    let back: ToolExecutionResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result, back);

    let fail_result = ToolExecutionResult {
        success: false,
        attempts: 3,
        output: None,
        error: Some("boom".into()),
    };
    let json2 = serde_json::to_string(&fail_result).unwrap();
    let back2: ToolExecutionResult = serde_json::from_str(&json2).unwrap();
    assert_eq!(fail_result, back2);
}
