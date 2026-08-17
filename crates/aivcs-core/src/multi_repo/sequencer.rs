//! Coordinated cross-repo PR and release sequencing.
//!
//! [`ReleaseSequencer`] takes a [`RepoDependencyGraph`] and a set of pending
//! releases, then executes them in topological order — blocking downstream
//! repos until upstream ones succeed or skipping them on failure.

use std::collections::HashSet;
use std::sync::Arc;

use oxidized_state::storage_traits::{ContentDigest, RunEvent, RunLedger, RunMetadata, RunSummary};
use serde::{Deserialize, Serialize};

use crate::multi_repo::error::{MultiRepoError, MultiRepoResult};
use crate::multi_repo::graph::RepoDependencyGraph;
use crate::recording::GraphRunRecorder;

/// Status of a single repo's release within a sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoReleaseStatus {
    Pending,
    InProgress,
    Succeeded {
        run_id: String,
    },
    Failed {
        reason: String,
    },
    /// Skipped because an upstream dependency failed.
    Skipped,
}

/// A single item in a cross-repo release sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceItem {
    pub repo_id: String,
    pub version_label: String,
    pub spec_digest: String,
    pub promoted_by: String,
    pub status: RepoReleaseStatus,
}

/// An ordered cross-repo release plan.
#[derive(Debug, Clone)]
pub struct SequencePlan {
    /// Stable plan identifier.
    pub plan_id: String,
    /// Items ordered by dependency (topological sort).
    pub items: Vec<SequenceItem>,
    /// Links back to the triggering run.
    pub originating_run_id: String,
}

/// Outcome of executing a [`SequencePlan`].
#[derive(Debug, Clone)]
pub struct SequenceOutcome {
    pub plan_id: String,
    /// Repo IDs that succeeded.
    pub succeeded: Vec<String>,
    /// Repo IDs that failed and their reasons.
    pub failed: Vec<(String, String)>,
    /// Repo IDs that were skipped (upstream failed).
    pub skipped: Vec<String>,
}

