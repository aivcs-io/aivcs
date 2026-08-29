use aivcs_core::{diff_node_paths, extract_node_path, NodeStep};
use chrono::Utc;
use oxidized_state::RunEvent;
use serde_json::json;

fn node_event(seq: u64, node_id: &str) -> RunEvent {
    RunEvent {
        seq,
        kind: "node_entered".to_string(),
        payload: json!({ "node_id": node_id }),
        timestamp: Utc::now(),
    }
}

fn non_node_event(seq: u64, kind: &str) -> RunEvent {
    RunEvent {
        seq,
        kind: kind.to_string(),
        payload: json!({ "some_key": "value" }),
        timestamp: Utc::now(),
    }
}

#[test]
fn identical_paths_produce_no_divergence() {
    let events = vec![node_event(1, "A"), node_event(2, "B"), node_event(3, "C")];
    let diff = diff_node_paths(&events, &events);
    assert!(
        diff.is_empty(),
        "identical paths should produce no divergence"
    );
}

#[test]
fn diverges_at_first_step() {
    let a = vec![node_event(1, "A"), node_event(2, "B")];
    let b = vec![node_event(1, "X"), node_event(2, "Y")];
    let diff = diff_node_paths(&a, &b);

    let div = diff.divergence.expect("should have divergence");
    assert!(div.common_prefix.is_empty());
    assert_eq!(div.tail_a.len(), 2);
    assert_eq!(div.tail_b.len(), 2);
    assert_eq!(div.tail_a[0].node_id, "A");
    assert_eq!(div.tail_b[0].node_id, "X");
}

#[test]
fn diverges_mid_path() {
    let a = vec![node_event(1, "A"), node_event(2, "B"), node_event(3, "C")];
    let b = vec![node_event(1, "A"), node_event(2, "B"), node_event(3, "D")];
    let diff = diff_node_paths(&a, &b);

    let div = diff.divergence.expect("should have divergence");
    assert_eq!(div.common_prefix, vec!["A", "B"]);
    assert_eq!(div.tail_a.len(), 1);
    assert_eq!(div.tail_a[0].node_id, "C");
    assert_eq!(div.tail_b.len(), 1);
    assert_eq!(div.tail_b[0].node_id, "D");
}

#[test]
fn path_a_is_prefix_of_b() {
    let a = vec![node_event(1, "A"), node_event(2, "B")];
    let b = vec![node_event(1, "A"), node_event(2, "B"), node_event(3, "C")];
    let diff = diff_node_paths(&a, &b);

    let div = diff.divergence.expect("should have divergence");
    assert_eq!(div.common_prefix, vec!["A", "B"]);
    assert!(div.tail_a.is_empty());
    assert_eq!(div.tail_b.len(), 1);
    assert_eq!(div.tail_b[0].node_id, "C");
}

#[test]
fn path_b_is_prefix_of_a() {
    let a = vec![node_event(1, "A"), node_event(2, "B"), node_event(3, "C")];
    let b = vec![node_event(1, "A"), node_event(2, "B")];
    let diff = diff_node_paths(&a, &b);

    let div = diff.divergence.expect("should have divergence");
    assert_eq!(div.common_prefix, vec!["A", "B"]);
    assert_eq!(div.tail_a.len(), 1);
    assert_eq!(div.tail_a[0].node_id, "C");
    assert!(div.tail_b.is_empty());
}

#[test]
fn empty_paths_produce_no_divergence() {
    let empty: Vec<RunEvent> = vec![];
    let diff = diff_node_paths(&empty, &empty);
    assert!(diff.is_empty(), "empty paths should produce no divergence");
}

#[test]
fn non_node_events_are_filtered_out() {
    let a = vec![
        non_node_event(1, "tool_called"),
        node_event(2, "A"),
        non_node_event(3, "node_exited"),
        node_event(4, "B"),
    ];
    let b = vec![node_event(1, "A"), node_event(2, "B")];
    let diff = diff_node_paths(&a, &b);
    assert!(
        diff.is_empty(),
        "non-node events should be filtered, leaving identical paths"
    );

    // Also verify extract_node_path directly
    let path = extract_node_path(&a);
    assert_eq!(path.len(), 2);
    assert_eq!(path[0].node_id, "A");
    assert_eq!(path[1].node_id, "B");
}

#[test]
fn malformed_node_events_are_skipped() {
    let malformed = RunEvent {
        seq: 1,
        kind: "node_entered".to_string(),
        payload: json!({ "some_key": "no_node_id_here" }),
        timestamp: Utc::now(),
    };
    let valid = node_event(2, "A");

    let a = vec![malformed, valid.clone()];
    let b = vec![valid];
    let diff = diff_node_paths(&a, &b);
    assert!(
        diff.is_empty(),
        "malformed events without node_id should be skipped, got: {:?}",
        diff.divergence
    );

    // Verify extraction skips malformed
    let path = extract_node_path(&a);
    assert_eq!(path.len(), 1);
    assert_eq!(
        path[0],
        NodeStep {
            seq: 2,
            node_id: "A".to_string()
        }
    );
}
