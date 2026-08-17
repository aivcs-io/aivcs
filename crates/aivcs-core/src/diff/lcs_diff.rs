//! Tool-call sequence diffing for run comparison.
//!
//! This module provides structural diffing of tool-call sequences between two runs.
//! It detects added/removed/reordered calls and parameter changes using
//! Longest Common Subsequence (LCS) alignment.

use oxidized_state::storage_traits::RunEvent;
use serde_json::Value;

/// A single tool call extracted from a run's events.
#[derive(Debug, Clone)]
pub struct ToolCallEntry {
    /// Sequence number from the run event
    pub seq: u64,
    /// Tool name extracted from payload["tool_name"]
    pub tool_name: String,
    /// Full event payload
    pub payload: Value,
}

/// A parameter change between two tool call versions.
#[derive(Debug, Clone)]
pub struct ParamChange {
    /// RFC 6901 JSON Pointer path (e.g., "/query", "/context/0")
    pub pointer: String,
    /// Value in run A
    pub value_a: Value,
    /// Value in run B
    pub value_b: Value,
}

/// A single change in the tool-call sequence.
#[derive(Debug, Clone)]
pub enum ToolCallChange {
    /// Tool call added in B but not in A
    Added { entry: ToolCallEntry },
    /// Tool call removed in B (was in A)
    Removed { entry: ToolCallEntry },
    /// Tool call reordered between A and B
    Reordered {
        tool_name: String,
        seq_a: u64,
        seq_b: u64,
    },
    /// Tool call exists in both, but parameters differ
    ParamDelta {
        tool_name: String,
        seq_a: u64,
        seq_b: u64,
        changes: Vec<ParamChange>,
    },
}

/// Summary of differences between two runs' tool-call sequences.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub run_id_a: String,
    pub run_id_b: String,
    pub changes: Vec<ToolCallChange>,
    pub identical: bool,
}

/// Extract tool-call entries from a run's event list.
///
/// Filters for events with `kind == "tool_called"` and extracts the tool name
/// from `payload["tool_name"]`. Returns entries sorted by `seq` (guaranteed by
/// the replay_run contract).
fn extract_tool_calls(events: &[RunEvent]) -> Vec<ToolCallEntry> {
    events
        .iter()
        .filter(|e| e.kind == "tool_called")
        .map(|e| ToolCallEntry {
            seq: e.seq,
            tool_name: e
                .payload
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            payload: e.payload.clone(),
        })
        .collect()
}

/// Compute the Longest Common Subsequence (LCS) of tool names.
///
/// Returns a list of (index_a, index_b) pairs indicating matching positions
/// in the two sequences.
fn lcs_alignment(calls_a: &[ToolCallEntry], calls_b: &[ToolCallEntry]) -> Vec<(usize, usize)> {
    let m = calls_a.len();
    let n = calls_b.len();

    if m == 0 || n == 0 {
        return Vec::new();
    }

    // DP table: dp[i][j] = length of LCS of calls_a[0..i] and calls_b[0..j]
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if calls_a[i - 1].tool_name == calls_b[j - 1].tool_name {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i][j - 1].max(dp[i - 1][j]);
            }
        }
    }

    // Backtrack to find the LCS indices
    let mut alignment = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if calls_a[i - 1].tool_name == calls_b[j - 1].tool_name {
            alignment.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i][j - 1] > dp[i - 1][j] {
            j -= 1;
        } else {
            i -= 1;
        }
    }

    alignment.reverse();
    alignment
}

