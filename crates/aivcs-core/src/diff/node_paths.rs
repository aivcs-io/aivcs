use oxidized_state::RunEvent;

/// A single node step extracted from a `RunEvent` stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStep {
    pub seq: u64,
    pub node_id: String,
}

/// The divergence point between two node traversal paths.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDivergence {
    /// Node IDs that both paths share before the first mismatch.
    pub common_prefix: Vec<String>,
    /// Remaining steps only in path A after the divergence point.
    pub tail_a: Vec<NodeStep>,
    /// Remaining steps only in path B after the divergence point.
    pub tail_b: Vec<NodeStep>,
}

/// The result of diffing two node traversal paths.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePathDiff {
    pub divergence: Option<NodeDivergence>,
}

impl NodePathDiff {
    pub fn is_empty(&self) -> bool {
        self.divergence.is_none()
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Extract the ordered node traversal path from a run event stream.
///
/// Only `"node_entered"` events are considered. Events without a valid
/// `payload["node_id"]` string are silently skipped.
pub fn extract_node_path(events: &[RunEvent]) -> Vec<NodeStep> {
    events
        .iter()
        .filter(|e| e.kind == "node_entered")
        .filter_map(|e| {
            let node_id = e
                .payload
                .get("node_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())?;
            Some(NodeStep {
                seq: e.seq,
                node_id,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Diff two ordered `RunEvent` sequences by their node traversal paths.
///
/// Extracts `"node_entered"` events from each sequence, then walks both
/// paths in lockstep to find the first divergence point. Returns
/// `NodePathDiff { divergence: None }` when the paths are identical.
pub fn diff_node_paths(a: &[RunEvent], b: &[RunEvent]) -> NodePathDiff {
    let path_a = extract_node_path(a);
    let path_b = extract_node_path(b);

    let mut common_prefix = Vec::new();
    let mut i = 0;

    while i < path_a.len() && i < path_b.len() {
        if path_a[i].node_id == path_b[i].node_id {
            common_prefix.push(path_a[i].node_id.clone());
            i += 1;
        } else {
            break;
        }
    }

    if i == path_a.len() && i == path_b.len() {
        NodePathDiff { divergence: None }
    } else {
        NodePathDiff {
            divergence: Some(NodeDivergence {
                common_prefix,
                tail_a: path_a[i..].to_vec(),
                tail_b: path_b[i..].to_vec(),
            }),
        }
    }
}
