//! CI signal aggregation across repos.
//!
//! [`CiAggregator`] collects [`CiRunRecord`] statuses from multiple repos and
//! produces a unified [`CiHealthReport`] per logical objective.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use oxidized_state::{CiRunRecord, CiRunStatus};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::multi_repo::error::{MultiRepoError, MultiRepoResult};

/// Aggregated health status of a single repo's CI signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoHealthStatus {
    /// All stages passed.
    Healthy,
    /// Some stages failed; the failing stage names are recorded.
    Degraded { failing_stages: Vec<String> },
    /// The run was cancelled or completely failed.
    Down,
    /// No CI run record found for this repo.
    Unknown,
}

/// CI health details for a single repo within an objective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoHealth {
    /// Repository identifier.
    pub repo_id: String,
    /// Derived health classification.
    pub status: RepoHealthStatus,
    /// Latest known CI run record (if available).
    pub last_run: Option<CiRunRecord>,
}

/// Unified CI health report for a logical objective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiHealthReport {
    /// Name of the logical objective (e.g. `"release-2.0"`, `"pr-142"`).
    pub objective: String,
    /// Per-repo health entries.
    pub repo_health: Vec<RepoHealth>,
    /// Timestamp when the report was generated.
    pub generated_at: chrono::DateTime<Utc>,
    /// `true` only when every repo is [`RepoHealthStatus::Healthy`].
    pub all_healthy: bool,
    /// `repo_id`s with `Down` or `Degraded` status.
    pub unhealthy_repos: Vec<String>,
}

impl CiHealthReport {
    /// Number of repos with `Healthy` status.
    pub fn healthy_count(&self) -> usize {
        self.repo_health
            .iter()
            .filter(|r| r.status == RepoHealthStatus::Healthy)
            .count()
    }

    /// Number of repos with `Degraded` status.
    pub fn degraded_count(&self) -> usize {
        self.repo_health
            .iter()
            .filter(|r| matches!(r.status, RepoHealthStatus::Degraded { .. }))
            .count()
    }

    /// Number of repos with `Down` status.
    pub fn down_count(&self) -> usize {
        self.repo_health
            .iter()
            .filter(|r| r.status == RepoHealthStatus::Down)
            .count()
    }
}

/// Injectable data-source for CI run records.
///
/// Implement this trait to plug in real CI APIs, the `RunLedger`, or test stubs.
#[async_trait]
pub trait CiRunFetcher: Send + Sync {
    /// Fetch the latest CI run record for `repo_id`, or `None` if not found.
    async fn fetch_latest_run(&self, repo_id: &str) -> MultiRepoResult<Option<CiRunRecord>>;
}

/// Collects and aggregates CI run status from multiple repos into a
/// [`CiHealthReport`].
pub struct CiAggregator {
    fetcher: Arc<dyn CiRunFetcher>,
}

impl CiAggregator {
    pub fn new(fetcher: Arc<dyn CiRunFetcher>) -> Self {
        Self { fetcher }
    }

