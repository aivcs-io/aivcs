//! Mesh-wide relationship graph: heterogeneous typed nodes and edges across the
//! data-mesh — issues, PRs, CI runs, checks, artifacts, agents, commits
//! (aivcs.io#290). Generalizes the issue-only [`crate::graph`] so the AX/UI and
//! agents can traverse "issue → PR → CI run → checks → agent" as one graph.
//!
//! Pure domain logic (no storage, no HTTP): stores persist [`MeshEdge`] rows as
//! native mesh graph relations, servers assemble [`MeshGraphView`]s. The issue
//! graph is the specialization where every node is an [`NodeKind::Issue`].

use crate::graph::{EdgeKind, IssueRef};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// The kind of entity a [`MeshRef`] addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    Issue,
    Pr,
    CiRun,
    Check,
    Artifact,
    Agent,
    Commit,
    /// A running k8s service — the target of og-crab live tests (og-crab#26).
    Service,
    /// A security/quality finding produced by a run.
    Finding,
    /// A measured metric (e.g. a perf/SLO result) produced by a run.
    Metric,
}

impl NodeKind {
    /// The wire/kebab token, e.g. `ci-run`.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Issue => "issue",
            NodeKind::Pr => "pr",
            NodeKind::CiRun => "ci-run",
            NodeKind::Check => "check",
            NodeKind::Artifact => "artifact",
            NodeKind::Agent => "agent",
            NodeKind::Commit => "commit",
            NodeKind::Service => "service",
            NodeKind::Finding => "finding",
            NodeKind::Metric => "metric",
        }
    }
}

impl FromStr for NodeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "issue" => NodeKind::Issue,
            "pr" => NodeKind::Pr,
            "ci-run" => NodeKind::CiRun,
            "check" => NodeKind::Check,
            "artifact" => NodeKind::Artifact,
            "agent" => NodeKind::Agent,
            "commit" => NodeKind::Commit,
            "service" => NodeKind::Service,
            "finding" => NodeKind::Finding,
            "metric" => NodeKind::Metric,
            other => return Err(format!("unknown node kind '{other}'")),
        })
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A reference to any node on the mesh, addressed as `kind:id`.
///
/// `id` is kind-specific but always a stable, opaque identity: issues/PRs use
/// `owner/repo#number`, ci-runs the run id, agents the agent name, commits
/// `owner/repo@sha`, checks/artifacts their record id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeshRef {
    pub kind: NodeKind,
    pub id: String,
}

impl MeshRef {
    pub fn new(kind: NodeKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }

    pub fn issue(r: &IssueRef) -> Self {
        Self::new(NodeKind::Issue, r.to_string())
    }
    pub fn pr(repo: impl AsRef<str>, number: u64) -> Self {
        Self::new(NodeKind::Pr, format!("{}#{}", repo.as_ref(), number))
    }
    pub fn ci_run(id: impl Into<String>) -> Self {
        Self::new(NodeKind::CiRun, id)
    }
    pub fn agent(name: impl Into<String>) -> Self {
        Self::new(NodeKind::Agent, name)
    }
    pub fn commit(repo: impl AsRef<str>, sha: impl AsRef<str>) -> Self {
        Self::new(
            NodeKind::Commit,
            format!("{}@{}", repo.as_ref(), sha.as_ref()),
        )
    }
    /// A running service, addressed by its discovery `serviceId` (og-crab#26).
    pub fn service(service_id: impl Into<String>) -> Self {
        Self::new(NodeKind::Service, service_id)
    }
    pub fn finding(id: impl Into<String>) -> Self {
        Self::new(NodeKind::Finding, id)
    }
    pub fn metric(id: impl Into<String>) -> Self {
        Self::new(NodeKind::Metric, id)
    }
}

impl fmt::Display for MeshRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.id)
    }
}

impl FromStr for MeshRef {
    type Err = String;
    /// Parse `kind:id`. The `id` may itself contain `:` (e.g. a URL); only the
    /// first `:` is the kind separator.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, id) = s
            .split_once(':')
            .ok_or_else(|| format!("mesh ref '{s}' must look like kind:id"))?;
        if id.is_empty() {
            return Err(format!("mesh ref '{s}' has an empty id"));
        }
        Ok(Self {
            kind: kind.parse()?,
            id: id.to_string(),
        })
    }
}

impl From<IssueRef> for MeshRef {
    fn from(r: IssueRef) -> Self {
        MeshRef::issue(&r)
    }
}

