//! Event payload validation for AIVCS run events.
//!
//! Validates that a `RunEvent` has a known, non-empty kind and that all
//! required payload fields are present for structured event kinds.
//!
//! `Custom:*` prefixed events bypass field checking — their schema is
//! caller-defined.

use oxidized_state::storage_traits::RunEvent;

use super::error::ValidationError;

/// All structured event kinds produced by `map_event()` in `event_adapter.rs`.
pub const KNOWN_EVENT_KINDS: &[&str] = &[
    "graph_started",
    "graph_completed",
    "graph_failed",
    "graph_interrupted",
    "node_entered",
    "node_exited",
    "node_failed",
    "node_retrying",
    "tool_called",
    "tool_returned",
    "tool_failed",
    "checkpoint_saved",
    "checkpoint_restored",
    "checkpoint_deleted",
    "state_updated",
    "message_added",
];

/// Required payload fields per structured event kind.
///
/// Kinds not listed here require no specific fields.
pub const REQUIRED_PAYLOAD_FIELDS: &[(&str, &[&str])] = &[
    ("graph_started", &["graph_name", "entry_point"]),
    ("graph_completed", &["iterations", "duration_ms"]),
    ("graph_failed", &["error"]),
    ("graph_interrupted", &["reason", "node_id"]),
    ("node_entered", &["node_id", "iteration"]),
    ("node_exited", &["node_id"]),
    ("node_failed", &["node_id", "error"]),
    ("node_retrying", &["node_id", "attempt"]),
    ("tool_called", &["tool_name"]),
    ("tool_returned", &["tool_name"]),
    ("tool_failed", &["tool_name"]),
    ("checkpoint_saved", &["checkpoint_id", "node_id"]),
    ("checkpoint_restored", &["checkpoint_id", "node_id"]),
    ("checkpoint_deleted", &["checkpoint_id"]),
    ("state_updated", &["node_id"]),
    ("message_added", &["role"]),
];

/// Validate a `RunEvent`.
///
/// Checks:
/// 1. `kind` is non-empty.
/// 2. If the kind is a known structured kind (not a `Custom:` prefix), all
///    required payload fields are present.
/// 3. Unknown non-custom kinds are rejected.
///
/// # Errors
///
/// - `ValidationError::EmptyKind` — `event.kind` is empty.
/// - `ValidationError::UnknownEventKind` — kind is not in `KNOWN_EVENT_KINDS`
///   and does not start with `"Custom:"`.
/// - `ValidationError::MissingPayloadField` — a required payload field is absent.
pub fn validate_run_event(event: &RunEvent) -> Result<(), ValidationError> {
    if event.kind.is_empty() {
        return Err(ValidationError::EmptyKind);
    }

    // Custom events bypass field validation
    if event.kind.starts_with("Custom:") {
        return Ok(());
    }

    // Reject unknown structured kinds
    if !KNOWN_EVENT_KINDS.contains(&event.kind.as_str()) {
        return Err(ValidationError::UnknownEventKind {
            kind: event.kind.clone(),
        });
    }

    // Check required fields
    if let Some((_, required)) = REQUIRED_PAYLOAD_FIELDS
        .iter()
        .find(|(k, _)| *k == event.kind.as_str())
    {
        for &field in *required {
            if event.payload.get(field).is_none() {
                return Err(ValidationError::MissingPayloadField {
                    kind: event.kind.clone(),
                    field: field.to_string(),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_event(kind: &str, payload: serde_json::Value) -> RunEvent {
        RunEvent {
            seq: 1,
            kind: kind.to_string(),
            payload,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_valid_node_entered_passes() {
        let event = make_event(
            "node_entered",
            serde_json::json!({ "node_id": "n1", "iteration": 1 }),
        );
        assert!(validate_run_event(&event).is_ok());
    }

    #[test]
    fn test_node_entered_missing_node_id_fails() {
        let event = make_event("node_entered", serde_json::json!({ "iteration": 1 }));
        let err = validate_run_event(&event).unwrap_err();
        match err {
            ValidationError::MissingPayloadField { kind, field } => {
                assert_eq!(kind, "node_entered");
                assert_eq!(field, "node_id");
            }
            other => panic!("Expected MissingPayloadField, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_kind_rejected() {
        let event = make_event("BogusEvent", serde_json::json!({}));
        let err = validate_run_event(&event).unwrap_err();
        match err {
            ValidationError::UnknownEventKind { kind } => {
                assert_eq!(kind, "BogusEvent");
            }
            other => panic!("Expected UnknownEventKind, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_prefix_passes_without_field_check() {
        let event = make_event("Custom:MyEvent", serde_json::json!({}));
        assert!(validate_run_event(&event).is_ok());
    }

    #[test]
    fn test_empty_kind_rejected() {
        let event = make_event("", serde_json::json!({}));
        let err = validate_run_event(&event).unwrap_err();
        assert!(matches!(err, ValidationError::EmptyKind));
    }

    #[test]
    fn test_valid_checkpoint_saved_passes() {
        let event = make_event(
            "checkpoint_saved",
            serde_json::json!({ "checkpoint_id": "cp1", "node_id": "n1" }),
        );
        assert!(validate_run_event(&event).is_ok());
    }

    #[test]
    fn test_checkpoint_saved_missing_node_id_fails() {
        let event = make_event(
            "checkpoint_saved",
            serde_json::json!({ "checkpoint_id": "cp1" }),
        );
        let err = validate_run_event(&event).unwrap_err();
        match err {
            ValidationError::MissingPayloadField { kind, field } => {
                assert_eq!(kind, "checkpoint_saved");
                assert_eq!(field, "node_id");
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn test_valid_tool_called_passes() {
        let event = make_event("tool_called", serde_json::json!({ "tool_name": "search" }));
        assert!(validate_run_event(&event).is_ok());
    }
}