/// Recursively compute JSON differences.
///
/// Returns a list of pointer paths and their differing values.
fn json_diff(prefix: &str, val_a: &Value, val_b: &Value) -> Vec<ParamChange> {
    if val_a == val_b {
        return Vec::new();
    }

    match (val_a, val_b) {
        (Value::Object(obj_a), Value::Object(obj_b)) => {
            let mut changes = Vec::new();
            let mut keys = std::collections::HashSet::new();
            keys.extend(obj_a.keys().cloned());
            keys.extend(obj_b.keys().cloned());

            for key in keys {
                let val_a_inner = obj_a.get(&key).unwrap_or(&Value::Null);
                let val_b_inner = obj_b.get(&key).unwrap_or(&Value::Null);
                let path = if prefix.is_empty() {
                    format!("/{}", key)
                } else {
                    format!("{}/{}", prefix, key)
                };
                changes.extend(json_diff(&path, val_a_inner, val_b_inner));
            }
            changes
        }
        (Value::Array(arr_a), Value::Array(arr_b)) => {
            let mut changes = Vec::new();
            let max_len = arr_a.len().max(arr_b.len());

            for i in 0..max_len {
                let val_a_inner = arr_a.get(i).unwrap_or(&Value::Null);
                let val_b_inner = arr_b.get(i).unwrap_or(&Value::Null);
                let path = format!("{}/{}", prefix, i);
                changes.extend(json_diff(&path, val_a_inner, val_b_inner));
            }
            changes
        }
        _ => {
            vec![ParamChange {
                pointer: if prefix.is_empty() {
                    "/".to_string()
                } else {
                    prefix.to_string()
                },
                value_a: val_a.clone(),
                value_b: val_b.clone(),
            }]
        }
    }
}

