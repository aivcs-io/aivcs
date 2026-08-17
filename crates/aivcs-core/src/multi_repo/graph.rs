//! Cross-repo dependency graph and topological execution planning.
//!
//! Models repositories as nodes in a directed acyclic graph (DAG). An edge
//! `A → B` means "B depends on A" — A must complete before B may run.
//!
//! Topological ordering is computed via Kahn's algorithm, producing a
//! level-ordered result so that same-level repos can be parallelized.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::multi_repo::error::{MultiRepoError, MultiRepoResult};

/// A single repository node in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoNode {
    /// Stable identifier, e.g. `"org/repo-name"`.
    pub repo_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Optional remote URL (for CI polling or webhook targeting).
    pub remote_url: Option<String>,
}

impl RepoNode {
    /// Create a minimal repo node.
    pub fn new(repo_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
            display_name: display_name.into(),
            remote_url: None,
        }
    }
}

/// A step in a repo-aware execution plan.
///
/// Mirrors [`crate::role_orchestration::router::RoleStep`] so that the same
/// parallel-group dispatch logic can drive repo-level execution.
#[derive(Debug, Clone)]
pub struct RepoStep {
    /// 0-indexed position in the plan.
    pub position: usize,
    /// The repo assigned to this step.
    pub repo: RepoNode,
    /// Repos whose completion this step waits for.
    pub depends_on: Vec<String>,
    /// True when this step can run concurrently with sibling steps at the
    /// same topological level.
    pub parallelizable: bool,
}

/// An ordered, validated execution plan for cross-repo operations.
#[derive(Debug, Clone)]
pub struct RepoExecutionPlan {
    /// Human-readable plan title.
    pub title: String,
    /// Ordered steps, respecting topological dependency order.
    pub steps: Vec<RepoStep>,
}

impl RepoExecutionPlan {
    /// Partition steps into sequential groups where adjacent parallelizable
    /// steps form one group and non-parallelizable steps form singletons.
    ///
    /// This mirrors `ExecutionPlan::parallel_groups()` from the role
    /// orchestration module.
    pub fn parallel_groups(&self) -> Vec<Vec<&RepoStep>> {
        let mut groups: Vec<Vec<&RepoStep>> = Vec::new();
        let mut current: Vec<&RepoStep> = Vec::new();
        let mut current_ids: HashSet<&str> = HashSet::new();

        for step in &self.steps {
            // A step must never share a group with a step it depends on, even
            // when both are flagged parallelizable. `parallelizable` only means
            // "shares a topological level with a sibling"; adjacent steps in
            // the plan can belong to different levels (e.g. roots `A,B`
            // followed by their dependents `C,D`), and grouping `C` with `A`
            // would schedule a repo concurrently with its own dependency.
            let depends_on_current = step
                .depends_on
                .iter()
                .any(|d| current_ids.contains(d.as_str()));

            if step.parallelizable && !depends_on_current {
                current.push(step);
                current_ids.insert(step.repo.repo_id.as_str());
            } else {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                    current_ids.clear();
                }
                if step.parallelizable {
                    // Opens a fresh parallel group (it depended on the previous one).
                    current.push(step);
                    current_ids.insert(step.repo.repo_id.as_str());
                } else {
                    groups.push(vec![step]);
                }
            }
        }
        if !current.is_empty() {
            groups.push(current);
        }
        groups
    }
}

/// Directed dependency graph over [`RepoNode`]s.
///
/// Edges are stored as `dependency → dependents` adjacency lists.
/// Cycles are detected at insertion time via DFS.
#[derive(Debug, Clone, Default)]
pub struct RepoDependencyGraph {
    nodes: HashMap<String, RepoNode>,
    /// `dependency_id → {dependent_id, ...}` (downstream adjacency)
    downstream: HashMap<String, HashSet<String>>,
    /// `dependent_id → {dependency_id, ...}` (upstream adjacency)
    upstream: HashMap<String, HashSet<String>>,
}

