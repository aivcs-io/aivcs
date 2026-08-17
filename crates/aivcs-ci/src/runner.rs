//! CI stage execution and event recording.

use crate::stage::StageConfig;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

/// Result of a stage execution.
#[derive(Debug, Clone)]
pub struct StageResult {
    /// Stage name.
    pub stage_name: String,

    /// Exit code (0 = success).
    pub exit_code: i32,

    /// Captured stdout.
    pub stdout: String,

    /// Captured stderr.
    pub stderr: String,

    /// Duration in milliseconds.
    pub duration_ms: u64,

    /// Whether execution succeeded.
    pub success: bool,
}

impl StageResult {
    /// Whether this stage passed (exit code 0).
    pub fn passed(&self) -> bool {
        self.success && self.exit_code == 0
    }
}

/// CI stage runner that executes a stage and records events.
pub struct CiRunner;

impl CiRunner {
    /// Execute a single stage and return the result.
    ///
    /// Records two events:
    /// - `ToolCalled` when stage starts
    /// - `ToolReturned` (success) or event with error info (failure) when stage completes
    pub async fn execute_stage(config: &StageConfig) -> anyhow::Result<StageResult> {
        let start = Instant::now();

        // Validate command
        if config.command.is_empty() {
            anyhow::bail!("Stage {} has empty command", config.name);
        }

        let exe = &config.command[0];
        let args = &config.command[1..];

        // Execute with timeout
        let child = Command::new(exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let output = if config.timeout_secs > 0 {
            tokio::time::timeout(
                std::time::Duration::from_secs(config.timeout_secs),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Stage {} timed out after {} seconds",
                    config.name,
                    config.timeout_secs
                )
            })??
        } else {
            child.wait_with_output().await?
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let success = output.status.success();

        Ok(StageResult {
            stage_name: config.name.clone(),
            exit_code,
            stdout,
            stderr,
            duration_ms,
            success,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_result_passed() {
        let result = StageResult {
            stage_name: "cargo_fmt".to_string(),
            exit_code: 0,
            stdout: "".to_string(),
            stderr: "".to_string(),
            duration_ms: 100,
            success: true,
        };
        assert!(result.passed());
    }

    #[test]
    fn test_stage_result_failed() {
        let result = StageResult {
            stage_name: "cargo_fmt".to_string(),
            exit_code: 1,
            stdout: "".to_string(),
            stderr: "error".to_string(),
            duration_ms: 100,
            success: false,
        };
        assert!(!result.passed());
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let config = StageConfig::custom(
            "echo_test".to_string(),
            vec!["echo".to_string(), "hello".to_string()],
            60,
        );

        let result = CiRunner::execute_stage(&config)
            .await
            .expect("execute failed");
        assert!(result.success);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_failing_command() {
        let config = StageConfig::custom("false_test".to_string(), vec!["false".to_string()], 60);

        let result = CiRunner::execute_stage(&config)
            .await
            .expect("execute failed");
        assert!(!result.success);
        assert_ne!(result.exit_code, 0);
    }
}
