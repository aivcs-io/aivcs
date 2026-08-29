//! Parallel Simulation Module
//!
//! Provides concurrent agent exploration capabilities:
//! - Fork multiple branches from a parent commit
//! - Run agent variants concurrently using Tokio
//! - Prune low-performing branches based on score threshold
//!
//! # TDD Tests:
//! - test_five_branches_are_forked_and_run_concurrently_via_tokio
//! - test_optimizer_kills_branch_when_score_threshold_is_missed

use anyhow::Result;
use oxidized_state::{CommitId, CommitRecord, SurrealHandle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use crate::metrics::METRICS;

/// Result of forking multiple branches
#[derive(Debug, Clone)]
pub struct ForkResult {
    /// Parent commit that was forked from
    #[allow(dead_code)]
    pub parent_commit: String,
    /// Fork labels (e.g. experiment-0, experiment-1)
    pub fork_labels: Vec<String>,
    /// Commit IDs for each branch
    pub commit_ids: Vec<CommitId>,
}

/// Status of a running parallel branch
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchStatus {
    /// Branch name
    pub name: String,
    /// Current commit ID
    pub commit_id: String,
    /// Performance score (0.0 - 1.0)
    pub score: f32,
    /// Whether the branch is still active
    pub active: bool,
    /// Step count in this branch
    pub step: usize,
}

/// Configuration for parallel exploration
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ParallelConfig {
    /// Minimum score threshold (branches below this get pruned)
    pub score_threshold: f32,
    /// Maximum concurrent branches
    #[allow(dead_code)]
    pub max_branches: usize,
    /// Auto-prune low performers
    #[allow(dead_code)]
    pub auto_prune: bool,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            score_threshold: 0.3,
            max_branches: 10,
            auto_prune: true,
        }
    }
}

/// Fork multiple agent branches from a parent commit
///
/// Creates `count` new branches, each starting from the same parent commit.
/// The branches are created concurrently using Tokio.
///
/// # TDD: test_five_branches_are_forked_and_run_concurrently_via_tokio
///
/// # Arguments
/// * `handle` - SurrealDB handle
/// * `parent_commit` - Commit ID to fork from
/// * `count` - Number of branches to create
/// * `prefix` - Branch name prefix (branches named "{prefix}-0", "{prefix}-1", etc.)
///
/// # Returns
/// * `ForkResult` containing the created branch names and commit IDs
#[instrument(skip(handle), fields(parent = %&parent_commit[..8.min(parent_commit.len())]))]
pub async fn fork_agent_parallel(
    handle: Arc<SurrealHandle>,
    parent_commit: &str,
    count: u8,
    prefix: &str,
) -> Result<ForkResult> {
    METRICS.inc_forks();
    info!(
        "Forking {} parallel branches from {}",
        count,
        &parent_commit[..8.min(parent_commit.len())]
    );

    // Get parent snapshot to clone state
    let parent_snapshot = handle.load_snapshot(parent_commit).await?;

    // Spawn concurrent tasks to create branches
    let mut tasks: Vec<JoinHandle<Result<(String, CommitId)>>> = Vec::new();

    for i in 0..count {
        let handle_clone = Arc::clone(&handle);
        let parent_id = parent_commit.to_string();
        let branch_name = format!("{}-{}", prefix, i);
        let state = parent_snapshot.state.clone();

        let task = tokio::spawn(async move {
            // Create commit ID for this branch
            let fork_data = format!("fork:{}:{}", parent_id, branch_name);
            let commit_id = CommitId::from_state(fork_data.as_bytes());

            // Save forked snapshot
            handle_clone.save_snapshot(&commit_id, state).await?;

            // Create commit record
            let commit = CommitRecord::new(
                commit_id.clone(),
                vec![parent_id.clone()],
                &format!("Fork branch {}", branch_name),
                "parallel-fork",
            );
            handle_clone.save_commit(&commit).await?;

            // Create graph edge
            handle_clone
                .save_commit_graph_edge(&commit_id.hash, &parent_id)
                .await?;

            debug!("Created fork {} at {}", branch_name, commit_id.short());
            Ok((branch_name, commit_id))
        });

        tasks.push(task);
    }

    // Wait for all forks to complete
    let mut fork_labels = Vec::new();
    let mut commit_ids = Vec::new();

    for task in tasks {
        let (name, id) = task.await??;
        fork_labels.push(name);
        commit_ids.push(id);
    }

    info!("Created {} parallel forks", fork_labels.len());

    Ok(ForkResult {
        parent_commit: parent_commit.to_string(),
        fork_labels,
        commit_ids,
    })
}

/// Parallel branch manager for tracking and pruning branches
#[allow(dead_code)]
pub struct ParallelManager {
    #[allow(dead_code)]
    handle: Arc<SurrealHandle>,
    config: ParallelConfig,
    branch_status: Arc<Mutex<Vec<BranchStatus>>>,
}