impl RepoDependencyGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a [`RepoNode`]. Idempotent — re-registering an existing id
    /// updates the node metadata.
    pub fn add_node(&mut self, node: RepoNode) {
        let id = node.repo_id.clone();
        self.nodes.insert(id.clone(), node);
        self.downstream.entry(id.clone()).or_default();
        self.upstream.entry(id).or_default();
    }

    /// Add a directed dependency edge: `dependent` depends on `dependency`.
    ///
    /// Both nodes must already be registered via [`Self::add_node`].
    /// Returns [`MultiRepoError::DependencyCycle`] if the edge would introduce
    /// a cycle (checked via DFS before the edge is committed).
    /// Returns [`MultiRepoError::RepoNotFound`] if either node is absent.
    pub fn add_dependency(&mut self, dependency: &str, dependent: &str) -> MultiRepoResult<()> {
        if !self.nodes.contains_key(dependency) {
            return Err(MultiRepoError::RepoNotFound {
                repo: dependency.to_string(),
            });
        }
        if !self.nodes.contains_key(dependent) {
            return Err(MultiRepoError::RepoNotFound {
                repo: dependent.to_string(),
            });
        }

        // Tentatively add the edge.
        self.downstream
            .entry(dependency.to_string())
            .or_default()
            .insert(dependent.to_string());
        self.upstream
            .entry(dependent.to_string())
            .or_default()
            .insert(dependency.to_string());

        // DFS cycle check starting from the newly added dependent.
        if let Some(cycle) = self.find_cycle_through(dependent) {
            // Roll back.
            self.downstream
                .get_mut(dependency)
                .unwrap()
                .remove(dependent);
            self.upstream.get_mut(dependent).unwrap().remove(dependency);
            return Err(MultiRepoError::DependencyCycle { repos: cycle });
        }

        Ok(())
    }

    /// Return repos in topological order (dependencies before dependents).
    ///
    /// Uses Kahn's algorithm. Returns [`MultiRepoError::DependencyCycle`]
    /// if a cycle is present (should not occur if `add_dependency` is used).
    pub fn topological_order(&self) -> MultiRepoResult<Vec<RepoNode>> {
        let mut in_degree: HashMap<&str, usize> =
            self.nodes.keys().map(|id| (id.as_str(), 0)).collect();

        for (dep, dependents) in &self.downstream {
            for d in dependents {
                *in_degree.entry(d.as_str()).or_default() += 1;
                let _ = dep; // suppress unused warning
            }
        }
        // Also handle nodes with no outgoing edges.
        for id in self.nodes.keys() {
            in_degree.entry(id.as_str()).or_default();
        }

        let mut roots: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        roots.sort_unstable();
        let mut queue: VecDeque<&str> = roots.into_iter().collect();

        let mut sorted = Vec::new();

        while let Some(node_id) = queue.pop_front() {
            sorted.push(node_id.to_string());
            if let Some(dependents) = self.downstream.get(node_id) {
                let mut next: Vec<&str> = Vec::new();
                for dep in dependents {
                    let deg = in_degree.get_mut(dep.as_str()).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next.push(dep.as_str());
                    }
                }
                // Stable sort to keep output deterministic.
                next.sort_unstable();
                queue.extend(next);
            }
        }

        if sorted.len() != self.nodes.len() {
            return Err(MultiRepoError::DependencyCycle {
                repos: self.nodes.keys().cloned().collect(),
            });
        }

        Ok(sorted
            .into_iter()
            .map(|id| self.nodes[&id].clone())
            .collect())
    }

    /// Return all direct dependencies of `repo_id` (repos it depends on).
    pub fn dependencies_of(&self, repo_id: &str) -> MultiRepoResult<Vec<&RepoNode>> {
        self.nodes
            .get(repo_id)
            .ok_or_else(|| MultiRepoError::RepoNotFound {
                repo: repo_id.to_string(),
            })?;
        let deps = self
            .upstream
            .get(repo_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        Ok(deps)
    }

    /// Return all direct dependents of `repo_id` (repos that depend on it).
    pub fn dependents_of(&self, repo_id: &str) -> MultiRepoResult<Vec<&RepoNode>> {
        self.nodes
            .get(repo_id)
            .ok_or_else(|| MultiRepoError::RepoNotFound {
                repo: repo_id.to_string(),
            })?;
        let deps = self
            .downstream
            .get(repo_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        Ok(deps)
    }

    /// All transitive dependents of `repo_id` (BFS over downstream edges).
    pub fn transitive_dependents_of(&self, repo_id: &str) -> MultiRepoResult<Vec<String>> {
        self.nodes
            .get(repo_id)
            .ok_or_else(|| MultiRepoError::RepoNotFound {
                repo: repo_id.to_string(),
            })?;

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(repo_id.to_string());

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = self.downstream.get(&current) {
                for dep in deps {
                    if visited.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        Ok(visited.into_iter().collect())
    }

    /// Convert to a [`RepoExecutionPlan`] by running topological sort and
    /// grouping same-level repos as parallelizable steps.
    ///
    /// Two steps are in the same "level" if they share exactly the same set
    /// of transitive upstream dependencies. We approximate this by tracking
    /// which Kahn wave each node belongs to.
    pub fn to_execution_plan(&self, title: &str) -> MultiRepoResult<RepoExecutionPlan> {
        if self.nodes.is_empty() {
            return Ok(RepoExecutionPlan {
                title: title.to_string(),
                steps: Vec::new(),
            });
        }

        // Kahn's algorithm with level tracking.
        let mut in_degree: HashMap<String, usize> =
            self.nodes.keys().map(|id| (id.clone(), 0)).collect();

        for dependents in self.downstream.values() {
            for dep in dependents {
                *in_degree.get_mut(dep).unwrap() += 1;
            }
        }

        let mut level_roots: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();
        level_roots.sort_unstable();
        let mut level_queue: VecDeque<(String, usize)> =
            level_roots.into_iter().map(|id| (id, 0usize)).collect();

        let mut node_level: HashMap<String, usize> = HashMap::new();
        let mut sorted_ids: Vec<String> = Vec::new();

        while let Some((node_id, level)) = level_queue.pop_front() {
            node_level.insert(node_id.clone(), level);
            sorted_ids.push(node_id.clone());

            if let Some(dependents) = self.downstream.get(&node_id) {
                let mut next: Vec<String> = Vec::new();
                for dep in dependents {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next.push(dep.clone());
                    }
                }
                next.sort_unstable();
                for dep in next {
                    level_queue.push_back((dep, level + 1));
                }
            }
        }

        if sorted_ids.len() != self.nodes.len() {
            return Err(MultiRepoError::DependencyCycle {
                repos: self.nodes.keys().cloned().collect(),
            });
        }

        // Count how many repos are at each level.
        let mut level_counts: HashMap<usize, usize> = HashMap::new();
        for l in node_level.values() {
            *level_counts.entry(*l).or_default() += 1;
        }

        let steps = sorted_ids
            .into_iter()
            .enumerate()
            .map(|(pos, id)| {
                let repo = self.nodes[&id].clone();
                let depends_on = self
                    .upstream
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                let level = *node_level.get(&id).unwrap();
                // Parallelizable when there are multiple repos at the same level.
                let parallelizable = level_counts.get(&level).copied().unwrap_or(1) > 1;
                RepoStep {
                    position: pos,
                    repo,
                    depends_on,
                    parallelizable,
                }
            })
            .collect();

        Ok(RepoExecutionPlan {
            title: title.to_string(),
            steps,
        })
    }

    /// DFS from `start` to detect cycles. Returns the cycle path if found.
    fn find_cycle_through(&self, start: &str) -> Option<Vec<String>> {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        if self.dfs_cycle(start, &mut visited, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    fn dfs_cycle<'a>(
        &'a self,
        node: &'a str,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if path.contains(&node.to_string()) {
            path.push(node.to_string());
            return true;
        }
        if visited.contains(node) {
            return false;
        }
        visited.insert(node.to_string());
        path.push(node.to_string());

        if let Some(dependents) = self.downstream.get(node) {
            for dep in dependents {
                if self.dfs_cycle(dep, visited, path) {
                    return true;
                }
            }
        }

        path.pop();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> RepoNode {
        RepoNode::new(id, id)
    }

    fn three_chain() -> RepoDependencyGraph {
        // C → B → A  (A depends on B, B depends on C)
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("C"));
        g.add_node(node("B"));
        g.add_node(node("A"));
        g.add_dependency("C", "B").unwrap(); // B depends on C
        g.add_dependency("B", "A").unwrap(); // A depends on B
        g
    }

    #[test]
    fn test_topological_order_respects_deps() {
        let g = three_chain();
        let order: Vec<String> = g
            .topological_order()
            .unwrap()
            .into_iter()
            .map(|n| n.repo_id)
            .collect();
        let c_idx = order.iter().position(|x| x == "C").unwrap();
        let b_idx = order.iter().position(|x| x == "B").unwrap();
        let a_idx = order.iter().position(|x| x == "A").unwrap();
        assert!(c_idx < b_idx, "C must come before B");
        assert!(b_idx < a_idx, "B must come before A");
    }

    #[test]
    fn test_cycle_detection_rejects_mutual_dependency() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("X"));
        g.add_node(node("Y"));
        g.add_dependency("X", "Y").unwrap(); // Y depends on X
        let result = g.add_dependency("Y", "X"); // X depends on Y → cycle
        assert!(matches!(
            result,
            Err(MultiRepoError::DependencyCycle { .. })
        ));
    }

    #[test]
    fn test_parallel_groups_partitions_independent_repos() {
        // A and B have no dependencies on each other → same level → parallelizable.
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("A"));
        g.add_node(node("B"));
        let plan = g.to_execution_plan("test").unwrap();
        let groups = plan.parallel_groups();
        // Both repos at level 0 → one parallel group.
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn test_single_repo_graph_produces_one_step_plan() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("solo"));
        let plan = g.to_execution_plan("solo plan").unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(!plan.steps[0].parallelizable);
    }

    #[test]
    fn test_to_execution_plan_title_is_preserved() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("r1"));
        let plan = g.to_execution_plan("my plan title").unwrap();
        assert_eq!(plan.title, "my plan title");
    }

    #[test]
    fn test_repo_not_found_error_on_missing_node() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("A"));
        let r = g.add_dependency("A", "missing");
        assert!(matches!(r, Err(MultiRepoError::RepoNotFound { .. })));
    }

    #[test]
    fn test_transitive_dependents_covers_full_chain() {
        let g = three_chain(); // C → B → A
        let mut trans = g.transitive_dependents_of("C").unwrap();
        trans.sort();
        assert!(trans.contains(&"B".to_string()));
        assert!(trans.contains(&"A".to_string()));
        assert!(!trans.contains(&"C".to_string()));
    }

    #[test]
    fn test_parallel_groups_never_group_a_step_with_its_dependency() {
        // Roots A,B (level 0); C and D both depend on A (level 1). All four are
        // "parallelizable" (their level has >1 node), so the old adjacency-blind
        // grouping merged them into a single group — scheduling C and D to run
        // concurrently with their dependency A.
        let mut g = RepoDependencyGraph::new();
        for id in &["A", "B", "C", "D"] {
            g.add_node(node(id));
        }
        g.add_dependency("A", "C").unwrap();
        g.add_dependency("A", "D").unwrap();

        let plan = g.to_execution_plan("levels").unwrap();
        let groups = plan.parallel_groups();

        // No group may contain both a step and one of its dependencies.
        for group in &groups {
            let ids: HashSet<&str> = group.iter().map(|s| s.repo.repo_id.as_str()).collect();
            for step in group {
                for dep in &step.depends_on {
                    assert!(
                        !ids.contains(dep.as_str()),
                        "step {} grouped with its dependency {}",
                        step.repo.repo_id,
                        dep
                    );
                }
            }
        }
        // A and B (level 0) form one group; C and D (level 1) form another.
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_diamond_graph_resolves_correctly() {
        // A→B, A→C, B→D, C→D
        let mut g = RepoDependencyGraph::new();
        for id in &["A", "B", "C", "D"] {
            g.add_node(node(id));
        }
        g.add_dependency("A", "B").unwrap();
        g.add_dependency("A", "C").unwrap();
        g.add_dependency("B", "D").unwrap();
        g.add_dependency("C", "D").unwrap();

        let order = g.topological_order().unwrap();
        let ids: Vec<&str> = order.iter().map(|n| n.repo_id.as_str()).collect();
        let a_idx = ids.iter().position(|&x| x == "A").unwrap();
        let d_idx = ids.iter().position(|&x| x == "D").unwrap();
        assert!(a_idx < d_idx);
    }

    #[test]
    fn test_topological_order_is_deterministic_for_independent_roots() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("B"));
        g.add_node(node("A"));
        g.add_node(node("C"));
        g.add_dependency("A", "C").unwrap();
        g.add_dependency("B", "C").unwrap();

        let first: Vec<String> = g
            .topological_order()
            .unwrap()
            .into_iter()
            .map(|n| n.repo_id)
            .collect();
        let second: Vec<String> = g
            .topological_order()
            .unwrap()
            .into_iter()
            .map(|n| n.repo_id)
            .collect();

        assert_eq!(first, second);
        assert_eq!(first, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_execution_plan_is_deterministic_for_independent_roots() {
        let mut g = RepoDependencyGraph::new();
        g.add_node(node("repo-b"));
        g.add_node(node("repo-a"));
        g.add_node(node("repo-c"));
        g.add_dependency("repo-a", "repo-c").unwrap();
        g.add_dependency("repo-b", "repo-c").unwrap();

        let plan = g.to_execution_plan("deterministic").unwrap();
        let ids: Vec<String> = plan.steps.into_iter().map(|s| s.repo.repo_id).collect();
        assert_eq!(ids, vec!["repo-a", "repo-b", "repo-c"]);
    }
}
