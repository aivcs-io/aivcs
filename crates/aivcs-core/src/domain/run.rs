//! Run and event tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A single execution of an agent against an AgentSpec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Run {
    /// Unique identifier for this run.
    pub run_id: Uuid,

    /// Digest of the AgentSpec this run executed.
    pub agent_spec_digest: String,

    /// Git commit where execution occurred.
    pub git_sha: String,

    /// When execution started.
    pub started_at: DateTime<Utc>,

    /// When execution finished (None if still running).
    pub finished_at: Option<DateTime<Utc>>,

    /// Current execution status.
    pub status: RunStatus,

    /// Inputs provided to the agent.
    pub inputs: serde_json::Value,

    /// Outputs from the agent (available after completion).
    pub outputs: Option<serde_json::Value>,

    /// Digest of final agent state (for deduplication).
    pub final_state_digest: Option<String>,
}

impl Run {
    /// Create a new run.
    pub fn new(agent_spec_digest: String, git_sha: String, inputs: serde_json::Value) -> Self {
        Self {
            run_id: Uuid::new_v4(),
            agent_spec_digest,
            git_sha,
            started_at: Utc::now(),
            finished_at: None,
            status: RunStatus::Running,
            inputs,
            outputs: None,
            final_state_digest: None,
        }
    }
}

/// Classification of an event in a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// Graph execution started.
    GraphStarted,

    /// Graph execution completed successfully.
    GraphCompleted { iterations: u32, duration_ms: u64 },

    /// Graph execution failed.
    GraphFailed { error: String },

    /// Entered a graph node.
    NodeEntered { node_id: String, iteration: u32 },

    /// Exited a graph node.
    NodeExited {
        node_id: String,
        next_node: Option<String>,
        duration_ms: u64,
    },

    /// Graph node execution failed.
    NodeFailed { node_id: String, error: String },

    /// Tool was called.
    ToolCalled { tool_name: String },

    /// Tool returned a result.
    ToolReturned { tool_name: String },

    /// Tool execution failed.
    ToolFailed { tool_name: String },

    /// Checkpoint marker in execution.
    CheckpointSaved {
        checkpoint_id: String,
        node_id: String,
    },

    /// Checkpoint restored.
    CheckpointRestored {
        checkpoint_id: String,
        node_id: String,
    },

    /// Checkpoint deleted.
    CheckpointDeleted { checkpoint_id: String },

    /// State updated in a node.
    StateUpdated {
        node_id: String,
        keys_changed: Vec<String>,
    },

    /// Message added to execution context.
    MessageAdded { role: String, content_length: usize },

    /// Graph execution interrupted.
    GraphInterrupted { reason: String, node_id: String },

    /// Node execution retrying.
    NodeRetrying {
        node_id: String,
        attempt: u32,
        delay_ms: u64,
    },
}

/// A single event in a run's execution trace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    /// Which run this event belongs to.
    pub run_id: Uuid,

    /// Monotonically increasing sequence number within the run.
    pub seq: u64,

    /// When the event occurred.
    pub timestamp: DateTime<Utc>,

    /// Event classification.
    pub kind: EventKind,

    /// Event-specific payload.
    pub payload: serde_json::Value,
}

impl Event {
    /// Create a new event.
    pub fn new(run_id: Uuid, seq: u64, kind: EventKind, payload: serde_json::Value) -> Self {
        Self {
            run_id,
            seq,
            timestamp: Utc::now(),
            kind,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_serde_roundtrip() {
        let run = Run::new(
            "spec_digest_123".to_string(),
            "git_sha_abc".to_string(),
            serde_json::json!({"question": "What is 2+2?"}),
        );

        let json = serde_json::to_string(&run).expect("serialize");
        let deserialized: Run = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(run, deserialized);
    }

    #[test]
    fn test_run_status_serde() {
        let statuses = [
            (RunStatus::Running, "\"RUNNING\""),
            (RunStatus::Completed, "\"COMPLETED\""),
            (RunStatus::Failed, "\"FAILED\""),
            (RunStatus::Cancelled, "\"CANCELLED\""),
        ];

        for (status, expected_json) in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            assert_eq!(json, *expected_json);
            let deserialized: RunStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*status, deserialized);
        }
    }

    #[test]
    fn test_event_serde_roundtrip_graph_started() {
        let run_id = Uuid::new_v4();
        let event = Event::new(run_id, 1, EventKind::GraphStarted, serde_json::json!({}));

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: Event = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serde_roundtrip_node_entered() {
        let run_id = Uuid::new_v4();
        let event = Event::new(
            run_id,
            1,
            EventKind::NodeEntered {
                node_id: "node_42".to_string(),
                iteration: 1,
            },
            serde_json::json!({"entry_time_ms": 100}),
        );

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: Event = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serde_roundtrip_tool_called() {
        let run_id = Uuid::new_v4();
        let event = Event::new(
            run_id,
            5,
            EventKind::ToolCalled {
                tool_name: "search".to_string(),
            },
            serde_json::json!({"query": "llm models"}),
        );

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: Event = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serde_roundtrip_checkpoint() {
        let run_id = Uuid::new_v4();
        let event = Event::new(
            run_id,
            10,
            EventKind::CheckpointSaved {
                checkpoint_id: "cp123".to_string(),
                node_id: "node_x".to_string(),
            },
            serde_json::json!({"phase": 1, "duration_ms": 5000}),
        );

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: Event = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_run_new_defaults() {
        let inputs = serde_json::json!({"test": "data"});
        let run = Run::new(
            "spec_digest".to_string(),
            "git_sha".to_string(),
            inputs.clone(),
        );

        assert_eq!(run.status, RunStatus::Running);
        assert!(run.finished_at.is_none());
        assert!(run.outputs.is_none());
        assert!(run.final_state_digest.is_none());
        assert_eq!(run.inputs, inputs);
    }
}
