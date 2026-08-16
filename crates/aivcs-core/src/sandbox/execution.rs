//! Execution controls: timeout, retry with exponential backoff, circuit breaker.

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::{SandboxError, SandboxResult};

/// Configuration for sandboxed tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxConfig {
    /// Maximum wall-clock time for a single attempt (milliseconds).
    pub timeout_ms: u64,
    /// Maximum number of retries (0 = no retries, run once).
    pub max_retries: u32,
    /// Base delay for exponential backoff between retries (milliseconds).
    pub backoff_base_ms: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_retries: 2,
            backoff_base_ms: 500,
        }
    }
}

/// Atomic circuit breaker that opens after N consecutive failures.
///
/// Thread-safe via `AtomicU32`. Resets on success.
#[derive(Debug)]
pub struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    threshold: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given failure threshold.
    pub fn new(threshold: u32) -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            threshold,
        }
    }

    /// Returns `true` if the breaker is open (too many consecutive failures).
    pub fn is_open(&self) -> bool {
        self.consecutive_failures.load(Ordering::Relaxed) >= self.threshold
    }

    /// Record a failure. Returns current consecutive failure count.
    pub fn record_failure(&self) -> u32 {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Reset on success.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Current consecutive failure count.
    pub fn failure_count(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

/// The result of a tool execution attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolExecutionResult {
    /// Whether the tool succeeded.
    pub success: bool,
    /// Number of attempts made (1 = no retries used).
    pub attempts: u32,
    /// Tool output payload (present on success).
    pub output: Option<serde_json::Value>,
    /// Error message (present on failure).
    pub error: Option<String>,
}

/// Execute a tool with timeout, retry, and circuit-breaker controls.
///
/// `tool_fn` is an async closure that performs the actual tool work and returns
/// `Ok(serde_json::Value)` on success or `Err(String)` on failure.
///
/// The circuit breaker is checked before each attempt and updated after.
pub async fn execute_with_controls<F, Fut>(
    config: &SandboxConfig,
    breaker: &Arc<CircuitBreaker>,
    tool_fn: F,
) -> SandboxResult<ToolExecutionResult>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<serde_json::Value, String>>,
{
    let max_attempts = config.max_retries + 1;

    for attempt in 1..=max_attempts {
        // Check circuit breaker
        if breaker.is_open() {
            return Err(SandboxError::CircuitBreakerOpen {
                consecutive_failures: breaker.failure_count(),
                threshold: breaker.threshold,
            });
        }

        let timeout = Duration::from_millis(config.timeout_ms);
        let result = tokio::time::timeout(timeout, tool_fn()).await;

        match result {
            Ok(Ok(value)) => {
                breaker.record_success();
                return Ok(ToolExecutionResult {
                    success: true,
                    attempts: attempt,
                    output: Some(value),
                    error: None,
                });
            }
            Ok(Err(err_msg)) => {
                breaker.record_failure();
                if attempt == max_attempts {
                    return Ok(ToolExecutionResult {
                        success: false,
                        attempts: attempt,
                        output: None,
                        error: Some(err_msg),
                    });
                }
                // Exponential backoff before retry
                let delay = Duration::from_millis(config.backoff_base_ms * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }
            Err(_elapsed) => {
                breaker.record_failure();
                if attempt == max_attempts {
                    return Err(SandboxError::Timeout {
                        elapsed_ms: config.timeout_ms,
                        limit_ms: config.timeout_ms,
                    });
                }
                let delay = Duration::from_millis(config.backoff_base_ms * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }
        }
    }

    // Unreachable, but satisfy the compiler.
    Err(SandboxError::ExecutionFailed {
        attempts: max_attempts,
        reason: "exhausted all attempts".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(3);
        assert!(!cb.is_open());
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_opens_at_threshold() {
        let cb = CircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open());
        cb.record_failure();
        assert!(cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let cb = CircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_sandbox_config_default() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.timeout_ms, 30_000);
        assert_eq!(cfg.max_retries, 2);
        assert_eq!(cfg.backoff_base_ms, 500);
    }

    #[test]
    fn test_sandbox_config_serde_roundtrip() {
        let cfg = SandboxConfig {
            timeout_ms: 5000,
            max_retries: 1,
            backoff_base_ms: 100,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SandboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[tokio::test]
    async fn test_execute_success_on_first_attempt() {
        let cfg = SandboxConfig {
            timeout_ms: 1000,
            max_retries: 2,
            backoff_base_ms: 10,
        };
        let breaker = Arc::new(CircuitBreaker::new(5));

        let result = execute_with_controls(&cfg, &breaker, || async {
            Ok(serde_json::json!({"ok": true}))
        })
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(result.attempts, 1);
        assert!(result.output.is_some());
    }

    #[tokio::test]
    async fn test_execute_retries_then_succeeds() {
        let cfg = SandboxConfig {
            timeout_ms: 1000,
            max_retries: 2,
            backoff_base_ms: 10,
        };
        let breaker = Arc::new(CircuitBreaker::new(5));
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = execute_with_controls(&cfg, &breaker, move || {
            let c = counter_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::Relaxed);
                if n < 2 {
                    Err("not yet".into())
                } else {
                    Ok(serde_json::json!({"ok": true}))
                }
            }
        })
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(result.attempts, 3);
    }

    #[tokio::test]
    async fn test_execute_exhausts_retries() {
        let cfg = SandboxConfig {
            timeout_ms: 1000,
            max_retries: 1,
            backoff_base_ms: 10,
        };
        let breaker = Arc::new(CircuitBreaker::new(10));

        let result = execute_with_controls(&cfg, &breaker, || async {
            Err::<serde_json::Value, _>("always fails".to_string())
        })
        .await
        .unwrap();

        assert!(!result.success);
        assert_eq!(result.attempts, 2);
        assert!(result.error.unwrap().contains("always fails"));
    }

    #[tokio::test]
    async fn test_execute_circuit_breaker_blocks() {
        let cfg = SandboxConfig {
            timeout_ms: 1000,
            max_retries: 0,
            backoff_base_ms: 10,
        };
        let breaker = Arc::new(CircuitBreaker::new(1));
        breaker.record_failure(); // open the breaker

        let result = execute_with_controls(&cfg, &breaker, || async {
            Ok(serde_json::json!({"ok": true}))
        })
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::CircuitBreakerOpen { .. } => {}
            other => panic!("expected CircuitBreakerOpen, got {:?}", other),
        }
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
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(serde_json::json!({"ok": true}))
        })
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::Timeout { .. } => {}
            other => panic!("expected Timeout, got {:?}", other),
        }
    }
}
