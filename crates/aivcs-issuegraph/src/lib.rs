pub mod graph;
pub mod mesh;
pub use graph::{would_create_blocks_cycle, EdgeKind, GraphView, IssueEdge, IssueNode, IssueRef};
pub use mesh::{
    would_create_cycle, MeshEdge, MeshEdgeKind, MeshGraphView, MeshNode, MeshRef, NodeKind,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Represents the Agentic Issue VCS object known as an IssueGraph.
/// It tracks the state, constraints, intent, and execution of a specific issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueGraph {
    pub id: Uuid,
    pub title: String,
    pub state: IssueState,
    pub intent: String,
    pub constraints: Vec<String>,
    pub branches: HashMap<String, IssueBranch>,
    pub current_branch: String,
    pub ledger: Vec<LedgerEntry>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Planned,
    Exploration,
    Executing,
    Verified,
    Closed,
}

/// An Issue Branch represents parallel exploration of solutions or planning paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueBranch {
    pub name: String,
    pub commits: Vec<IssueCommit>,
}

/// Issue Commits represent flat snapshots of cognition, execution state, or plan revisions,
/// aligned fully with the SurrealDB table structure and the hitl/packages/demo-adapter schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueCommit {
    pub issue_ref: String,

    #[serde(rename = "commit_id")]
    pub id: Uuid,

    pub parent_commit_id: Option<Uuid>,
    pub state: IssueState,
    pub intent: String,
    pub acceptance_criteria: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub agent_refs: Vec<String>,
    pub repo_refs: Vec<String>,
    pub pr_refs: Vec<String>,
    pub ci_refs: Vec<String>,
    pub risk_class: String,
    pub policy_claims: HashMap<String, String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,

    // Compatibility fields
    pub message: String,
    pub execution_state: Option<serde_json::Value>,
    pub diff_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerEntry {
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub metadata: Option<serde_json::Value>,
}

