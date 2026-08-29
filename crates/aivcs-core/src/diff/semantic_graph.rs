//! Structural diff for oxidizedgraph-style snapshots embedded in commit state.
//!
//! TODO(stevedores-org/oxidizedgraph#60): swap this JSON structural diff for
//! `oxidizedgraph::diff::Graph::diff` once upstream lands.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Serializable graph + prompt topology extracted from an aivcs state snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphSnapshot {
    pub entry: Option<String>,
    pub exits: Vec<String>,
    pub nodes: BTreeSet<String>,
    pub edges: BTreeSet<(String, String)>,
    pub prompts: BTreeMap<String, String>,
}

impl GraphSnapshot {
    pub fn has_semantic_content(&self) -> bool {
        !self.nodes.is_empty()
            || !self.edges.is_empty()
            || !self.prompts.is_empty()
            || self.entry.is_some()
            || !self.exits.is_empty()
    }
}

/// Delta between two graph snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SemanticGraphDiff {
    pub prompt_changes: BTreeMap<String, PromptChange>,
    pub nodes_added: Vec<String>,
    pub nodes_removed: Vec<String>,
    pub edges_added: Vec<(String, String)>,
    pub edges_removed: Vec<(String, String)>,
    pub entry_changed: Option<(Option<String>, Option<String>)>,
    pub exits_changed: Option<(Vec<String>, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptChange {
    pub before: Option<String>,
    pub after: Option<String>,
}

impl SemanticGraphDiff {
    pub fn is_empty(&self) -> bool {
        self.prompt_changes.is_empty()
            && self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
            && self.entry_changed.is_none()
            && self.exits_changed.is_none()
    }
}

/// Parse a graph snapshot from commit state JSON.
///
/// Supports:
/// - top-level `graph` + `prompts` objects
/// - node objects with inline `prompt` fields
pub fn extract_graph_snapshot(state: &Value) -> GraphSnapshot {
    let mut snapshot = GraphSnapshot::default();

    if let Some(graph) = state.get("graph").and_then(Value::as_object) {
        if let Some(entry) = graph.get("entry").and_then(Value::as_str) {
            snapshot.entry = Some(entry.to_string());
        }
        if let Some(exits) = graph.get("exits").and_then(Value::as_array) {
            snapshot.exits = exits
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        if let Some(nodes) = graph.get("nodes").and_then(Value::as_array) {
            for node in nodes {
                match node {
                    Value::String(id) => {
                        snapshot.nodes.insert(id.clone());
                    }
                    Value::Object(obj) => {
                        if let Some(id) = obj.get("id").and_then(Value::as_str) {
                            snapshot.nodes.insert(id.to_string());
                            if let Some(prompt) = obj.get("prompt").and_then(Value::as_str) {
                                snapshot.prompts.insert(id.to_string(), prompt.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(edges) = graph.get("edges").and_then(Value::as_array) {
            for edge in edges {
                if let Some((from, to)) = parse_edge(edge) {
                    snapshot.edges.insert((from, to));
                }
            }
        }
    }

    if let Some(prompts) = state.get("prompts").and_then(Value::as_object) {
        for (node_id, prompt) in prompts {
            if let Some(text) = prompt.as_str() {
                snapshot.prompts.insert(node_id.clone(), text.to_string());
            }
        }
    }

    snapshot
}

fn parse_edge(edge: &Value) -> Option<(String, String)> {
    if let Some(obj) = edge.as_object() {
        let from = obj
            .get("from")
            .or_else(|| obj.get("source"))
            .and_then(Value::as_str)?;
        let to = obj
            .get("to")
            .or_else(|| obj.get("target"))
            .and_then(Value::as_str)?;
        return Some((from.to_string(), to.to_string()));
    }
    if let Some(arr) = edge.as_array() {
        if arr.len() == 2 {
            let from = arr[0].as_str()?;
            let to = arr[1].as_str()?;
            return Some((from.to_string(), to.to_string()));
        }
    }
    None
}

pub fn diff_graph_snapshots(base: &GraphSnapshot, head: &GraphSnapshot) -> SemanticGraphDiff {
    let mut prompt_changes = BTreeMap::new();
    let node_ids: BTreeSet<_> = base
        .prompts
        .keys()
        .chain(head.prompts.keys())
        .cloned()
        .collect();
    for node_id in node_ids {
        let before = base.prompts.get(&node_id).cloned();
        let after = head.prompts.get(&node_id).cloned();
        if before != after {
            prompt_changes.insert(node_id, PromptChange { before, after });
        }
    }

    let nodes_added: Vec<_> = head.nodes.difference(&base.nodes).cloned().collect();
    let nodes_removed: Vec<_> = base.nodes.difference(&head.nodes).cloned().collect();
    let edges_added: Vec<_> = head.edges.difference(&base.edges).cloned().collect();
    let edges_removed: Vec<_> = base.edges.difference(&head.edges).cloned().collect();

    let entry_changed = if base.entry != head.entry {
        Some((base.entry.clone(), head.entry.clone()))
    } else {
        None
    };
    let exits_changed = if base.exits != head.exits {
        Some((base.exits.clone(), head.exits.clone()))
    } else {
        None
    };

    SemanticGraphDiff {
        prompt_changes,
        nodes_added,
        nodes_removed,
        edges_added,
        edges_removed,
        entry_changed,
        exits_changed,
    }
}

pub fn format_semantic_diff_markdown(diff: &SemanticGraphDiff) -> String {
    if diff.is_empty() {
        return "_No semantic delta — only code/infra changes._".to_string();
    }

    let mut out = String::new();

    if !diff.prompt_changes.is_empty() {
        out.push_str("### Prompt changes\n\n");
        for (node_id, change) in &diff.prompt_changes {
            out.push_str(&format!("**{node_id}**\n\n"));
            out.push_str("```diff\n");
            out.push_str(&format_prompt_diff(change));
            out.push_str("```\n\n");
        }
    }

    if !diff.nodes_added.is_empty()
        || !diff.nodes_removed.is_empty()
        || !diff.edges_added.is_empty()
        || !diff.edges_removed.is_empty()
    {
        out.push_str("### Graph topology\n\n");
        for node in &diff.nodes_added {
            out.push_str(&format!("- Added node `{node}`\n"));
        }
        for node in &diff.nodes_removed {
            out.push_str(&format!("- Removed node `{node}`\n"));
        }
        for (from, to) in &diff.edges_added {
            out.push_str(&format!("- Added edge `{from}` → `{to}`\n"));
        }
        for (from, to) in &diff.edges_removed {
            out.push_str(&format!("- Removed edge `{from}` → `{to}`\n"));
        }
        out.push('\n');
    }

    if let Some((before, after)) = &diff.entry_changed {
        out.push_str("### Entry / exits\n\n");
        out.push_str(&format!(
            "- Entry: `{}` → `{}`\n",
            before.as_deref().unwrap_or("(none)"),
            after.as_deref().unwrap_or("(none)")
        ));
    }
    if let Some((before, after)) = &diff.exits_changed {
        if diff.entry_changed.is_none() {
            out.push_str("### Entry / exits\n\n");
        }
        out.push_str(&format!(
            "- Exits: `{}` → `{}`\n",
            format_exits(before),
            format_exits(after)
        ));
    }

    out.trim_end().to_string()
}

fn format_exits(exits: &[String]) -> String {
    if exits.is_empty() {
        "(none)".to_string()
    } else {
        exits.join(", ")
    }
}

fn format_prompt_diff(change: &PromptChange) -> String {
    let before_lines: Vec<&str> = change.before.as_deref().unwrap_or("").lines().collect();
    let after_lines: Vec<&str> = change.after.as_deref().unwrap_or("").lines().collect();

    let mut out = String::new();
    for line in &before_lines {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in &after_lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if before_lines.is_empty() && after_lines.is_empty() {
        out.push_str("(empty prompt)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_base() -> GraphSnapshot {
        GraphSnapshot {
            entry: Some("start".to_string()),
            exits: vec!["end".to_string()],
            nodes: ["start", "plan", "end"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            edges: [("start", "plan"), ("plan", "end")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            prompts: BTreeMap::from([
                ("plan".to_string(), "Plan carefully.\nStep 1.".to_string()),
                ("end".to_string(), "Finish.".to_string()),
            ]),
        }
    }

    fn sample_head() -> GraphSnapshot {
        GraphSnapshot {
            entry: Some("start".to_string()),
            exits: vec!["end".to_string()],
            nodes: ["start", "plan", "review", "end"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            edges: [("start", "plan"), ("plan", "review"), ("review", "end")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            prompts: BTreeMap::from([
                (
                    "plan".to_string(),
                    "Plan carefully.\nStep 1.\nStep 2.".to_string(),
                ),
                ("review".to_string(), "Review output.".to_string()),
                ("end".to_string(), "Finish.".to_string()),
            ]),
        }
    }

    #[test]
    fn extract_graph_snapshot_from_state_json() {
        let state = json!({
            "graph": {
                "entry": "start",
                "exits": ["end"],
                "nodes": ["start", "plan"],
                "edges": [{"from": "start", "to": "plan"}]
            },
            "prompts": {
                "plan": "Do the thing."
            }
        });
        let snap = extract_graph_snapshot(&state);
        assert_eq!(snap.entry.as_deref(), Some("start"));
        assert!(snap.nodes.contains("plan"));
        assert!(snap
            .edges
            .contains(&("start".to_string(), "plan".to_string())));
        assert_eq!(
            snap.prompts.get("plan").map(String::as_str),
            Some("Do the thing.")
        );
    }

    #[test]
    fn diff_and_markdown_snapshot_matches_fixture() {
        let diff = diff_graph_snapshots(&sample_base(), &sample_head());
        let md = format_semantic_diff_markdown(&diff);

        assert!(md.contains("### Prompt changes"));
        assert!(md.contains("**plan**"));
        assert!(md.contains("+Step 2."));
        assert!(md.contains("### Graph topology"));
        assert!(md.contains("Added node `review`"));
        assert!(md.contains("Added edge `plan` → `review`"));
        assert!(md.contains("Removed edge `plan` → `end`"));
    }

    #[test]
    fn empty_diff_emits_no_delta_message() {
        let snap = sample_base();
        let md = format_semantic_diff_markdown(&diff_graph_snapshots(&snap, &snap));
        assert_eq!(md, "_No semantic delta — only code/infra changes._");
    }
}
