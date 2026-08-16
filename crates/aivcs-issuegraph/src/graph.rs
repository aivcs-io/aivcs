//! Cross-issue dependency graph: typed edges between issues, addressed as
//! `owner/repo#number` — the relationship layer that turns hosted issues into
//! an issuegraph.
//!
//! This module is pure domain logic (no storage, no HTTP): stores in
//! `aivcs-api` persist [`IssueEdge`] rows, servers assemble [`GraphView`]s,
//! and the oxide forge renders them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// A reference to an issue anywhere on the forge: `owner/repo#number`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueRef {
    /// Repository slug, `owner/name`.
    pub repo: String,
    /// Per-repo issue number.
    pub number: u64,
}

impl IssueRef {
    pub fn new(repo: impl Into<String>, number: u64) -> Self {
        Self {
            repo: repo.into(),
            number,
        }
    }

    /// Canonical URL path on the forge, e.g. `/lornu-ai/oci-builds/issues/95`.
    pub fn url_path(&self) -> String {
        format!("/{}/issues/{}", self.repo, self.number)
    }
}

impl fmt::Display for IssueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

impl FromStr for IssueRef {
    type Err = String;

    /// Parse `owner/repo#number`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (repo, num) = s
            .rsplit_once('#')
            .ok_or_else(|| format!("issue ref '{s}' must look like owner/repo#number"))?;
        let mut parts = repo.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(o), Some(r), None) if !o.is_empty() && !r.is_empty() => {}
            _ => return Err(format!("issue ref '{s}' must have an owner/repo slug")),
        }
        let number: u64 = num
            .parse()
            .map_err(|_| format!("issue ref '{s}' has a non-numeric issue number"))?;
        if number == 0 {
            return Err(format!("issue ref '{s}' — issue numbers are 1-based"));
        }
        Ok(Self {
            repo: repo.to_string(),
            number,
        })
    }
}

/// The relationship an edge asserts, read as `from <kind> to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// `from` blocks `to` — `to` cannot complete until `from` does.
    Blocks,
    /// `from` is blocked by `to`. Canonicalized to `Blocks` (reversed).
    BlockedBy,
    /// Loose association, symmetric.
    Relates,
    /// `from` is the parent (epic/tracking issue) of `to`.
    Parent,
    /// `from` is a child of `to`. Canonicalized to `Parent` (reversed).
    Child,
}

impl EdgeKind {
    /// The same relationship read from the other endpoint.
    pub fn inverse(self) -> Self {
        match self {
            EdgeKind::Blocks => EdgeKind::BlockedBy,
            EdgeKind::BlockedBy => EdgeKind::Blocks,
            EdgeKind::Relates => EdgeKind::Relates,
            EdgeKind::Parent => EdgeKind::Child,
            EdgeKind::Child => EdgeKind::Parent,
        }
    }

    /// Whether this kind is stored as-is; `BlockedBy`/`Child` are stored as
    /// their inverse with the endpoints swapped so each relationship has one
    /// canonical row.
    pub fn is_canonical(self) -> bool {
        !matches!(self, EdgeKind::BlockedBy | EdgeKind::Child)
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            EdgeKind::Blocks => "blocks",
            EdgeKind::BlockedBy => "blocked-by",
            EdgeKind::Relates => "relates",
            EdgeKind::Parent => "parent",
            EdgeKind::Child => "child",
        };
        f.write_str(s)
    }
}

/// A typed, directed relationship between two issues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueEdge {
    pub id: Uuid,
    pub from: IssueRef,
    pub kind: EdgeKind,
    pub to: IssueRef,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

impl IssueEdge {
    /// Build a canonical edge: `blocked-by`/`child` are flipped so exactly one
    /// row represents each relationship regardless of which end declared it.
    pub fn canonical(
        from: IssueRef,
        kind: EdgeKind,
        to: IssueRef,
        created_by: impl Into<String>,
    ) -> Result<Self, String> {
        if from == to {
            return Err(format!("issue {from} cannot link to itself"));
        }
        let (from, kind, to) = if kind.is_canonical() {
            (from, kind, to)
        } else {
            (to, kind.inverse(), from)
        };
        Ok(Self {
            id: Uuid::new_v4(),
            from,
            kind,
            to,
            created_by: created_by.into(),
            created_at: Utc::now(),
        })
    }

    /// True when this edge represents the same relationship as `other`
    /// (used for idempotent link creation). `Relates` is symmetric.
    pub fn same_relationship(&self, other: &Self) -> bool {
        if self.kind != other.kind {
            return false;
        }
        if self.from == other.from && self.to == other.to {
            return true;
        }
        self.kind == EdgeKind::Relates && self.from == other.to && self.to == other.from
    }
}