/// A typed relationship on the mesh, read as `from <kind> to`. Extends the
/// issue-graph vocabulary with CI/agent relations. Passive `*-by` forms (and
/// `child`) canonicalize to their active inverse so each relationship has one
/// stored row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeshEdgeKind {
    // ── issue-graph relations (carried over for a unified vocabulary) ──
    Blocks,
    BlockedBy,
    Relates,
    Parent,
    Child,
    // ── CI / agent relations ──
    /// pr → issue: the PR implements the issue.
    Implements,
    ImplementedBy,
    /// ci-run → pr: the run verifies the PR's checks.
    Verifies,
    VerifiedBy,
    /// ci-run → artifact: the run produced the artifact.
    Produces,
    ProducedBy,
    /// commit → ci-run: the commit triggered the run.
    Triggers,
    TriggeredBy,
    /// agent → pr/commit: the agent authored the work.
    Authored,
    AuthoredBy,
    /// check → ci-run: a (HITL) check gates the run.
    Gates,
    GatedBy,
    // ── live-testing relations (og-crab#26) ──
    /// ci-run → service: the run live-tested (probed) the service.
    Probes,
    ProbedBy,
    /// ci-run → finding: the run surfaced the finding.
    Found,
    FoundBy,
    /// ci-run → metric: the run recorded the metric (perf/SLO result).
    Measured,
    MeasuredBy,
}

impl MeshEdgeKind {
    /// The same relationship read from the other endpoint.
    pub fn inverse(self) -> Self {
        use MeshEdgeKind::*;
        match self {
            Blocks => BlockedBy,
            BlockedBy => Blocks,
            Relates => Relates,
            Parent => Child,
            Child => Parent,
            Implements => ImplementedBy,
            ImplementedBy => Implements,
            Verifies => VerifiedBy,
            VerifiedBy => Verifies,
            Produces => ProducedBy,
            ProducedBy => Produces,
            Triggers => TriggeredBy,
            TriggeredBy => Triggers,
            Authored => AuthoredBy,
            AuthoredBy => Authored,
            Gates => GatedBy,
            GatedBy => Gates,
            Probes => ProbedBy,
            ProbedBy => Probes,
            Found => FoundBy,
            FoundBy => Found,
            Measured => MeasuredBy,
            MeasuredBy => Measured,
        }
    }

    /// Whether this kind is stored as-is. The passive `*-by` forms and `child`
    /// are stored as their active inverse with endpoints swapped.
    pub fn is_canonical(self) -> bool {
        use MeshEdgeKind::*;
        !matches!(
            self,
            BlockedBy
                | Child
                | ImplementedBy
                | VerifiedBy
                | ProducedBy
                | TriggeredBy
                | AuthoredBy
                | GatedBy
                | ProbedBy
                | FoundBy
                | MeasuredBy
        )
    }

    /// `relates` is the only symmetric relation.
    pub fn is_symmetric(self) -> bool {
        matches!(self, MeshEdgeKind::Relates)
    }
}

impl From<EdgeKind> for MeshEdgeKind {
    fn from(k: EdgeKind) -> Self {
        match k {
            EdgeKind::Blocks => MeshEdgeKind::Blocks,
            EdgeKind::BlockedBy => MeshEdgeKind::BlockedBy,
            EdgeKind::Relates => MeshEdgeKind::Relates,
            EdgeKind::Parent => MeshEdgeKind::Parent,
            EdgeKind::Child => MeshEdgeKind::Child,
        }
    }
}

impl fmt::Display for MeshEdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use MeshEdgeKind::*;
        let s = match self {
            Blocks => "blocks",
            BlockedBy => "blocked-by",
            Relates => "relates",
            Parent => "parent",
            Child => "child",
            Implements => "implements",
            ImplementedBy => "implemented-by",
            Verifies => "verifies",
            VerifiedBy => "verified-by",
            Produces => "produces",
            ProducedBy => "produced-by",
            Triggers => "triggers",
            TriggeredBy => "triggered-by",
            Authored => "authored",
            AuthoredBy => "authored-by",
            Gates => "gates",
            GatedBy => "gated-by",
            Probes => "probes",
            ProbedBy => "probed-by",
            Found => "found",
            FoundBy => "found-by",
            Measured => "measured",
            MeasuredBy => "measured-by",
        };
        f.write_str(s)
    }
}

/// A typed, directed relationship between two mesh nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshEdge {
    pub id: Uuid,
    pub from: MeshRef,
    pub kind: MeshEdgeKind,
    pub to: MeshRef,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