/// Diff the tool-call sequences of two runs.
///
/// # Algorithm
///
/// 1. Extract tool calls (kind="tool_called") from both event sequences
/// 2. Compute LCS alignment on tool names
/// 3. For each index:
///    - Not in LCS → Added or Removed
///    - In LCS with seq mismatch → Reordered
///    - In LCS with payload difference → ParamDelta
///
/// Returns a `DiffSummary` with all changes and an `identical` flag.
pub fn diff_tool_calls(
    run_id_a: &str,
    events_a: &[RunEvent],
    run_id_b: &str,
    events_b: &[RunEvent],
) -> DiffSummary {
    let calls_a = extract_tool_calls(events_a);
    let calls_b = extract_tool_calls(events_b);

    let alignment = lcs_alignment(&calls_a, &calls_b);

    // Build a set of aligned indices for quick lookup
    let mut aligned_a: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut aligned_b: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i_a, i_b) in &alignment {
        aligned_a.insert(*i_a);
        aligned_b.insert(*i_b);
    }

    let mut changes = Vec::new();

    // Handle removed calls (in A but not aligned in B)
    for (i, call) in calls_a.iter().enumerate() {
        if !aligned_a.contains(&i) {
            changes.push(ToolCallChange::Removed {
                entry: call.clone(),
            });
        }
    }

    // Handle parameter changes and reordering for matched calls
    // To detect reordering, compare relative index positions with relative seq positions
    for (idx, (i_a, i_b)) in alignment.iter().enumerate() {
        let call_a = &calls_a[*i_a];
        let call_b = &calls_b[*i_b];

        // Check if this call is reordered by comparing its relative position
        // to the previous matched call
        let is_reordered = if idx > 0 {
            let (prev_i_a, prev_i_b) = alignment[idx - 1];
            let prev_call_a = &calls_a[prev_i_a];
            let prev_call_b = &calls_b[prev_i_b];

            // If the relative index order differs from relative seq order, it's reordered
            // i.e., if i_a < prev_i_a but i_b > prev_i_b (or vice versa)
            (*i_a > prev_i_a) != (call_a.seq > prev_call_a.seq)
                || (*i_b > prev_i_b) != (call_b.seq > prev_call_b.seq)
        } else {
            false
        };

        if is_reordered {
            changes.push(ToolCallChange::Reordered {
                tool_name: call_a.tool_name.clone(),
                seq_a: call_a.seq,
                seq_b: call_b.seq,
            });
        } else {
            // Check for parameter changes
            let param_changes = json_diff("", &call_a.payload, &call_b.payload);
            if !param_changes.is_empty() {
                changes.push(ToolCallChange::ParamDelta {
                    tool_name: call_a.tool_name.clone(),
                    seq_a: call_a.seq,
                    seq_b: call_b.seq,
                    changes: param_changes,
                });
            }
        }
    }

    // Handle added calls (in B but not aligned in A)
    for (i, call) in calls_b.iter().enumerate() {
        if !aligned_b.contains(&i) {
            changes.push(ToolCallChange::Added {
                entry: call.clone(),
            });
        }
    }

    let identical = changes.is_empty();

    DiffSummary {
        run_id_a: run_id_a.to_string(),
        run_id_b: run_id_b.to_string(),
        changes,
        identical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_tool_event(seq: u64, tool_name: &str, extra_payload: Option<Value>) -> RunEvent {
        let mut payload = serde_json::json!({
            "tool_name": tool_name,
        });
        if let Some(extra) = extra_payload {
            if let Value::Object(ref mut obj) = payload {
                if let Value::Object(ref extra_obj) = extra {
                    for (k, v) in extra_obj.iter() {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        RunEvent {
            seq,
            kind: "tool_called".to_string(),
            payload,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_identical_runs_no_diff() {
        let events_a = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];
        let events_b = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];

        let diff = diff_tool_calls("run_a", &events_a, "run_b", &events_b);

        assert!(diff.identical);
        assert!(diff.changes.is_empty());
    }

    #[test]
    fn test_tool_added() {
        let events_a = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];
        let events_b = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "translate", None),
            make_tool_event(3, "fetch", None),
        ];

        let diff = diff_tool_calls("run_a", &events_a, "run_b", &events_b);

        assert!(!diff.identical);
        assert_eq!(diff.changes.len(), 1);

        match &diff.changes[0] {
            ToolCallChange::Added { entry } => {
                assert_eq!(entry.tool_name, "translate");
            }
            other => panic!("Expected Added, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_removed() {
        let events_a = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "translate", None),
            make_tool_event(3, "fetch", None),
        ];
        let events_b = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];

        let diff = diff_tool_calls("run_a", &events_a, "run_b", &events_b);

        assert!(!diff.identical);
        assert_eq!(diff.changes.len(), 1);

        match &diff.changes[0] {
            ToolCallChange::Removed { entry } => {
                assert_eq!(entry.tool_name, "translate");
            }
            other => panic!("Expected Removed, got {:?}", other),
        }
    }

    #[test]
    fn test_param_delta() {
        let events_a = vec![make_tool_event(
            1,
            "search",
            Some(serde_json::json!({"query": "cats"})),
        )];
        let events_b = vec![make_tool_event(
            1,
            "search",
            Some(serde_json::json!({"query": "dogs"})),
        )];

        let diff = diff_tool_calls("run_a", &events_a, "run_b", &events_b);

        assert!(!diff.identical);
        assert_eq!(diff.changes.len(), 1);

        match &diff.changes[0] {
            ToolCallChange::ParamDelta {
                tool_name, changes, ..
            } => {
                assert_eq!(tool_name, "search");
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].pointer, "/query");
            }
            other => panic!("Expected ParamDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_symmetry_property() {
        let events_a = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];
        let events_b = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "translate", None),
            make_tool_event(3, "fetch", None),
        ];

        let diff_ab = diff_tool_calls("run_a", &events_a, "run_b", &events_b);
        let diff_ba = diff_tool_calls("run_b", &events_b, "run_a", &events_a);

        // diff(a,b) should have Added { translate }
        assert!(matches!(&diff_ab.changes[0], ToolCallChange::Added { .. }));

        // diff(b,a) should have Removed { translate }
        assert!(matches!(
            &diff_ba.changes[0],
            ToolCallChange::Removed { .. }
        ));
    }

    #[test]
    fn test_empty_vs_nonempty() {
        let events_a = vec![];
        let events_b = vec![
            make_tool_event(1, "search", None),
            make_tool_event(2, "fetch", None),
        ];

        let diff = diff_tool_calls("run_a", &events_a, "run_b", &events_b);

        assert!(!diff.identical);
        assert_eq!(diff.changes.len(), 2);

        for change in &diff.changes {
            assert!(matches!(change, ToolCallChange::Added { .. }));
        }
    }
}