impl IssueGraph {
    pub fn new(title: String, intent: String) -> Self {
        let now = Utc::now();
        let main_branch = IssueBranch {
            name: "main".to_string(),
            commits: vec![],
        };
        let mut branches = HashMap::new();
        branches.insert("main".to_string(), main_branch);

        Self {
            id: Uuid::new_v4(),
            title,
            state: IssueState::Planned,
            intent,
            constraints: vec![],
            branches,
            current_branch: "main".to_string(),
            ledger: vec![],
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a constraint (acceptance criteria) to the issue graph
    pub fn add_constraint(&mut self, constraint: String) {
        self.constraints.push(constraint);
        self.updated_at = Utc::now();
    }

    /// Set the active branch
    pub fn checkout(&mut self, branch_name: &str) -> Result<(), anyhow::Error> {
        if !self.branches.contains_key(branch_name) {
            return Err(anyhow::anyhow!("Branch '{}' does not exist", branch_name));
        }
        self.current_branch = branch_name.to_string();
        self.updated_at = Utc::now();
        self.add_ledger_entry(
            "system".to_string(),
            format!("checkout branch '{}'", branch_name),
            None,
        );
        Ok(())
    }

    /// Create a new branch from the current branch's latest commit
    pub fn create_branch(&mut self, name: &str) -> Result<(), anyhow::Error> {
        if self.branches.contains_key(name) {
            return Err(anyhow::anyhow!("Branch '{}' already exists", name));
        }

        let current_commits = self
            .branches
            .get(&self.current_branch)
            .map(|b| b.commits.clone())
            .unwrap_or_default();

        let new_branch = IssueBranch {
            name: name.to_string(),
            commits: current_commits,
        };

        self.branches.insert(name.to_string(), new_branch);
        self.updated_at = Utc::now();
        self.add_ledger_entry(
            "system".to_string(),
            format!("create branch '{}' from '{}'", name, self.current_branch),
            None,
        );
        Ok(())
    }

    /// Get the latest commit on the current branch
    pub fn latest_commit(&self) -> Option<&IssueCommit> {
        self.branches
            .get(&self.current_branch)
            .and_then(|b| b.commits.last())
    }

    /// Helper builder method to generate and record a new commit on the active branch
    pub fn commit(
        &mut self,
        message: String,
        created_by: String,
        execution_state: Option<serde_json::Value>,
        diff_summary: Option<String>,
    ) -> Uuid {
        let parent_commit_id = self.latest_commit().map(|c| c.id);
        let issue_ref = format!("issue:{}", self.id);

        let commit = IssueCommit {
            issue_ref,
            id: Uuid::new_v4(),
            parent_commit_id,
            state: self.state.clone(),
            intent: self.intent.clone(),
            acceptance_criteria: self.constraints.clone(),
            evidence_refs: vec![],
            agent_refs: vec![],
            repo_refs: vec![],
            pr_refs: vec![],
            ci_refs: vec![],
            risk_class: "low".to_string(),
            policy_claims: HashMap::new(),
            created_by: created_by.clone(),
            created_at: Utc::now(),
            message,
            execution_state,
            diff_summary,
        };

        let branch = self
            .branches
            .get_mut(&self.current_branch)
            .expect("Current branch not found");
        branch.commits.push(commit.clone());

        self.add_ledger_entry(created_by, format!("commit: {}", commit.id), None);
        self.updated_at = Utc::now();
        commit.id
    }

    /// Detailed commit creation method allowing customization of all references and metadata
    pub fn commit_detailed(&mut self, mut commit: IssueCommit) -> Uuid {
        commit.parent_commit_id = self.latest_commit().map(|c| c.id);
        let commit_id = commit.id;

        let branch = self
            .branches
            .get_mut(&self.current_branch)
            .expect("Current branch not found");
        branch.commits.push(commit.clone());

        self.add_ledger_entry(
            commit.created_by.clone(),
            format!("commit: {}", commit_id),
            None,
        );
        self.updated_at = Utc::now();
        commit_id
    }

    pub fn add_ledger_entry(
        &mut self,
        actor: String,
        action: String,
        metadata: Option<serde_json::Value>,
    ) {
        self.ledger.push(LedgerEntry {
            timestamp: Utc::now(),
            actor,
            action,
            metadata,
        });
        self.updated_at = Utc::now();
    }

    pub fn semantic_merge(
        &mut self,
        target_branch: &str,
        source_branch: &str,
    ) -> Result<(), anyhow::Error> {
        if !self.branches.contains_key(source_branch) {
            return Err(anyhow::anyhow!("Source branch not found"));
        }

        let source = self.branches.get(source_branch).unwrap().clone();
        let target = self
            .branches
            .get_mut(target_branch)
            .ok_or_else(|| anyhow::anyhow!("Target branch not found"))?;

        let existing_ids: std::collections::HashSet<Uuid> =
            target.commits.iter().map(|c| c.id).collect();
        for commit in source.commits {
            if !existing_ids.contains(&commit.id) {
                target.commits.push(commit);
            }
        }

        self.add_ledger_entry(
            "system".into(),
            format!("semantic_merge {} into {}", source_branch, target_branch),
            None,
        );
        self.updated_at = Utc::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_graph_creation_and_basic_commit() {
        let mut graph = IssueGraph::new(
            "Fix connection timeout".to_string(),
            "Investigate and increase the Database pool timeout config".to_string(),
        );

        assert_eq!(graph.state, IssueState::Planned);
        assert_eq!(graph.current_branch, "main");
        assert!(graph.branches.contains_key("main"));

        graph.add_constraint("Timeout should be exactly 30 seconds".to_string());
        assert_eq!(graph.constraints.len(), 1);

        let commit_id = graph.commit(
            "Initial planning phase finished".to_string(),
            "lornu-ai-bot".to_string(),
            None,
            None,
        );

        let main_branch = graph.branches.get("main").unwrap();
        assert_eq!(main_branch.commits.len(), 1);

        let first_commit = &main_branch.commits[0];
        assert_eq!(first_commit.id, commit_id);
        assert_eq!(first_commit.message, "Initial planning phase finished");
        assert_eq!(first_commit.created_by, "lornu-ai-bot");
        assert_eq!(
            first_commit.acceptance_criteria[0],
            "Timeout should be exactly 30 seconds"
        );
        assert_eq!(first_commit.state, IssueState::Planned);
        assert_eq!(first_commit.parent_commit_id, None);
    }

    #[test]
    fn test_branching_and_semantic_merge() {
        let mut graph = IssueGraph::new(
            "Implement OIDC Auth".to_string(),
            "Add OpenID Connect support for agent identities".to_string(),
        );

        // Commit on main
        graph.commit(
            "Initial setup".to_string(),
            "agent-1".to_string(),
            None,
            None,
        );

        // Branch and checkout
        assert!(graph.create_branch("feature/oidc-flows").is_ok());
        assert!(graph.checkout("feature/oidc-flows").is_ok());

        assert_eq!(graph.current_branch, "feature/oidc-flows");

        // Advanced states
        graph.state = IssueState::Executing;
        let feat_commit_id = graph.commit(
            "Implement flow handshake".to_string(),
            "agent-1".to_string(),
            None,
            None,
        );

        // Verify the latest commit has parent_commit_id pointing to initial setup
        let latest = graph.latest_commit().unwrap();
        assert_eq!(latest.id, feat_commit_id);
        assert!(latest.parent_commit_id.is_some());

        // Checkout main and semantic merge
        assert!(graph.checkout("main").is_ok());
        assert!(graph.semantic_merge("main", "feature/oidc-flows").is_ok());

        let main_branch = graph.branches.get("main").unwrap();
        assert_eq!(main_branch.commits.len(), 2);
        assert_eq!(main_branch.commits[1].id, feat_commit_id);
    }

    #[test]
    fn test_serialization_compatibility_roundtrip() {
        let mut graph = IssueGraph::new(
            "Test serialization".to_string(),
            "Validate that JSON serialization maps fully to the model".to_string(),
        );

        let mut claims = HashMap::new();
        claims.insert("risk_class".to_string(), "medium".to_string());
        claims.insert("policy_compliance".to_string(), "approved".to_string());

        let commit = IssueCommit {
            issue_ref: "issue:12345".to_string(),
            id: Uuid::new_v4(),
            parent_commit_id: None,
            state: IssueState::Exploration,
            intent: "Exploration branch intent".to_string(),
            acceptance_criteria: vec!["Must pass all baseline tests".to_string()],
            evidence_refs: vec!["evidence_123".to_string()],
            agent_refs: vec!["agent_abc".to_string()],
            repo_refs: vec!["lornu-ai/aivcs.io".to_string()],
            pr_refs: vec!["pr_45".to_string()],
            ci_refs: vec!["ci_78".to_string()],
            risk_class: "medium".to_string(),
            policy_claims: claims,
            created_by: "agent-verifier".to_string(),
            created_at: Utc::now(),
            message: "Setup exploratory verification runs".to_string(),
            execution_state: Some(serde_json::json!({"status": "running", "step": 3})),
            diff_summary: Some("+++ b/src/lib.rs".to_string()),
        };

        graph.commit_detailed(commit.clone());

        let serialized = serde_json::to_string(&graph).unwrap();
        let deserialized: IssueGraph = serde_json::from_str(&serialized).unwrap();

        let branch = deserialized.branches.get("main").unwrap();
        assert_eq!(branch.commits.len(), 1);
        assert_eq!(branch.commits[0], commit);
    }
}