/// A node in a rendered graph: the issue reference plus display summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueNode {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub title: String,
    pub state: String,
}

/// The assembled dependency graph for a repo, org, or single issue's
/// neighborhood: what API clients receive and the forge renders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphView {
    pub nodes: Vec<IssueNode>,
    pub edges: Vec<IssueEdge>,
}

/// Would adding `candidate` (assumed canonical) close a `blocks` cycle?
///
/// Follows existing `Blocks` edges from `candidate.to` — if `candidate.from`
/// is reachable, the new edge would make an issue transitively block itself.
pub fn would_create_blocks_cycle(edges: &[IssueEdge], candidate: &IssueEdge) -> bool {
    if candidate.kind != EdgeKind::Blocks {
        return false;
    }
    let mut stack = vec![&candidate.to];
    let mut seen: HashSet<&IssueRef> = HashSet::new();
    while let Some(node) = stack.pop() {
        if *node == candidate.from {
            return true;
        }
        if !seen.insert(node) {
            continue;
        }
        for e in edges.iter().filter(|e| e.kind == EdgeKind::Blocks) {
            if e.from == *node {
                stack.push(&e.to);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> IssueRef {
        s.parse().unwrap()
    }

    #[test]
    fn parses_and_displays_issue_refs() {
        let re = r("lornu-ai/oci-builds#95");
        assert_eq!(re.repo, "lornu-ai/oci-builds");
        assert_eq!(re.number, 95);
        assert_eq!(re.to_string(), "lornu-ai/oci-builds#95");
        assert_eq!(re.url_path(), "/lornu-ai/oci-builds/issues/95");

        assert!("no-slash#1".parse::<IssueRef>().is_err());
        assert!("a/b/c#1".parse::<IssueRef>().is_err());
        assert!("a/b#zero".parse::<IssueRef>().is_err());
        assert!("a/b#0".parse::<IssueRef>().is_err());
        assert!("a/b".parse::<IssueRef>().is_err());
    }

    #[test]
    fn canonicalizes_inverse_kinds() {
        let e = IssueEdge::canonical(r("o/a#1"), EdgeKind::BlockedBy, r("o/b#2"), "u").unwrap();
        assert_eq!(e.kind, EdgeKind::Blocks);
        assert_eq!(e.from, r("o/b#2"));
        assert_eq!(e.to, r("o/a#1"));

        let e = IssueEdge::canonical(r("o/a#1"), EdgeKind::Child, r("o/b#2"), "u").unwrap();
        assert_eq!(e.kind, EdgeKind::Parent);
        assert_eq!(e.from, r("o/b#2"));

        let e = IssueEdge::canonical(r("o/a#1"), EdgeKind::Blocks, r("o/b#2"), "u").unwrap();
        assert_eq!(e.kind, EdgeKind::Blocks);
        assert_eq!(e.from, r("o/a#1"));
    }

    #[test]
    fn rejects_self_links() {
        assert!(IssueEdge::canonical(r("o/a#1"), EdgeKind::Relates, r("o/a#1"), "u").is_err());
    }

    #[test]
    fn relates_is_symmetric_for_dedup() {
        let a = IssueEdge::canonical(r("o/a#1"), EdgeKind::Relates, r("o/b#2"), "u").unwrap();
        let b = IssueEdge::canonical(r("o/b#2"), EdgeKind::Relates, r("o/a#1"), "u").unwrap();
        assert!(a.same_relationship(&b));

        let c = IssueEdge::canonical(r("o/a#1"), EdgeKind::Blocks, r("o/b#2"), "u").unwrap();
        let d = IssueEdge::canonical(r("o/b#2"), EdgeKind::Blocks, r("o/a#1"), "u").unwrap();
        assert!(!c.same_relationship(&d), "blocks is directional");
    }

    #[test]
    fn detects_blocks_cycles_transitively() {
        let ab = IssueEdge::canonical(r("o/a#1"), EdgeKind::Blocks, r("o/b#1"), "u").unwrap();
        let bc = IssueEdge::canonical(r("o/b#1"), EdgeKind::Blocks, r("o/c#1"), "u").unwrap();
        let edges = vec![ab, bc];

        let closes = IssueEdge::canonical(r("o/c#1"), EdgeKind::Blocks, r("o/a#1"), "u").unwrap();
        assert!(would_create_blocks_cycle(&edges, &closes));

        let fine = IssueEdge::canonical(r("o/a#1"), EdgeKind::Blocks, r("o/c#1"), "u").unwrap();
        assert!(!would_create_blocks_cycle(&edges, &fine));

        let relates = IssueEdge::canonical(r("o/c#1"), EdgeKind::Relates, r("o/a#1"), "u").unwrap();
        assert!(!would_create_blocks_cycle(&edges, &relates));
    }
}
