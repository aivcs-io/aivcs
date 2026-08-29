//! Cross-org integration audit logic (Issue #184).
//!
//! Analyzes cross-repo dependency graphs to identify reliability coupling,
//! critical paths, and single points of failure (SPOFs).

use crate::multi_repo::model::CrossRepoGraph;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Reliability metrics for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoCoupling {
    pub repo_id: String,
    /// Number of immediate downstream dependents.
    pub direct_dependents: usize,
    /// Total number of downstream nodes affected by this repo (transitive).
    pub blast_radius: usize,
    /// If true, this repo is a dependency for a significant portion of the swarm.
    pub is_critical_path: bool,
}

/// Consolidated audit results for a cross-org integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossOrgAudit {
    pub coupling: Vec<RepoCoupling>,
    pub critical_spofs: Vec<String>,
}

pub struct AuditEngine<'a> {
    graph: &'a CrossRepoGraph,
}

impl<'a> AuditEngine<'a> {
    pub fn new(graph: &'a CrossRepoGraph) -> Self {
        Self { graph }
    }

    /// Perform a full reliability coupling analysis.
    pub fn audit(&self) -> CrossOrgAudit {
        let mut coupling = Vec::new();
        let mut critical_spofs = Vec::new();

        // Map dependency -> list of dependents
        let mut downstream_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for dep in &self.graph.dependencies {
            downstream_map
                .entry(&dep.dependency.name)
                .or_default()
                .push(&dep.dependent.name);
        }

        for repo in &self.graph.repos {
            let direct_dependents = downstream_map
                .get(repo.name.as_str())
                .map(|v| v.len())
                .unwrap_or(0);

            let blast_radius = self.compute_blast_radius(repo.name.as_str(), &downstream_map);

            // Heuristic for critical path: affects > 30% of the graph
            let is_critical_path = blast_radius as f32 > (self.graph.repos.len() as f32 * 0.3);

            coupling.push(RepoCoupling {
                repo_id: repo.name.clone(),
                direct_dependents,
                blast_radius,
                is_critical_path,
            });

            if is_critical_path {
                critical_spofs.push(repo.name.clone());
            }
        }

        CrossOrgAudit {
            coupling,
            critical_spofs,
        }
    }

    fn compute_blast_radius(&self, start_node: &str, map: &HashMap<&str, Vec<&str>>) -> usize {
        let mut visited = HashSet::new();
        let mut stack = vec![start_node];

        while let Some(node) = stack.pop() {
            if let Some(downstream) = map.get(node) {
                for next in downstream {
                    if visited.insert(*next) {
                        stack.push(next);
                    }
                }
            }
        }

        visited.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_repo::model::{RepoDependency, RepoId};

    #[test]
    fn test_blast_radius_calculation() {
        let repos = vec![
            RepoId::new("A"),
            RepoId::new("B"),
            RepoId::new("C"),
            RepoId::new("D"),
        ];
        // A -> B -> C
        // A -> D
        let dependencies = vec![
            RepoDependency {
                dependent: RepoId::new("B"),
                dependency: RepoId::new("A"),
            },
            RepoDependency {
                dependent: RepoId::new("C"),
                dependency: RepoId::new("B"),
            },
            RepoDependency {
                dependent: RepoId::new("D"),
                dependency: RepoId::new("A"),
            },
        ];
        let graph = CrossRepoGraph::new(repos, dependencies);
        let engine = AuditEngine::new(&graph);
        let audit = engine.audit();

        let a_coupling = audit.coupling.iter().find(|c| c.repo_id == "A").unwrap();
        assert_eq!(a_coupling.direct_dependents, 2);
        assert_eq!(a_coupling.blast_radius, 3); // B, C, D
        assert!(a_coupling.is_critical_path);

        let b_coupling = audit.coupling.iter().find(|c| c.repo_id == "B").unwrap();
        assert_eq!(b_coupling.blast_radius, 1); // C
    }
}
