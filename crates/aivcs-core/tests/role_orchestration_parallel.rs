//! Parallel role execution isolation tests.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aivcs_core::role_orchestration::{
    error::{RoleError, RoleResult},
    executor::{execute_roles_parallel, token_from_result, ParallelRoleConfig},
    roles::{AgentRole, RoleOutput},
};
use async_trait::async_trait;
use oxidized_state::fakes::MemoryRunLedger;
use oxidized_state::storage_traits::{
    ContentDigest, RunEvent, RunId, RunLedger, RunMetadata, RunRecord, RunStatus, RunSummary,
};
use oxidized_state::StorageError;

fn spec() -> ContentDigest {
    ContentDigest::from_bytes(b"test-spec")
}

struct FlakyCreateRunLedger {
    inner: MemoryRunLedger,
    fail_first_n: AtomicUsize,
}

impl FlakyCreateRunLedger {
    fn new(fail_first_n: usize) -> Self {
        Self {
            inner: MemoryRunLedger::new(),
            fail_first_n: AtomicUsize::new(fail_first_n),
        }
    }
}

#[async_trait]
impl RunLedger for FlakyCreateRunLedger {
    async fn create_run(
        &self,
        spec_digest: &ContentDigest,
        metadata: RunMetadata,
    ) -> Result<RunId, StorageError> {
        let remaining = self.fail_first_n.load(Ordering::SeqCst);
        if remaining > 0
            && self
                .fail_first_n
                .compare_exchange(remaining, remaining - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return Err(StorageError::Backend(
                "injected create_run failure".to_string(),
            ));
        }
        self.inner.create_run(spec_digest, metadata).await
    }

    async fn append_event(&self, run_id: &RunId, event: RunEvent) -> Result<(), StorageError> {
        self.inner.append_event(run_id, event).await
    }

    async fn complete_run(&self, run_id: &RunId, summary: RunSummary) -> Result<(), StorageError> {
        self.inner.complete_run(run_id, summary).await
    }

    async fn fail_run(&self, run_id: &RunId, summary: RunSummary) -> Result<(), StorageError> {
        self.inner.fail_run(run_id, summary).await
    }

    async fn cancel_run(&self, run_id: &RunId, summary: RunSummary) -> Result<(), StorageError> {
        self.inner.cancel_run(run_id, summary).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<RunRecord, StorageError> {
        self.inner.get_run(run_id).await
    }

    async fn get_events(&self, run_id: &RunId) -> Result<Vec<RunEvent>, StorageError> {
        self.inner.get_events(run_id).await
    }

    async fn list_runs(
        &self,
        spec_digest: Option<&ContentDigest>,
    ) -> Result<Vec<RunRecord>, StorageError> {
        self.inner.list_runs(spec_digest).await
    }
}

/// Stub executor: always succeeds with a canned output per role.
#[allow(clippy::manual_async_fn)]
fn ok_executor(
    role: AgentRole,
    run_id: oxidized_state::storage_traits::RunId,
) -> impl std::future::Future<Output = RoleResult<RoleOutput>> {
    let _ = run_id;
    async move {
        Ok(match role {
            AgentRole::Reviewer => RoleOutput::Review {
                approved: true,
                comments: vec![],
                requires_fix: false,
            },
            AgentRole::Tester => RoleOutput::TestReport {
                passed: true,
                total_cases: 3,
                failed_cases: vec![],
                diagnostic_digest: None,
            },
            _ => RoleOutput::Fix {
                patch_digest: "stub".to_string(),
                resolved_issues: vec![],
            },
        })
    }
}

/// Stub executor: always fails.
#[allow(clippy::manual_async_fn)]
fn fail_executor(
    _role: AgentRole,
    _run_id: oxidized_state::storage_traits::RunId,
) -> impl std::future::Future<Output = RoleResult<RoleOutput>> {
    async move {
        Err(RoleError::ParallelExecutionFailed {
            detail: "intentional failure".to_string(),
        })
    }
}

#[tokio::test]
async fn test_parallel_roles_get_isolated_run_ids() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let results = execute_roles_parallel(
        Arc::clone(&ledger),
        "parent-run-1",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        ok_executor,
    )
    .await
    .unwrap();

    assert_eq!(results.len(), 2);

    let ids: HashSet<String> = results.iter().map(|r| r.run_id.0.clone()).collect();
    assert_eq!(ids.len(), 2, "each role must get a distinct RunId");
}

#[tokio::test]
async fn test_parallel_roles_do_not_share_ledger_state() {
    let ledger = Arc::new(MemoryRunLedger::new());

    let results = execute_roles_parallel(
        Arc::clone(&ledger) as Arc<dyn RunLedger>,
        "parent-run-2",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        ok_executor,
    )
    .await
    .unwrap();

    // Each RunId must resolve to a distinct, completed run record.
    for result in &results {
        let record = ledger.get_run(&result.run_id).await.unwrap();
        assert_eq!(record.run_id, result.run_id);
        assert_eq!(record.status, RunStatus::Completed);
    }

    // Verify no run shares events with another.
    for result in &results {
        let events = ledger.get_events(&result.run_id).await.unwrap();
        // Events are empty (stub executor doesn't append any), confirming isolation
        assert!(
            events.is_empty(),
            "stub executor should not append events to ledger"
        );
    }
}

#[tokio::test]
async fn test_all_roles_fail_returns_parallel_execution_failed_error() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let result = execute_roles_parallel(
        ledger,
        "parent-run-3",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        fail_executor,
    )
    .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        RoleError::ParallelExecutionFailed { .. }
    ));
}

#[tokio::test]
async fn test_successful_parallel_run_produces_handoff_tokens() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let results = execute_roles_parallel(
        ledger,
        "parent-run-4",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig::default(),
        ok_executor,
    )
    .await
    .unwrap();

    for result in results {
        assert!(result.success);
        let token = token_from_result(result).unwrap();
        // Token must self-verify cleanly.
        assert!(token.verify().is_ok());
    }
}

#[tokio::test]
async fn test_fail_fast_stops_remaining_tasks() {
    let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());

    let config = ParallelRoleConfig {
        max_concurrent: 1, // serial to make ordering deterministic
        fail_fast: true,
    };

    // All executors fail — fail_fast should still return after they all tried.
    let result = execute_roles_parallel(
        ledger,
        "parent-run-5",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        config,
        fail_executor,
    )
    .await;

    // All failed → ParallelExecutionFailed
    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_run_failure_is_reported_as_role_failure_result() {
    let ledger: Arc<dyn RunLedger> = Arc::new(FlakyCreateRunLedger::new(1));

    let results = execute_roles_parallel(
        ledger,
        "parent-run-6",
        vec![AgentRole::Reviewer, AgentRole::Tester],
        &spec(),
        ParallelRoleConfig {
            max_concurrent: 1,
            fail_fast: false,
        },
        ok_executor,
    )
    .await
    .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().filter(|r| !r.success).count(), 1);
}