impl SequenceOutcome {
    /// `true` when no failures and no unexpected skips occurred.
    pub fn overall_success(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Trait for the per-repo release execution backend.
///
/// Inject a real implementation that calls CI/CD APIs, or a stub for tests.
#[async_trait::async_trait]
pub trait RepoReleaser: Send + Sync {
    /// Promote `repo_id` to `version_label`, returning a run identifier.
    async fn release(
        &self,
        repo_id: &str,
        version_label: &str,
        spec_digest: &str,
        promoted_by: &str,
    ) -> MultiRepoResult<String>;
}

/// Orchestrates cross-repo release sequencing using a [`RepoDependencyGraph`]
/// to enforce topological ordering.
pub struct ReleaseSequencer {
    graph: RepoDependencyGraph,
    ledger: Arc<dyn RunLedger>,
}

impl ReleaseSequencer {
    pub fn new(graph: RepoDependencyGraph, ledger: Arc<dyn RunLedger>) -> Self {
        Self { graph, ledger }
    }

    /// Build a topologically-ordered [`SequencePlan`] from a list of release
    /// descriptors `(repo_id, version_label, spec_digest, promoted_by)`.
    ///
    /// Repos that are in the graph but not in `releases` are included as
    /// `Skipped` items when they sit on the dependency path between provided
    /// releases (to preserve ordering invariants).
    pub fn build_plan(
        &self,
        releases: Vec<(String, String, String, String)>,
        originating_run_id: &str,
    ) -> MultiRepoResult<SequencePlan> {
        let topo = self.graph.topological_order()?;
        let release_map: std::collections::HashMap<String, (String, String, String)> = releases
            .into_iter()
            .map(|(repo, ver, digest, by)| (repo, (ver, digest, by)))
            .collect();

        let plan_id = format!(
            "seq-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("x")
        );

        let items = topo
            .into_iter()
            .map(|node| {
                if let Some((version_label, spec_digest, promoted_by)) =
                    release_map.get(&node.repo_id)
                {
                    SequenceItem {
                        repo_id: node.repo_id,
                        version_label: version_label.clone(),
                        spec_digest: spec_digest.clone(),
                        promoted_by: promoted_by.clone(),
                        status: RepoReleaseStatus::Pending,
                    }
                } else {
                    SequenceItem {
                        repo_id: node.repo_id.clone(),
                        version_label: "skipped".to_string(),
                        spec_digest: String::new(),
                        promoted_by: String::new(),
                        status: RepoReleaseStatus::Skipped,
                    }
                }
            })
            .collect();

        Ok(SequencePlan {
            plan_id,
            items,
            originating_run_id: originating_run_id.to_string(),
        })
    }

    /// Execute a [`SequencePlan`] using `releaser` as the per-repo backend.
    ///
    /// Steps are executed in plan order. When a repo fails, all transitive
    /// dependents are marked `Skipped` automatically.
    pub async fn execute_plan(
        &self,
        mut plan: SequencePlan,
        releaser: &dyn RepoReleaser,
    ) -> MultiRepoResult<SequenceOutcome> {
        let spec =
            ContentDigest::from_bytes(format!("sequence:{}", plan.originating_run_id).as_bytes());
        let metadata = RunMetadata {
            git_sha: None,
            agent_name: "release-sequencer".to_string(),
            tags: serde_json::json!({ "plan_id": plan.plan_id }),
            evaluation: Default::default(),
        };

        let recorder = GraphRunRecorder::start(Arc::clone(&self.ledger), &spec, metadata)
            .await
            .map_err(|e| MultiRepoError::Storage(e.to_string()))?;

        let mut succeeded = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut skipped = Vec::new();
        let mut skip_set: HashSet<String> = HashSet::new();
        let mut seq: u64 = 1;

        for item in &mut plan.items {
            // Pre-skip repos marked as Skipped in the plan (no release entry)
            // or whose upstream failed.
            if matches!(item.status, RepoReleaseStatus::Skipped) || skip_set.contains(&item.repo_id)
            {
                item.status = RepoReleaseStatus::Skipped;
                skipped.push(item.repo_id.clone());
                continue;
            }

            // Record start.
            let start_event = RunEvent {
                seq,
                kind: "NodeEntered".to_string(),
                payload: serde_json::json!({ "node_id": item.repo_id, "version": item.version_label }),
                timestamp: chrono::Utc::now(),
            };
            self.ledger
                .append_event(recorder.run_id(), start_event)
                .await
                .map_err(|e| MultiRepoError::Storage(e.to_string()))?;
            seq += 1;

            item.status = RepoReleaseStatus::InProgress;

            match releaser
                .release(
                    &item.repo_id,
                    &item.version_label,
                    &item.spec_digest,
                    &item.promoted_by,
                )
                .await
            {
                Ok(run_id) => {
                    item.status = RepoReleaseStatus::Succeeded {
                        run_id: run_id.clone(),
                    };
                    succeeded.push(item.repo_id.clone());

                    let end_event = RunEvent {
                        seq,
                        kind: "NodeExited".to_string(),
                        payload: serde_json::json!({ "node_id": item.repo_id, "run_id": run_id }),
                        timestamp: chrono::Utc::now(),
                    };
                    self.ledger
                        .append_event(recorder.run_id(), end_event)
                        .await
                        .map_err(|e| MultiRepoError::Storage(e.to_string()))?;
                    seq += 1;
                }
                Err(e) => {
                    let reason = e.to_string();
                    item.status = RepoReleaseStatus::Failed {
                        reason: reason.clone(),
                    };
                    failed.push((item.repo_id.clone(), reason.clone()));

                    let fail_event = RunEvent {
                        seq,
                        kind: "NodeFailed".to_string(),
                        payload: serde_json::json!({ "node_id": item.repo_id, "error": reason }),
                        timestamp: chrono::Utc::now(),
                    };
                    self.ledger
                        .append_event(recorder.run_id(), fail_event)
                        .await
                        .map_err(|e| MultiRepoError::Storage(e.to_string()))?;
                    seq += 1;

                    // Mark all transitive dependents for skipping.
                    if let Ok(trans) = self.graph.transitive_dependents_of(&item.repo_id) {
                        for dep_id in trans {
                            skip_set.insert(dep_id);
                        }
                    }
                }
            }
        }

        let total_events = seq - 1;
        let overall_ok = failed.is_empty();

        if overall_ok {
            recorder
                .finish_ok(RunSummary {
                    total_events,
                    final_state_digest: None,
                    duration_ms: 0,
                    success: true,
                })
                .await
                .map_err(|e| MultiRepoError::Storage(e.to_string()))?;
        } else {
            recorder
                .finish_err(RunSummary {
                    total_events,
                    final_state_digest: None,
                    duration_ms: 0,
                    success: false,
                })
                .await
                .map_err(|e| MultiRepoError::Storage(e.to_string()))?;
        }

        Ok(SequenceOutcome {
            plan_id: plan.plan_id,
            succeeded,
            failed,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_repo::graph::RepoNode;
    use oxidized_state::fakes::MemoryRunLedger;

    fn graph_abc() -> RepoDependencyGraph {
        // A → B → C (C depends on B, B depends on A)
        let mut g = RepoDependencyGraph::new();
        for id in &["A", "B", "C"] {
            g.add_node(RepoNode::new(*id, *id));
        }
        g.add_dependency("A", "B").unwrap();
        g.add_dependency("B", "C").unwrap();
        g
    }

    fn release(repo: &str) -> (String, String, String, String) {
        (
            repo.to_string(),
            format!("v1-{}", repo),
            format!("digest-{}", repo),
            "bot".to_string(),
        )
    }

    struct SuccessReleaser;

    #[async_trait::async_trait]
    impl RepoReleaser for SuccessReleaser {
        async fn release(
            &self,
            repo_id: &str,
            _v: &str,
            _d: &str,
            _b: &str,
        ) -> MultiRepoResult<String> {
            Ok(format!("run-{}", repo_id))
        }
    }

    struct FailFirstReleaser {
        fail_repo: String,
    }

    #[async_trait::async_trait]
    impl RepoReleaser for FailFirstReleaser {
        async fn release(
            &self,
            repo_id: &str,
            _v: &str,
            _d: &str,
            _b: &str,
        ) -> MultiRepoResult<String> {
            if repo_id == self.fail_repo {
                Err(MultiRepoError::SequencingFailed {
                    repo: repo_id.to_string(),
                    reason: "intentional failure".to_string(),
                })
            } else {
                Ok(format!("run-{}", repo_id))
            }
        }
    }

    #[test]
    fn test_build_plan_orders_by_dependency() {
        let g = graph_abc();
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let seq = ReleaseSequencer::new(g, ledger);
        let plan = seq
            .build_plan(vec![release("A"), release("B"), release("C")], "run-origin")
            .unwrap();
        let ids: Vec<&str> = plan.items.iter().map(|i| i.repo_id.as_str()).collect();
        let a_idx = ids.iter().position(|&x| x == "A").unwrap();
        let b_idx = ids.iter().position(|&x| x == "B").unwrap();
        let c_idx = ids.iter().position(|&x| x == "C").unwrap();
        assert!(a_idx < b_idx && b_idx < c_idx);
    }

    #[tokio::test]
    async fn test_execute_plan_skips_downstream_on_upstream_failure() {
        let g = graph_abc();
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let sequencer = ReleaseSequencer::new(g, Arc::clone(&ledger));

        let plan = sequencer
            .build_plan(vec![release("A"), release("B"), release("C")], "run-0")
            .unwrap();

        let releaser = FailFirstReleaser {
            fail_repo: "A".to_string(),
        };
        let outcome = sequencer.execute_plan(plan, &releaser).await.unwrap();

        assert!(!outcome.overall_success());
        assert!(outcome.failed.iter().any(|(r, _)| r == "A"));
        // B and C are downstream of A — should be skipped.
        assert!(outcome.skipped.contains(&"B".to_string()));
        assert!(outcome.skipped.contains(&"C".to_string()));
    }

    #[tokio::test]
    async fn test_execute_plan_all_succeed() {
        let g = graph_abc();
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let sequencer = ReleaseSequencer::new(g, Arc::clone(&ledger));

        let plan = sequencer
            .build_plan(vec![release("A"), release("B"), release("C")], "run-1")
            .unwrap();

        let outcome = sequencer
            .execute_plan(plan, &SuccessReleaser)
            .await
            .unwrap();

        assert!(outcome.overall_success());
        assert_eq!(outcome.succeeded.len(), 3);
        assert!(outcome.skipped.is_empty());
    }

    #[test]
    fn test_build_plan_skips_unspecified_intermediate_repos() {
        let g = graph_abc();
        let ledger: Arc<dyn RunLedger> = Arc::new(MemoryRunLedger::new());
        let sequencer = ReleaseSequencer::new(g, ledger);
        // Only include A and C — B is not in the releases list.
        let plan = sequencer
            .build_plan(vec![release("A"), release("C")], "run-2")
            .unwrap();

        let b_item = plan.items.iter().find(|i| i.repo_id == "B").unwrap();
        assert_eq!(b_item.status, RepoReleaseStatus::Skipped);
    }
}
