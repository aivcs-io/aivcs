//! State-safe parallel role execution.
//!
//! Each role is assigned an isolated [`RunId`] in the shared [`RunLedger`], so
//! writes from role A can never overwrite role B's events. The caller injects
//! a `role_executor` async closure for testability; in production this closure
//! dispatches to the real agent runtime.

use std::future::Future;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{instrument, warn};

use oxidized_state::storage_traits::{ContentDigest, RunId, RunLedger, RunMetadata, RunSummary};

use crate::role_orchestration::{
    error::{RoleError, RoleResult},
    roles::{AgentRole, HandoffToken, RoleOutput},
};

/// The outcome of a single role run.
#[derive(Debug, Clone)]
pub struct RoleRunResult {
    pub role: AgentRole,
    /// The isolated `RunId` created for this role's ledger entries.
    pub run_id: RunId,
    pub output: RoleOutput,
    pub success: bool,
}

/// Configuration for a parallel role execution batch.
#[derive(Debug, Clone)]
pub struct ParallelRoleConfig {
    /// Maximum number of concurrent role tasks.
    pub max_concurrent: usize,
    /// Abort remaining tasks as soon as one fails.
    pub fail_fast: bool,
}

impl Default for ParallelRoleConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            fail_fast: false,
        }
    }
}

/// Execute `roles` in parallel, each in an isolated ledger namespace.
///
/// `role_executor` is an async closure `(AgentRole, RunId) -> RoleResult<RoleOutput>`.
/// Inject a deterministic stub in tests; wire to the real agent runtime in production.
///
/// Each role gets its own [`RunId`] scoped with `parent_run_id` as a tag, so
/// cross-role ledger contamination is structurally impossible.
///
/// Returns `Err(RoleError::ParallelExecutionFailed)` only when *every* role fails.
/// Individual role failures are recorded in `RoleRunResult::success = false`.
#[instrument(skip(ledger, role_executor), fields(parent_run_id = %parent_run_id))]
pub async fn execute_roles_parallel<F, Fut>(
    ledger: Arc<dyn RunLedger>,
    parent_run_id: &str,
    roles: Vec<AgentRole>,
    spec_digest: &ContentDigest,
    config: ParallelRoleConfig,
    role_executor: F,
) -> RoleResult<Vec<RoleRunResult>>
where
    F: Fn(AgentRole, RunId) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = RoleResult<RoleOutput>> + Send,
{
    let executor = Arc::new(role_executor);
    let results: Arc<Mutex<Vec<RoleRunResult>>> = Arc::new(Mutex::new(Vec::new()));
    let (fail_tx, _fail_rx_guard) = tokio::sync::watch::channel(false);
    let fail_flag = Arc::new(fail_tx);

    // Semaphore enforces max_concurrent
    let sem = Arc::new(tokio::sync::Semaphore::new(config.max_concurrent));

    let mut tasks = Vec::new();

    for role in roles {
        let ledger = Arc::clone(&ledger);
        let spec_digest = spec_digest.clone();
        let executor = Arc::clone(&executor);
        let results = Arc::clone(&results);
        let fail_flag = Arc::clone(&fail_flag);
        let fail_rx = fail_flag.subscribe();
        let config = config.clone();
        let parent_id = parent_run_id.to_string();
        let sem = Arc::clone(&sem);

        let task = tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    warn!(role = %role, error = %e, "failed to acquire semaphore permit");
                    return;
                }
            };

            // Abort early if fail_fast was triggered by a sibling.
            if *fail_rx.borrow() {
                return;
            }

            let metadata = RunMetadata {
                git_sha: None,
                agent_name: format!("role:{role}"),
                tags: serde_json::json!({
                    "parent_run_id": parent_id,
                    "role": role.to_string(),
                }),
                evaluation: Default::default(),
            };

            let run_id = match ledger.create_run(&spec_digest, metadata).await {
                Ok(id) => id,
                Err(e) => {
                    warn!(role = %role, error = %e, "failed to create run for role");
                    if config.fail_fast {
                        let _ = fail_flag.send(true);
                    }
                    results.lock().await.push(RoleRunResult {
                        role,
                        run_id: RunId::new(),
                        output: RoleOutput::Fix {
                            patch_digest: String::new(),
                            resolved_issues: vec![format!("failed to create run: {e}")],
                        },
                        success: false,
                    });
                    return;
                }
            };

            match executor(role.clone(), run_id.clone()).await {
                Ok(output) => {
                    let _ = ledger
                        .complete_run(
                            &run_id,
                            RunSummary {
                                total_events: 1,
                                final_state_digest: None,
                                duration_ms: 0,
                                success: true,
                            },
                        )
                        .await;

                    results.lock().await.push(RoleRunResult {
                        role,
                        run_id,
                        output,
                        success: true,
                    });
                }
                Err(e) => {
                    warn!(role = %role, error = %e, "role execution failed");
                    let _ = ledger
                        .fail_run(
                            &run_id,
                            RunSummary {
                                total_events: 0,
                                final_state_digest: None,
                                duration_ms: 0,
                                success: false,
                            },
                        )
                        .await;

                    if config.fail_fast {
                        let _ = fail_flag.send(true);
                    }

                    results.lock().await.push(RoleRunResult {
                        role,
                        run_id,
                        output: RoleOutput::Fix {
                            patch_digest: String::new(),
                            resolved_issues: vec![e.to_string()],
                        },
                        success: false,
                    });
                }
            }
        });

        tasks.push(task);
    }

    for task in tasks {
        let _ = task.await;
    }

    let guard = results.lock().await;
    let results_vec: Vec<RoleRunResult> = guard.clone();
    drop(guard);

    if !results_vec.is_empty() && results_vec.iter().all(|r| !r.success) {
        return Err(RoleError::ParallelExecutionFailed {
            detail: "all parallel role runs failed".to_string(),
        });
    }

    Ok(results_vec)
}

/// Convert a [`RoleRunResult`] into a [`HandoffToken`].
///
/// Returns [`RoleError::Domain`] if the output cannot be serialised.
pub fn token_from_result(result: RoleRunResult) -> RoleResult<HandoffToken> {
    HandoffToken::new(result.output).map_err(RoleError::Domain)
}
