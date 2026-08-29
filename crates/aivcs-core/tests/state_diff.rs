use aivcs_core::{
    diff_run_states, diff_scoped_state, extract_last_checkpoint, StateDelta, CHECKPOINT_SAVED_KIND,
};
use chrono::Utc;
use oxidized_state::RunEvent;
use serde_json::{json, Value};

fn checkpoint_event(seq: u64, payload: Value) -> RunEvent {
    RunEvent {
        seq,
        kind: CHECKPOINT_SAVED_KIND.to_string(),
        payload,
        timestamp: Utc::now(),
    }
}

fn non_checkpoint_event(seq: u64, kind: &str) -> RunEvent {
    RunEvent {
        seq,
        kind: kind.to_string(),
        payload: json!({"irrelevant": true}),
        timestamp: Utc::now(),
    }
}

#[test]
fn identical_states_produce_no_diff() {
    let state = json!({"model": "gpt-4", "config": {"retries": 3}});
    let diff = diff_scoped_state(&state, &state, &["/model", "/config/retries"]);
    assert!(diff.is_empty(), "identical states should produce no deltas");
}

#[test]
fn single_pointer_value_changed() {
    let a = json!({"model": "gpt-4", "temperature": 0.7});
    let b = json!({"model": "gpt-4", "temperature": 0.9});
    let diff = diff_scoped_state(&a, &b, &["/model", "/temperature"]);

    assert_eq!(diff.deltas.len(), 1);
    assert_eq!(
        diff.deltas[0],
        StateDelta {
            pointer: "/temperature".to_string(),
            before: json!(0.7),
            after: json!(0.9),
        }
    );
}

#[test]
fn nested_pointer_value_changed() {
    let a = json!({"config": {"retries": 3, "timeout": 30}});
    let b = json!({"config": {"retries": 5, "timeout": 30}});
    let diff = diff_scoped_state(&a, &b, &["/config/retries", "/config/timeout"]);

    assert_eq!(diff.deltas.len(), 1);
    assert_eq!(diff.deltas[0].pointer, "/config/retries");
    assert_eq!(diff.deltas[0].before, json!(3));
    assert_eq!(diff.deltas[0].after, json!(5));
}

#[test]
fn pointer_exists_only_in_a() {
    let a = json!({"model": "gpt-4", "deprecated_field": true});
    let b = json!({"model": "gpt-4"});
    let diff = diff_scoped_state(&a, &b, &["/deprecated_field"]);

    assert_eq!(diff.deltas.len(), 1);
    assert_eq!(diff.deltas[0].before, json!(true));
    assert_eq!(diff.deltas[0].after, Value::Null);
}

#[test]
fn pointer_exists_only_in_b() {
    let a = json!({"model": "gpt-4"});
    let b = json!({"model": "gpt-4", "new_field": "added"});
    let diff = diff_scoped_state(&a, &b, &["/new_field"]);

    assert_eq!(diff.deltas.len(), 1);
    assert_eq!(diff.deltas[0].before, Value::Null);
    assert_eq!(diff.deltas[0].after, json!("added"));
}

#[test]
fn multiple_pointers_mixed_changes() {
    let a = json!({"x": 1, "y": 2, "z": 3});
    let b = json!({"x": 1, "y": 99, "z": 3, "w": 4});

    let diff = diff_scoped_state(&a, &b, &["/x", "/y", "/z", "/w"]);

    assert_eq!(diff.deltas.len(), 2);
    // /y changed
    assert!(diff
        .deltas
        .iter()
        .any(|d| d.pointer == "/y" && d.before == json!(2) && d.after == json!(99)));
    // /w added
    assert!(diff
        .deltas
        .iter()
        .any(|d| d.pointer == "/w" && d.before == Value::Null && d.after == json!(4)));
}

#[test]
fn empty_pointers_produce_no_diff() {
    let a = json!({"model": "gpt-4"});
    let b = json!({"model": "gpt-5"});
    let diff = diff_scoped_state(&a, &b, &[]);
    assert!(diff.is_empty(), "no pointers means no deltas");
}

#[test]
fn both_absent_pointers_produce_no_diff() {
    let a = json!({"x": 1});
    let b = json!({"y": 2});
    let diff = diff_scoped_state(&a, &b, &["/missing"]);
    assert!(
        diff.is_empty(),
        "pointer absent in both should produce no delta"
    );
}

#[test]
fn extract_last_checkpoint_uses_final_event() {
    let events = vec![
        checkpoint_event(1, json!({"phase": 1})),
        non_checkpoint_event(2, "node_entered"),
        checkpoint_event(3, json!({"phase": 2})),
    ];

    let state = extract_last_checkpoint(&events).expect("should find checkpoint");
    assert_eq!(state, json!({"phase": 2}));
}

#[test]
fn extract_last_checkpoint_returns_none_when_absent() {
    let events = vec![
        non_checkpoint_event(1, "node_entered"),
        non_checkpoint_event(2, "tool_called"),
    ];
    assert!(extract_last_checkpoint(&events).is_none());
}

#[test]
fn diff_run_states_end_to_end() {
    let a = vec![
        non_checkpoint_event(1, "node_entered"),
        checkpoint_event(2, json!({"model": "gpt-4", "steps": 10})),
    ];
    let b = vec![
        non_checkpoint_event(1, "node_entered"),
        checkpoint_event(2, json!({"model": "gpt-4", "steps": 15})),
    ];

    let diff = diff_run_states(&a, &b, &["/model", "/steps"]);
    assert_eq!(diff.deltas.len(), 1);
    assert_eq!(diff.deltas[0].pointer, "/steps");
    assert_eq!(diff.deltas[0].before, json!(10));
    assert_eq!(diff.deltas[0].after, json!(15));
}

#[test]
fn diff_run_states_no_checkpoints_produces_empty_diff() {
    let a = vec![non_checkpoint_event(1, "node_entered")];
    let b = vec![non_checkpoint_event(1, "node_entered")];
    let diff = diff_run_states(&a, &b, &["/anything"]);
    assert!(
        diff.is_empty(),
        "no checkpoints in either stream should produce empty diff"
    );
}
