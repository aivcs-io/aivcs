//! Deploy-by-digest with registry lookup.
//!
//! Looks up the current release for an agent, delegates to
//! [`DeployByDigestRunner`] for deterministic run creation, and returns
//! a [`ReplaySummary`] whose digest can be compared across identical
//! invocations (golden equality).

use chrono::{DateTime, Utc};
use oxidized_state::storage_traits::{ReleaseRegistry, RunId, RunLedger};

use crate::deploy_runner::DeployByDigestRunner;
use crate::domain::{AivcsError, Result};
use crate::replay::{replay_run, ReplaySummary};

/// Result of a deploy-by-digest invocation.
#[derive(Debug, Clone)]
pub struct DeployResult {
    /// The run ID created for this deployment.
    pub run_id: RunId,
    /// The spec digest from the current release.
    pub spec_digest: String,
    /// Replay summary with golden digest.
    pub summary: ReplaySummary,
}

/// Deploy an agent by looking up its current release digest.
///
/// Resolves the agent's current release via the registry, then delegates
/// to [`DeployByDigestRunner`] to create a deterministic run. Finally
/// replays the run to produce a [`ReplaySummary`] suitable for golden
/// equality testing.
///
/// Pass a fixed `timestamp` to get deterministic digests across invocations.
pub async fn deploy_by_digest(
    registry: &dyn ReleaseRegistry,
    ledger: &dyn RunLedger,
    agent_name: &str,
    timestamp: Option<DateTime<Utc>>,
) -> Result<DeployResult> {
    // 1. Look up the current release
    let release = registry
        .current(agent_name)
        .await
        .map_err(|e| AivcsError::StorageError(e.to_string()))?
        .ok_or_else(|| {
            AivcsError::ReleaseConflict(format!("no current release for agent '{}'", agent_name))
        })?;

    let spec_digest_str = release.spec_digest.as_str().to_string();

    // 2. Delegate to DeployByDigestRunner
    let output = match timestamp {
        Some(ts) => {
            DeployByDigestRunner::run_at(ledger, &release.spec_digest, agent_name, ts).await?
        }
        None => DeployByDigestRunner::run(ledger, &release.spec_digest, agent_name).await?,
    };

    // 3. Replay to get the golden digest
    let (_events, replay_summary) = replay_run(ledger, &output.run_id.0).await?;

    Ok(DeployResult {
        run_id: output.run_id,
        spec_digest: spec_digest_str,
        summary: replay_summary,
    })
}