    /// Aggregate CI status for all `repo_ids` under `objective`.
    ///
    /// All `fetch_latest_run` calls are fired concurrently.
    pub async fn aggregate(
        &self,
        objective: &str,
        repo_ids: &[String],
    ) -> MultiRepoResult<CiHealthReport> {
        let objective_owned = objective.to_string();
        let mut join_set = JoinSet::new();
        for (idx, repo_id) in repo_ids.iter().cloned().enumerate() {
            let fetcher = Arc::clone(&self.fetcher);
            let objective = objective_owned.clone();
            join_set.spawn(async move {
                let run = fetcher.fetch_latest_run(&repo_id).await.map_err(|e| {
                    MultiRepoError::AggregationError {
                        objective: objective.clone(),
                        detail: e.to_string(),
                    }
                })?;
                Ok::<(usize, Option<CiRunRecord>), MultiRepoError>((idx, run))
            });
        }

        let mut ordered_runs: Vec<Option<Option<CiRunRecord>>> = vec![None; repo_ids.len()];
        while let Some(joined) = join_set.join_next().await {
            let result = joined.map_err(|e| MultiRepoError::AggregationError {
                objective: objective_owned.clone(),
                detail: format!("ci fetch task join error: {e}"),
            })?;
            let (idx, run) = result?;
            ordered_runs[idx] = Some(run);
        }

        let mut repo_health = Vec::new();
        for (repo_id, run_slot) in repo_ids.iter().zip(ordered_runs) {
            let run = run_slot.ok_or_else(|| MultiRepoError::AggregationError {
                objective: objective_owned.clone(),
                detail: format!("missing ci fetch result for repo '{repo_id}'"),
            })?;
            let status = match &run {
                None => RepoHealthStatus::Unknown,
                Some(r) => Self::classify(r),
            };
            repo_health.push(RepoHealth {
                repo_id: repo_id.clone(),
                status,
                last_run: run,
            });
        }

        let all_healthy = repo_health
            .iter()
            .all(|r| r.status == RepoHealthStatus::Healthy);

        let unhealthy_repos = repo_health
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    RepoHealthStatus::Down | RepoHealthStatus::Degraded { .. }
                )
            })
            .map(|r| r.repo_id.clone())
            .collect();

        Ok(CiHealthReport {
            objective: objective.to_string(),
            repo_health,
            generated_at: Utc::now(),
            all_healthy,
            unhealthy_repos,
        })
    }

    /// Classify a [`CiRunRecord`] into a [`RepoHealthStatus`].
    fn classify(run: &CiRunRecord) -> RepoHealthStatus {
        match run.status {
            CiRunStatus::Succeeded => RepoHealthStatus::Healthy,
            CiRunStatus::Failed => {
                let failing: Vec<String> = run
                    .step_results
                    .iter()
                    .filter(|s| matches!(s.status, CiRunStatus::Failed))
                    .map(|s| s.step_name.clone())
                    .collect();
                if failing.is_empty() {
                    RepoHealthStatus::Down
                } else {
                    RepoHealthStatus::Degraded {
                        failing_stages: failing,
                    }
                }
            }
            CiRunStatus::Cancelled => RepoHealthStatus::Down,
            CiRunStatus::Running | CiRunStatus::Queued => RepoHealthStatus::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxidized_state::{CiRunRecord, CiRunStatus};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::time::{sleep, Duration};

    /// Stub fetcher backed by an in-memory map of repo_id → CiRunRecord.
    struct MockFetcher {
        runs: Mutex<HashMap<String, CiRunRecord>>,
    }

    impl MockFetcher {
        fn with(runs: Vec<(String, CiRunRecord)>) -> Arc<Self> {
            Arc::new(Self {
                runs: Mutex::new(runs.into_iter().collect()),
            })
        }
    }

    #[async_trait]
    impl CiRunFetcher for MockFetcher {
        async fn fetch_latest_run(&self, repo_id: &str) -> MultiRepoResult<Option<CiRunRecord>> {
            Ok(self.runs.lock().unwrap().get(repo_id).cloned())
        }
    }

    fn succeeded_run(repo_id: &str) -> CiRunRecord {
        let mut r = CiRunRecord::queued(repo_id, "pipe-1");
        r.status = CiRunStatus::Succeeded;
        r
    }

    fn failed_run(repo_id: &str, failing_step: &str) -> CiRunRecord {
        use oxidized_state::CiStepResult;
        let mut r = CiRunRecord::queued(repo_id, "pipe-1");
        r.status = CiRunStatus::Failed;
        r.step_results.push(CiStepResult {
            step_name: failing_step.to_string(),
            status: CiRunStatus::Failed,
            exit_code: Some(1),
            started_at: None,
            finished_at: None,
            stdout_digest: None,
            stderr_digest: None,
        });
        r
    }

    #[tokio::test]
    async fn test_aggregate_all_healthy() {
        let fetcher = MockFetcher::with(vec![
            ("org/a".to_string(), succeeded_run("snap-a")),
            ("org/b".to_string(), succeeded_run("snap-b")),
        ]);
        let agg = CiAggregator::new(fetcher);
        let report = agg
            .aggregate("release-1.0", &["org/a".to_string(), "org/b".to_string()])
            .await
            .unwrap();
        assert!(report.all_healthy);
        assert!(report.unhealthy_repos.is_empty());
        assert_eq!(report.healthy_count(), 2);
    }

    #[tokio::test]
    async fn test_aggregate_degraded_repo_appears_in_unhealthy_list() {
        let fetcher = MockFetcher::with(vec![
            ("org/a".to_string(), succeeded_run("snap-a")),
            ("org/b".to_string(), failed_run("snap-b", "clippy")),
        ]);
        let agg = CiAggregator::new(fetcher);
        let report = agg
            .aggregate("pr-42", &["org/a".to_string(), "org/b".to_string()])
            .await
            .unwrap();
        assert!(!report.all_healthy);
        assert!(report.unhealthy_repos.contains(&"org/b".to_string()));
        assert_eq!(report.degraded_count(), 1);
    }

    #[tokio::test]
    async fn test_aggregate_unknown_when_no_run() {
        let fetcher = MockFetcher::with(vec![]); // no data
        let agg = CiAggregator::new(fetcher);
        let report = agg
            .aggregate("empty-obj", &["org/x".to_string()])
            .await
            .unwrap();
        assert!(!report.all_healthy);
        assert_eq!(report.repo_health[0].status, RepoHealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_health_report_counts_are_correct() {
        use oxidized_state::CiRunRecord;
        let mut cancelled = CiRunRecord::queued("snap-c", "pipe-1");
        cancelled.status = CiRunStatus::Cancelled;

        let fetcher = MockFetcher::with(vec![
            ("org/a".to_string(), succeeded_run("snap-a")),
            ("org/b".to_string(), failed_run("snap-b", "test")),
            ("org/c".to_string(), cancelled),
        ]);
        let agg = CiAggregator::new(fetcher);
        let report = agg
            .aggregate(
                "mixed",
                &[
                    "org/a".to_string(),
                    "org/b".to_string(),
                    "org/c".to_string(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(report.healthy_count(), 1);
        assert_eq!(report.degraded_count(), 1);
        assert_eq!(report.down_count(), 1);
    }

    struct SlowFetcher {
        run: CiRunRecord,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    impl SlowFetcher {
        fn new(run: CiRunRecord) -> Arc<Self> {
            Arc::new(Self {
                run,
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl CiRunFetcher for SlowFetcher {
        async fn fetch_latest_run(&self, _repo_id: &str) -> MultiRepoResult<Option<CiRunRecord>> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            loop {
                let current_max = self.max_in_flight.load(Ordering::SeqCst);
                if now <= current_max {
                    break;
                }
                if self
                    .max_in_flight
                    .compare_exchange(current_max, now, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(Some(self.run.clone()))
        }
    }

    #[tokio::test]
    async fn test_aggregate_fetches_repos_concurrently() {
        let fetcher = SlowFetcher::new(succeeded_run("snap-concurrent"));
        let agg = CiAggregator::new(fetcher.clone());

        let repos = vec![
            "org/a".to_string(),
            "org/b".to_string(),
            "org/c".to_string(),
            "org/d".to_string(),
        ];
        let report = agg.aggregate("concurrency", &repos).await.unwrap();

        assert_eq!(report.repo_health.len(), 4);
        assert!(
            fetcher.max_in_flight.load(Ordering::SeqCst) > 1,
            "expected concurrent fetches, max_in_flight={}",
            fetcher.max_in_flight.load(Ordering::SeqCst)
        );
    }
}
