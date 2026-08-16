//! CI run results and stage outcomes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a CI run or stage.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CIStatus {
    Pending,
    Running,
    Passed,
    Failed,
    Cancelled,
}

/// Result of a single CI stage execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CIStageResult {
    /// Stage name (e.g. "fmt", "clippy", "test").
    pub stage: String,

    /// Command that was executed.
    pub command: String,

    /// Stage outcome.
    pub status: CIStatus,

    /// Execution duration in milliseconds.
    pub duration_ms: u64,

    /// Whether the result came from cache.
    pub cache_hit: bool,

    /// Number of diagnostics produced by this stage.
    pub diagnostics_count: u32,
}

impl CIStageResult {
    /// Create a new stage result.
    pub fn new(stage: String, command: String, status: CIStatus, duration_ms: u64) -> Self {
        Self {
            stage,
            command,
            status,
            duration_ms,
            cache_hit: false,
            diagnostics_count: 0,
        }
    }
}

/// Aggregate result of an entire CI run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CIResult {
    /// Run ID this result belongs to.
    pub run_id: Uuid,

    /// Overall run status.
    pub overall_status: CIStatus,

    /// Per-stage results in execution order.
    pub stages: Vec<CIStageResult>,

    /// When execution started.
    pub started_at: DateTime<Utc>,

    /// When execution finished.
    pub finished_at: Option<DateTime<Utc>>,

    /// Total wall-clock duration in milliseconds.
    pub total_duration_ms: u64,

    /// Number of stages that passed.
    pub passed: u32,

    /// Number of stages that failed.
    pub failed: u32,
}

impl CIResult {
    /// Create a new CI result from stage results.
    ///
    /// `finished_at` should be the actual completion time; pass `None` if the
    /// run is still in progress.
    pub fn new(
        run_id: Uuid,
        stages: Vec<CIStageResult>,
        started_at: DateTime<Utc>,
        finished_at: Option<DateTime<Utc>>,
    ) -> Self {
        let passed = stages
            .iter()
            .filter(|s| s.status == CIStatus::Passed)
            .count() as u32;
        let failed = stages
            .iter()
            .filter(|s| s.status == CIStatus::Failed)
            .count() as u32;
        let total_duration_ms = finished_at
            .as_ref()
            .map(|finished| {
                let delta_ms = finished
                    .signed_duration_since(started_at)
                    .num_milliseconds();
                delta_ms.max(0) as u64
            })
            .unwrap_or(0);
        let overall_status = if failed > 0 {
            CIStatus::Failed
        } else {
            CIStatus::Passed
        };

        Self {
            run_id,
            overall_status,
            stages,
            started_at,
            finished_at,
            total_duration_ms,
            passed,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ci_status_serde() {
        let statuses = [
            CIStatus::Pending,
            CIStatus::Running,
            CIStatus::Passed,
            CIStatus::Failed,
            CIStatus::Cancelled,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deserialized: CIStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*status, deserialized);
        }
        assert_eq!(
            serde_json::to_string(&CIStatus::Passed).expect("serialize"),
            "\"passed\""
        );
    }

    #[test]
    fn test_ci_stage_result_serde_roundtrip() {
        let result = CIStageResult::new(
            "clippy".to_string(),
            "cargo clippy --workspace".to_string(),
            CIStatus::Passed,
            4500,
        );

        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: CIStageResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, deserialized);
    }

    #[test]
    fn test_ci_stage_result_defaults() {
        let result = CIStageResult::new(
            "fmt".to_string(),
            "cargo fmt --check".to_string(),
            CIStatus::Passed,
            200,
        );
        assert!(!result.cache_hit);
        assert_eq!(result.diagnostics_count, 0);
    }

    #[test]
    fn test_ci_result_computes_aggregates() {
        let run_id = Uuid::new_v4();
        let stages = vec![
            CIStageResult::new("fmt".into(), "cargo fmt".into(), CIStatus::Passed, 100),
            CIStageResult::new(
                "clippy".into(),
                "cargo clippy".into(),
                CIStatus::Failed,
                500,
            ),
            CIStageResult::new("test".into(), "cargo test".into(), CIStatus::Passed, 2000),
        ];

        let started_at = Utc::now();
        let finished_at = started_at + chrono::Duration::milliseconds(2600);
        let result = CIResult::new(run_id, stages, started_at, Some(finished_at));

        assert_eq!(result.passed, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.overall_status, CIStatus::Failed);
        assert_eq!(result.total_duration_ms, 2600);
    }

    #[test]
    fn test_ci_result_all_pass() {
        let run_id = Uuid::new_v4();
        let stages = vec![
            CIStageResult::new("fmt".into(), "cargo fmt".into(), CIStatus::Passed, 100),
            CIStageResult::new("test".into(), "cargo test".into(), CIStatus::Passed, 1000),
        ];

        let started_at = Utc::now();
        let finished_at = started_at + chrono::Duration::milliseconds(1100);
        let result = CIResult::new(run_id, stages, started_at, Some(finished_at));

        assert_eq!(result.overall_status, CIStatus::Passed);
        assert_eq!(result.passed, 2);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn test_ci_result_serde_roundtrip() {
        let run_id = Uuid::new_v4();
        let stages = vec![CIStageResult::new(
            "test".into(),
            "cargo test".into(),
            CIStatus::Passed,
            3000,
        )];
        let started_at = Utc::now();
        let finished_at = started_at + chrono::Duration::milliseconds(3000);
        let result = CIResult::new(run_id, stages, started_at, Some(finished_at));

        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: CIResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, deserialized);
    }
}
