use oxidized_state::RunEvent;
use serde_json::Value;

/// A single delta at an RFC 6901 JSON pointer path between two states.
#[derive(Debug, Clone, PartialEq)]
pub struct StateDelta {
    /// RFC 6901 JSON pointer, e.g. `"/memory/0/context"`.
    pub pointer: String,
    /// Value in A (`Null` if absent).
    pub before: Value,
    /// Value in B (`Null` if absent).
    pub after: Value,
}

/// The result of diffing two states at scoped JSON pointer paths.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedStateDiff {
    pub deltas: Vec<StateDelta>,
}

impl ScopedStateDiff {
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Event kind emitted by the oxidizedgraph event adapter when a checkpoint is saved.
pub const CHECKPOINT_SAVED_KIND: &str = "checkpoint_saved";

/// Extract the payload of the last `"CheckpointSaved"` event from a run event stream.
///
/// Returns `None` if no checkpoint-saved events exist.
pub fn extract_last_checkpoint(events: &[RunEvent]) -> Option<Value> {
    events
        .iter()
        .rev()
        .find(|e| e.kind == CHECKPOINT_SAVED_KIND)
        .map(|e| e.payload.clone())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Diff two JSON values at the given RFC 6901 JSON pointer paths.
///
/// For each pointer, resolves the value in both `a` and `b`. If they differ
/// (including one being absent while the other is present), a `StateDelta` is
/// emitted. Pointers where both values are identical or both absent are skipped.
pub fn diff_scoped_state(a: &Value, b: &Value, pointers: &[&str]) -> ScopedStateDiff {
    let deltas = pointers
        .iter()
        .filter_map(|ptr| {
            let val_a = a.pointer(ptr).unwrap_or(&Value::Null);
            let val_b = b.pointer(ptr).unwrap_or(&Value::Null);

            if val_a == val_b {
                None
            } else {
                Some(StateDelta {
                    pointer: (*ptr).to_string(),
                    before: val_a.clone(),
                    after: val_b.clone(),
                })
            }
        })
        .collect();

    ScopedStateDiff { deltas }
}

/// Convenience: extract last checkpoint state from two event streams and diff
/// at the given JSON pointer paths.
///
/// Returns an empty diff if either stream has no checkpoint events.
pub fn diff_run_states(a: &[RunEvent], b: &[RunEvent], pointers: &[&str]) -> ScopedStateDiff {
    let state_a = extract_last_checkpoint(a).unwrap_or(Value::Null);
    let state_b = extract_last_checkpoint(b).unwrap_or(Value::Null);
    diff_scoped_state(&state_a, &state_b, pointers)
}