#[allow(dead_code)]
impl ParallelManager {
    /// Create a new parallel manager
    pub fn new(handle: Arc<SurrealHandle>, config: ParallelConfig) -> Self {
        Self {
            handle,
            config,
            branch_status: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a branch for tracking
    pub async fn register_branch(&self, name: &str, commit_id: &str) {
        let mut status = self.branch_status.lock().await;
        status.push(BranchStatus {
            name: name.to_string(),
            commit_id: commit_id.to_string(),
            score: 1.0, // Start with perfect score
            active: true,
            step: 0,
        });
    }

    /// Update branch score
    pub async fn update_score(&self, branch_name: &str, score: f32) {
        let mut status = self.branch_status.lock().await;
        if let Some(branch) = status.iter_mut().find(|b| b.name == branch_name) {
            branch.score = score;
        }
    }

    /// Update branch step count
    pub async fn update_step(&self, branch_name: &str, step: usize) {
        let mut status = self.branch_status.lock().await;
        if let Some(branch) = status.iter_mut().find(|b| b.name == branch_name) {
            branch.step = step;
        }
    }

    /// Get all branch statuses
    pub async fn get_statuses(&self) -> Vec<BranchStatus> {
        self.branch_status.lock().await.clone()
    }

    /// Prune branches that fall below the score threshold
    ///
    /// # TDD: test_optimizer_kills_branch_when_score_threshold_is_missed
    ///
    /// Returns the names of pruned branches
    pub async fn prune_low_performing_branches(&self) -> Result<Vec<String>> {
        let mut status = self.branch_status.lock().await;
        let threshold = self.config.score_threshold;

        let mut pruned = Vec::new();

        for branch in status.iter_mut() {
            if branch.active && branch.score < threshold {
                warn!(
                    "Pruning branch '{}' - score {} below threshold {}",
                    branch.name, branch.score, threshold
                );
                branch.active = false;
                pruned.push(branch.name.clone());
            }
        }

        if !pruned.is_empty() {
            info!("Pruned {} low-performing branches", pruned.len());
        }

        Ok(pruned)
    }

    /// Get active branch count
    pub async fn active_count(&self) -> usize {
        let status = self.branch_status.lock().await;
        status.iter().filter(|b| b.active).count()
    }

    /// Check if a specific branch is still active
    pub async fn is_active(&self, branch_name: &str) -> bool {
        let status = self.branch_status.lock().await;
        status
            .iter()
            .find(|b| b.name == branch_name)
            .map(|b| b.active)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_five_branches_are_forked_and_run_concurrently_via_tokio() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());

        // Create a parent commit to fork from
        let parent_state = serde_json::json!({
            "agent": "optimizer",
            "strategy": "baseline",
            "step": 0
        });
        let parent_id = CommitId::from_state(b"parent-state");
        handle
            .save_snapshot(&parent_id, parent_state)
            .await
            .unwrap();

        let parent_commit = CommitRecord::new(parent_id.clone(), vec![], "Parent commit", "test");
        handle.save_commit(&parent_commit).await.unwrap();

        // Fork 5 branches concurrently
        let result = fork_agent_parallel(Arc::clone(&handle), &parent_id.hash, 5, "experiment")
            .await
            .unwrap();

        // Verify all 5 branches were created
        assert_eq!(result.fork_labels.len(), 5, "Should create 5 forks");
        assert_eq!(result.commit_ids.len(), 5, "Should have 5 commit IDs");

        // Verify forks have unique labels
        let unique_names: std::collections::HashSet<_> = result.fork_labels.iter().collect();
        assert_eq!(unique_names.len(), 5, "Fork labels should be unique");

        // Verify graph edges point to parent
        for commit_id in &result.commit_ids {
            let parent = handle.get_parent(&commit_id.hash).await.unwrap();
            assert_eq!(
                parent,
                Some(parent_id.hash.clone()),
                "Fork should have parent edge"
            );
        }
    }

    #[tokio::test]
    async fn test_optimizer_kills_branch_when_score_threshold_is_missed() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());

        let config = ParallelConfig {
            score_threshold: 0.5,
            max_branches: 10,
            auto_prune: true,
        };

        let manager = ParallelManager::new(handle, config);

        // Register branches with different scores
        manager.register_branch("high-performer", "commit-1").await;
        manager
            .register_branch("medium-performer", "commit-2")
            .await;
        manager.register_branch("low-performer", "commit-3").await;
        manager
            .register_branch("very-low-performer", "commit-4")
            .await;

        // Set scores
        manager.update_score("high-performer", 0.9).await;
        manager.update_score("medium-performer", 0.6).await;
        manager.update_score("low-performer", 0.3).await; // Below threshold
        manager.update_score("very-low-performer", 0.1).await; // Below threshold

        // Prune low performers
        let pruned = manager.prune_low_performing_branches().await.unwrap();

        // Verify correct branches were pruned
        assert_eq!(pruned.len(), 2, "Should prune 2 branches");
        assert!(pruned.contains(&"low-performer".to_string()));
        assert!(pruned.contains(&"very-low-performer".to_string()));

        // Verify high performers are still active
        assert!(manager.is_active("high-performer").await);
        assert!(manager.is_active("medium-performer").await);
        assert!(!manager.is_active("low-performer").await);
        assert!(!manager.is_active("very-low-performer").await);

        // Verify active count
        assert_eq!(manager.active_count().await, 2);
    }

    #[tokio::test]
    async fn test_parallel_manager_tracks_branch_progress() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
        let manager = ParallelManager::new(handle, ParallelConfig::default());

        manager.register_branch("branch-1", "commit-abc").await;
        manager.update_step("branch-1", 5).await;
        manager.update_score("branch-1", 0.75).await;

        let statuses = manager.get_statuses().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].step, 5);
        assert_eq!(statuses[0].score, 0.75);
        assert!(statuses[0].active);
    }
}
