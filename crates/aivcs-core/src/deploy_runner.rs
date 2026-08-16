//! Deploy-by-digest reference runner.
//!
//! Runs an agent by `AgentSpec` digest through `RunLedger` and emits a minimal,
//! deterministic event sequence for replay/golden validation.

use chrono::{DateTime, Utc};
use oxidized_state::{
    ContentDigest, RunEvent, RunId, RunLedger, RunMetadata, RunSummary, StorageError,
};
use std::time::Instant;

use crate::domain::{AivcsError, Result};

/// Output of a deploy-by-digest run.
#[derive(Debug, Clone)]
pub struct DeployRunOutput {
    pub run_id: RunId,
    pub emitted_events: usize,
}

/// Reference runner for deploy-by-digest execution.
pub struct DeployByDigestRunner;

impl DeployByDigestRunner {
    /// Run deploy-by-digest using current UTC time for event timestamps.
    pub async fn run(
        ledger: &dyn RunLedger,
        spec_digest: &ContentDigest,
        agent_name: &str,
    ) -> Result<DeployRunOutput> {
        let now = Utc::now();
        Self::run_at(ledger, spec_digest, agent_name, now).await
    }

    /// Run deploy-by-digest at a fixed timestamp (used for deterministic tests).
    pub async fn run_at(
        ledger: &dyn RunLedger,
        spec_digest: &ContentDigest,
        agent_name: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<DeployRunOutput> {
        let started = Instant::now();
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: agent_name.to_string(),
            tags: serde_json::json!({
                "mode": "deploy_by_digest",
            }),
            evaluation: Default::default(),
        };

        let run_id = ledger
            .create_run(spec_digest, metadata)
            .await
            .map_err(storage_err)?;

        let events = vec![
            RunEvent {
                seq: 1,
                kind: "deploy_started".to_string(),
                payload: serde_json::json!({
                    "spec_digest": spec_digest.as_str(),
                }),
                timestamp,
            },
            RunEvent {
                seq: 2,
                kind: "agent_executed".to_string(),
                payload: serde_json::json!({
                    "agent_name": agent_name,
                    "spec_digest": spec_digest.as_str(),
                }),
                timestamp,
            },
            RunEvent {
                seq: 3,
                kind: "deploy_completed".to_string(),
                payload: serde_json::json!({
                    "success": true,
                }),
                timestamp,
            },
        ];

        for event in events {
            ledger
                .append_event(&run_id, event)
                .await
                .map_err(storage_err)?;
        }

        let summary = RunSummary {
            total_events: 3,
            // This reference runner emits lifecycle events only; it does not materialize
            // a final state blob, so no final-state digest is available.
            final_state_digest: None,
            duration_ms: started.elapsed().as_millis() as u64,
            success: true,
        };
        ledger
            .complete_run(&run_id, summary)
            .await
            .map_err(storage_err)?;

        Ok(DeployRunOutput {
            run_id,
            emitted_events: 3,
        })
    }
}

fn storage_err(e: StorageError) -> AivcsError {
    AivcsError::StorageError(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxidized_state::fakes::MemoryRunLedger;
    use oxidized_state::RunStatus;

    #[tokio::test]
    async fn deploy_run_records_spec_digest_and_completes() {
        let ledger = MemoryRunLedger::new();
        let digest = ContentDigest::from_bytes(b"agent-spec-v1");

        let output = DeployByDigestRunner::run(&ledger, &digest, "agent-alpha")
            .await
            .expect("deploy run");

        assert_eq!(output.emitted_events, 3);
        let run = ledger.get_run(&output.run_id).await.expect("get run");
        assert_eq!(run.spec_digest, digest);
        assert_eq!(run.status, RunStatus::Completed);
        let summary = run.summary.expect("summary");
        assert_eq!(summary.total_events, 3);
        assert!(summary.final_state_digest.is_none());
    }
}
