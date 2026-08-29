//! CI pipeline orchestration and run recording.

use crate::runner::{CiRunner, StageResult};
use crate::spec::CiSpec;
use crate::stage::StageConfig;
use aivcs_core::domain::run::{Event, EventKind};
use aivcs_core::recording::GraphRunRecorder;
use oxidized_state::{ContentDigest, RunLedger, RunMetadata, RunSummary};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;
use uuid::Uuid;

/// Result of a complete CI pipeline execution.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Run ID from AIVCS.
    pub run_id: String,

    /// Whether all stages passed.
    pub success: bool,

    /// Results of individual stages.
    pub stages: Vec<StageResult>,

    /// Total duration in milliseconds.
    pub duration_ms: u64,

    /// Digest of the CI specification.
    pub spec_digest: String,
}

impl PipelineResult {
    /// Number of stages that passed.
    pub fn passed_count(&self) -> usize {
        self.stages.iter().filter(|s| s.passed()).count()
    }

    /// Number of stages that failed.
    pub fn failed_count(&self) -> usize {
        self.stages.iter().filter(|s| !s.passed()).count()
    }
}

/// CI pipeline orchestrator.
pub struct CiPipeline;

impl CiPipeline {
    /// Execute a CI pipeline and record all events into AIVCS.
    ///
    /// Each enabled stage produces:
    /// - One `ToolCalled` event on start
    /// - One `ToolReturned` event on success or `ToolFailed` event on failure
    ///
    /// The run is finalized as either Completed (if all stages passed) or Failed.
    pub async fn run(
        ledger: Arc<dyn RunLedger>,
        ci_spec: &CiSpec,
        stages: Vec<StageConfig>,
    ) -> anyhow::Result<PipelineResult> {
        let start = Instant::now();

        // Create AgentSpec and get digest
        let agent_spec = ci_spec.to_agent_spec()?;
        let spec_digest = ContentDigest::from_bytes(agent_spec.spec_digest.as_bytes());

        // Start recording run
        let metadata = RunMetadata {
            git_sha: Some(ci_spec.git_sha.clone()),
            agent_name: format!(
                "cargo-ci-{}",
                spec_digest
                    .as_str()
                    .get(..12)
                    .unwrap_or(spec_digest.as_str())
            ),
            tags: json!({
                "stages": stages.iter().map(|s| &s.name).collect::<Vec<_>>(),
                "workspace": ci_spec.workspace_path.to_string_lossy(),
                "toolchain": &ci_spec.toolchain_hash,
            }),
            evaluation: Default::default(),
        };

        let recorder = GraphRunRecorder::start(ledger.clone(), &spec_digest, metadata).await?;
        let run_id = recorder.run_id().to_string();

        info!(run_id = %run_id, "Starting CI pipeline");

        let mut stage_results = Vec::new();
        let mut seq = 1u64;
        let mut all_passed = true;

        // Execute each enabled stage
        for config in stages {
            if !config.enabled {
                info!(stage = %config.name, "Skipping disabled stage");
                continue;
            }

            info!(stage = %config.name, "Executing stage");

            // Record ToolCalled event
            let tool_name = config.name.clone();
            let called_event = Event::new(
                Uuid::new_v4(),
                seq,
                EventKind::ToolCalled {
                    tool_name: tool_name.clone(),
                },
                json!({
                    "command": &config.command,
                    "timeout_secs": config.timeout_secs,
                }),
            );
            recorder.record(&called_event).await?;
            seq += 1;

            // Execute stage — catch errors so we can record a ToolFailed event
            let stage_start = Instant::now();
            let result = match CiRunner::execute_stage(&config).await {
                Ok(r) => r,
                Err(e) => {
                    // Stage execution itself failed (e.g. timeout, spawn error).
                    // Record a ToolFailed event so the gate sees it.
                    all_passed = false;
                    let duration_ms_stage = stage_start.elapsed().as_millis() as u64;
                    let failed_event = Event::new(
                        Uuid::new_v4(),
                        seq,
                        EventKind::ToolFailed {
                            tool_name: tool_name.clone(),
                        },
                        json!({
                            "exit_code": -1,
                            "stdout": "",
                            "stderr": e.to_string(),
                            "duration_ms": duration_ms_stage,
                            "error": format!("Stage '{}' execution error: {}", tool_name, e),
                        }),
                    );
                    recorder.record(&failed_event).await?;
                    seq += 1;

                    stage_results.push(StageResult {
                        stage_name: tool_name,
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        duration_ms: duration_ms_stage,
                        success: false,
                    });
                    continue;
                }
            };

            // Record result event
            if result.passed() {
                let returned_event = Event::new(
                    Uuid::new_v4(),
                    seq,
                    EventKind::ToolReturned {
                        tool_name: tool_name.clone(),
                    },
                    json!({
                        "exit_code": result.exit_code,
                        "stdout": &result.stdout,
                        "stderr": &result.stderr,
                        "duration_ms": result.duration_ms,
                    }),
                );
                recorder.record(&returned_event).await?;
            } else {
                all_passed = false;
                let failed_event = Event::new(
                    Uuid::new_v4(),
                    seq,
                    EventKind::ToolFailed {
                        tool_name: tool_name.clone(),
                    },
                    json!({
                        "exit_code": result.exit_code,
                        "stdout": &result.stdout,
                        "stderr": &result.stderr,
                        "duration_ms": result.duration_ms,
                        "error": format!("Stage '{}' exited with code {}", tool_name, result.exit_code),
                    }),
                );
                recorder.record(&failed_event).await?;
            }
            seq += 1;

            stage_results.push(result);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        // Finalize run
        let summary = RunSummary {
            total_events: seq - 1,
            final_state_digest: None,
            duration_ms,
            success: all_passed,
        };

        if all_passed {
            recorder.finish_ok(summary).await?;
            info!(run_id = %run_id, "CI pipeline completed successfully");
        } else {
            recorder.finish_err(summary).await?;
            info!(run_id = %run_id, "CI pipeline failed");
        }

        Ok(PipelineResult {
            run_id,
            success: all_passed,
            stages: stage_results,
            duration_ms,
            spec_digest: spec_digest.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_result_counts() {
        let result = PipelineResult {
            run_id: "run123".to_string(),
            success: true,
            stages: vec![
                StageResult {
                    stage_name: "fmt".to_string(),
                    exit_code: 0,
                    stdout: "".to_string(),
                    stderr: "".to_string(),
                    duration_ms: 100,
                    success: true,
                },
                StageResult {
                    stage_name: "check".to_string(),
                    exit_code: 0,
                    stdout: "".to_string(),
                    stderr: "".to_string(),
                    duration_ms: 200,
                    success: true,
                },
            ],
            duration_ms: 300,
            spec_digest: "abc123".to_string(),
        };

        assert_eq!(result.passed_count(), 2);
        assert_eq!(result.failed_count(), 0);
        assert!(result.success);
    }

    #[test]
    fn test_pipeline_result_with_failures() {
        let result = PipelineResult {
            run_id: "run123".to_string(),
            success: false,
            stages: vec![
                StageResult {
                    stage_name: "fmt".to_string(),
                    exit_code: 0,
                    stdout: "".to_string(),
                    stderr: "".to_string(),
                    duration_ms: 100,
                    success: true,
                },
                StageResult {
                    stage_name: "check".to_string(),
                    exit_code: 1,
                    stdout: "".to_string(),
                    stderr: "error".to_string(),
                    duration_ms: 200,
                    success: false,
                },
            ],
            duration_ms: 300,
            spec_digest: "abc123".to_string(),
        };

        assert_eq!(result.passed_count(), 1);
        assert_eq!(result.failed_count(), 1);
        assert!(!result.success);
    }
}
