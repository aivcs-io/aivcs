//! Multi-repo model: repo identity, dependency graph, and constraints.
//!
//! EPIC9: Cross-repo dependency-aware execution and change graph.

use serde::{Deserialize, Serialize};

/// Identifies a repository (e.g. org/name or URL).
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoId {
    /// Canonical name, e.g. "stevedores-org/aivcs".
    pub name: String,
}

impl RepoId {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Directed dependency: `dependent` depends on `dependency` (dependent is built after dependency).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoDependency {
    pub dependent: RepoId,
    pub dependency: RepoId,
}

/// Cross-repo change graph: repos and their dependencies.
/// Supports topological order and cycle detection for dependency-aware execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrossRepoGraph {
    /// All repo nodes.
    pub repos: Vec<RepoId>,
    /// Directed edges: from dependency -> dependent (dependent runs after dependency).
    pub dependencies: Vec<RepoDependency>,
}

impl CrossRepoGraph {
    pub fn new(repos: Vec<RepoId>, dependencies: Vec<RepoDependency>) -> Self {
        Self {
            repos,
            dependencies,
        }
    }

    /// Returns repos in dependency order: dependencies first, dependents last.
    /// Fails with an error if the graph contains a cycle.
    pub fn execution_order(&self) -> Result<Vec<RepoId>, String> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let names: HashSet<&str> = self.repos.iter().map(|r| r.name.as_str()).collect();
        let mut in_degree: HashMap<&str, u32> = names.iter().map(|&n| (n, 0)).collect();
        let mut out_edges: HashMap<&str, Vec<&str>> =
            names.iter().map(|&n| (n, Vec::new())).collect();

        for d in &self.dependencies {
            if !names.contains(d.dependency.name.as_str())
                || !names.contains(d.dependent.name.as_str())
            {
                continue;
            }
            out_edges
                .get_mut(d.dependency.name.as_str())
                .unwrap()
                .push(d.dependent.name.as_str());
            *in_degree.get_mut(d.dependent.name.as_str()).unwrap() += 1;
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();
        let mut order = Vec::with_capacity(self.repos.len());

        while let Some(n) = queue.pop_front() {
            order.push(RepoId::new(n));
            for &m in out_edges.get(n).unwrap_or(&vec![]) {
                let deg = in_degree.get_mut(m).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(m);
                }
            }
        }

        if order.len() != self.repos.len() {
            return Err("cycle detected in repo dependencies".to_string());
        }
        Ok(order)
    }

    /// Returns true if the graph has a cycle.
    pub fn has_cycle(&self) -> bool {
        self.execution_order().is_err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_id_equality() {
        let a = RepoId::new("org/a");
        let b = RepoId::new("org/a");
        let c = RepoId::new("org/c");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_execution_order_linear() {
        let a = RepoId::new("a");
        let b = RepoId::new("b");
        let c = RepoId::new("c");
        let graph = CrossRepoGraph::new(
            vec![a.clone(), b.clone(), c.clone()],
            vec![
                RepoDependency {
                    dependent: b.clone(),
                    dependency: a.clone(),
                },
                RepoDependency {
                    dependent: c.clone(),
                    dependency: b.clone(),
                },
            ],
        );
        let order = graph.execution_order().expect("no cycle");
        assert_eq!(order.len(), 3);
        assert_eq!(order[0].name, "a");
        assert_eq!(order[1].name, "b");
        assert_eq!(order[2].name, "c");
    }

    #[test]
    fn test_execution_order_diamond() {
        let a = RepoId::new("a");
        let b = RepoId::new("b");
        let c = RepoId::new("c");
        let d = RepoId::new("d");
        let graph = CrossRepoGraph::new(
            vec![a.clone(), b.clone(), c.clone(), d.clone()],
            vec![
                RepoDependency {
                    dependent: b.clone(),
                    dependency: a.clone(),
                },
                RepoDependency {
                    dependent: c.clone(),
                    dependency: a.clone(),
                },
                RepoDependency {
                    dependent: d.clone(),
                    dependency: b.clone(),
                },
                RepoDependency {
                    dependent: d.clone(),
                    dependency: c.clone(),
                },
            ],
        );
        let order = graph.execution_order().expect("no cycle");
        assert_eq!(order.len(), 4);
        assert_eq!(order[0].name, "a");
        let pos_b = order.iter().position(|r| r.name == "b").unwrap();
        let pos_c = order.iter().position(|r| r.name == "c").unwrap();
        let pos_d = order.iter().position(|r| r.name == "d").unwrap();
        assert!(pos_b < pos_d && pos_c < pos_d);
    }

    #[test]
    fn test_cycle_detected() {
        let a = RepoId::new("a");
        let b = RepoId::new("b");
        let graph = CrossRepoGraph::new(
            vec![a.clone(), b.clone()],
            vec![
                RepoDependency {
                    dependent: b.clone(),
                    dependency: a.clone(),
                },
                RepoDependency {
                    dependent: a.clone(),
                    dependency: b.clone(),
                },
            ],
        );
        assert!(graph.execution_order().is_err());
        assert!(graph.has_cycle());
    }

    #[test]
    fn test_serde_repo_id() {
        let id = RepoId::new("stevedores-org/aivcs");
        let json = serde_json::to_string(&id).expect("serialize");
        let back: RepoId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }
}