impl MeshEdge {
    /// Build a canonical edge: passive `*-by`/`child` kinds are flipped so
    /// exactly one row represents each relationship regardless of which end
    /// declared it. Rejects self-links.
    pub fn canonical(
        from: MeshRef,
        kind: MeshEdgeKind,
        to: MeshRef,
        created_by: impl Into<String>,
    ) -> Result<Self, String> {
        if from == to {
            return Err(format!("node {from} cannot link to itself"));
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

    /// True when this edge represents the same relationship as `other` (for
    /// idempotent link creation). Symmetric kinds match either orientation.
    pub fn same_relationship(&self, other: &Self) -> bool {
        if self.kind != other.kind {
            return false;
        }
        if self.from == other.from && self.to == other.to {
            return true;
        }
        self.kind.is_symmetric() && self.from == other.to && self.to == other.from
    }
}

/// A node in a rendered mesh graph: the reference plus a display label and an
/// optional status (e.g. issue state, run status, check conclusion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshNode {
    #[serde(flatten)]
    pub node: MeshRef,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// The assembled heterogeneous graph a mesh traversal returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeshGraphView {
    pub nodes: Vec<MeshNode>,
    pub edges: Vec<MeshEdge>,
}

/// Would adding `candidate` (assumed canonical) close a cycle among edges of
/// the same `kind`? Only meaningful for acyclic relations (e.g. `Blocks`,
/// `Parent`). Follows `kind` edges from `candidate.to`; a cycle exists if
/// `candidate.from` is reachable.
pub fn would_create_cycle(edges: &[MeshEdge], candidate: &MeshEdge) -> bool {
    let kind = candidate.kind;
    if kind.is_symmetric() {
        return false;
    }
    let mut stack = vec![&candidate.to];
    let mut seen: HashSet<&MeshRef> = HashSet::new();
    while let Some(node) = stack.pop() {
        if *node == candidate.from {
            return true;
        }
        if !seen.insert(node) {
            continue;
        }
        for e in edges.iter().filter(|e| e.kind == kind) {
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

    fn m(s: &str) -> MeshRef {
        s.parse().unwrap()
    }

    #[test]
    fn parses_and_displays_mesh_refs() {
        let r = m("ci-run:run-2f9c");
        assert_eq!(r.kind, NodeKind::CiRun);
        assert_eq!(r.id, "run-2f9c");
        assert_eq!(r.to_string(), "ci-run:run-2f9c");

        // id may contain colons (e.g. a log/URL id) — only the first splits.
        let a = m("artifact:s3://bucket/x");
        assert_eq!(a.kind, NodeKind::Artifact);
        assert_eq!(a.id, "s3://bucket/x");

        assert!("nope:x".parse::<MeshRef>().is_err()); // unknown kind
        assert!("agent:".parse::<MeshRef>().is_err()); // empty id
        assert!("no-colon".parse::<MeshRef>().is_err());
    }

    #[test]
    fn issue_ref_bridges_into_mesh() {
        let ir: IssueRef = "lornu-ai/aivcs.io#284".parse().unwrap();
        let mr: MeshRef = ir.clone().into();
        assert_eq!(mr.kind, NodeKind::Issue);
        assert_eq!(mr.to_string(), "issue:lornu-ai/aivcs.io#284");
        assert_eq!(
            MeshEdgeKind::from(EdgeKind::BlockedBy),
            MeshEdgeKind::BlockedBy
        );
    }

    #[test]
    fn canonicalizes_passive_kinds() {
        // ci-run verified-by pr  ⇒  pr? no: Verifies is ci-run → pr. VerifiedBy
        // (pr → ci-run) canonicalizes to Verifies (ci-run → pr).
        let e = MeshEdge::canonical(
            MeshRef::pr("lornu-ai/aivcs.io", 284),
            MeshEdgeKind::VerifiedBy,
            MeshRef::ci_run("run-1"),
            "u",
        )
        .unwrap();
        assert_eq!(e.kind, MeshEdgeKind::Verifies);
        assert_eq!(e.from, MeshRef::ci_run("run-1"));
        assert_eq!(e.to, MeshRef::pr("lornu-ai/aivcs.io", 284));

        // authored-by (pr → agent) ⇒ Authored (agent → pr)
        let e = MeshEdge::canonical(
            MeshRef::pr("lornu-ai/aivcs.io", 284),
            MeshEdgeKind::AuthoredBy,
            MeshRef::agent("sso-auth-agent-4"),
            "u",
        )
        .unwrap();
        assert_eq!(e.kind, MeshEdgeKind::Authored);
        assert_eq!(e.from, MeshRef::agent("sso-auth-agent-4"));

        // active form is kept as-is
        let e = MeshEdge::canonical(
            MeshRef::ci_run("run-1"),
            MeshEdgeKind::Produces,
            MeshRef::new(NodeKind::Artifact, "a1"),
            "u",
        )
        .unwrap();
        assert_eq!(e.kind, MeshEdgeKind::Produces);
        assert_eq!(e.from, MeshRef::ci_run("run-1"));
    }

    #[test]
    fn every_kind_has_a_stable_inverse() {
        use MeshEdgeKind::*;
        for k in [
            Blocks,
            BlockedBy,
            Relates,
            Parent,
            Child,
            Implements,
            ImplementedBy,
            Verifies,
            VerifiedBy,
            Produces,
            ProducedBy,
            Triggers,
            TriggeredBy,
            Authored,
            AuthoredBy,
            Gates,
            GatedBy,
            Probes,
            ProbedBy,
            Found,
            FoundBy,
            Measured,
            MeasuredBy,
        ] {
            assert_eq!(k.inverse().inverse(), k, "{k} inverse must round-trip");
            // exactly one of a kind / its inverse is canonical (except symmetric)
            if !k.is_symmetric() {
                assert_ne!(k.is_canonical(), k.inverse().is_canonical(), "{k}");
            }
        }
    }

    #[test]
    fn rejects_self_links() {
        assert!(MeshEdge::canonical(
            MeshRef::ci_run("r1"),
            MeshEdgeKind::Relates,
            MeshRef::ci_run("r1"),
            "u",
        )
        .is_err());
    }

    #[test]
    fn detects_cycles_for_directed_kinds() {
        // issue A blocks B blocks C; C blocks A closes a cycle.
        let ab = MeshEdge::canonical(
            m("issue:o/a#1"),
            MeshEdgeKind::Blocks,
            m("issue:o/b#1"),
            "u",
        )
        .unwrap();
        let bc = MeshEdge::canonical(
            m("issue:o/b#1"),
            MeshEdgeKind::Blocks,
            m("issue:o/c#1"),
            "u",
        )
        .unwrap();
        let edges = vec![ab, bc];

        let closes = MeshEdge::canonical(
            m("issue:o/c#1"),
            MeshEdgeKind::Blocks,
            m("issue:o/a#1"),
            "u",
        )
        .unwrap();
        assert!(would_create_cycle(&edges, &closes));

        let fine = MeshEdge::canonical(
            m("issue:o/a#1"),
            MeshEdgeKind::Blocks,
            m("issue:o/c#1"),
            "u",
        )
        .unwrap();
        assert!(!would_create_cycle(&edges, &fine));

        // symmetric relations never "cycle"
        let rel = MeshEdge::canonical(
            m("issue:o/c#1"),
            MeshEdgeKind::Relates,
            m("issue:o/a#1"),
            "u",
        )
        .unwrap();
        assert!(!would_create_cycle(&edges, &rel));
    }

    #[test]
    fn same_relationship_dedup() {
        let a =
            MeshEdge::canonical(m("ci-run:r1"), MeshEdgeKind::Relates, m("pr:o/a#1"), "u").unwrap();
        let b =
            MeshEdge::canonical(m("pr:o/a#1"), MeshEdgeKind::Relates, m("ci-run:r1"), "u").unwrap();
        assert!(a.same_relationship(&b), "relates is symmetric");

        let c = MeshEdge::canonical(m("ci-run:r1"), MeshEdgeKind::Verifies, m("pr:o/a#1"), "u")
            .unwrap();
        let d = MeshEdge::canonical(m("pr:o/a#1"), MeshEdgeKind::Verifies, m("ci-run:r1"), "u")
            .unwrap();
        assert!(!c.same_relationship(&d), "verifies is directional");
    }

    #[test]
    fn og_crab_live_testing_tracking() {
        // The og-crab#26 graph model: a live run probes a service, and surfaces
        // findings + metrics — tracked as canonical MeshEdges.
        let run = MeshRef::ci_run("live-2f9c");
        let svc = MeshRef::service("aivcs-api");
        let finding = MeshRef::finding("tls-weak-cipher");
        let metric = MeshRef::metric("p99-latency");
        assert_eq!(svc.to_string(), "service:aivcs-api");

        let probes =
            MeshEdge::canonical(run.clone(), MeshEdgeKind::Probes, svc.clone(), "og-crab").unwrap();
        assert_eq!(probes.kind, MeshEdgeKind::Probes);
        assert_eq!(probes.from, run);
        assert_eq!(probes.to, svc);

        // Passive form canonicalizes: service found-by run ⇒ run found finding.
        let found = MeshEdge::canonical(
            finding.clone(),
            MeshEdgeKind::FoundBy,
            run.clone(),
            "og-crab",
        )
        .unwrap();
        assert_eq!(found.kind, MeshEdgeKind::Found);
        assert_eq!(found.from, run);
        assert_eq!(found.to, finding);

        let measured = MeshEdge::canonical(
            run.clone(),
            MeshEdgeKind::Measured,
            metric.clone(),
            "og-crab",
        )
        .unwrap();
        assert_eq!(measured.kind, MeshEdgeKind::Measured);
        assert_eq!(measured.to, metric);

        // Round-trips through the wire form.
        assert_eq!("service:aivcs-api".parse::<MeshRef>().unwrap(), svc);
        assert_eq!(
            "finding:tls-weak-cipher".parse::<MeshRef>().unwrap(),
            finding
        );
    }
}
