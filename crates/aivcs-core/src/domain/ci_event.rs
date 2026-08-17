//! CI lifecycle event types for run tracking and provenance.
//!
//! Events are emitted during CI run execution and persisted via
//! the EventBus → RunLedger pipeline. Each event references CAS
//! digests for bulky payloads rather than inlining them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Classification of a CI lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CIEventKind {
    /// CI run has started execution.
    RunStarted { run_id: Uuid, stages: Vec<String> },

    /// A CI stage has begun executing.
    StageStarted { run_id: Uuid, stage: String },

    /// A CI stage has finished execution.
    StageFinished {
        run_id: Uuid,
        stage: String,
        passed: bool,
        duration_ms: u64,
        cache_hit: bool,
    },

    /// CI run has finished (all stages complete or aborted).
    RunFinished {
        run_id: Uuid,
        passed: bool,
        total_duration_ms: u64,
    },

    /// Diagnostics have been produced for a run.
    DiagnosticsProduced {
        run_id: Uuid,
        diagnostics_digest: String,
        count: u32,
    },

    /// A repair plan has been generated.
    RepairPlanned {
        run_id: Uuid,
        plan_digest: String,
        patch_count: u32,
    },

    /// A patch has been applied from a repair plan.
    PatchApplied {
        run_id: Uuid,
        patch_digest: String,
        changed_paths: Vec<String>,
    },

    /// Verification rerun has completed.
    VerificationFinished {
        run_id: Uuid,
        verification_run_id: Uuid,
        passed: bool,
    },

    /// A promotion gate has been evaluated.
    GateEvaluated {
        run_id: Uuid,
        gate_id: String,
        passed: bool,
        violations_count: u32,
    },
}

/// A single CI lifecycle event in a run's execution trace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CIEvent {
    /// Monotonically increasing sequence number within the run.
    pub seq: u64,

    /// When the event occurred.
    pub timestamp: DateTime<Utc>,

    /// Event classification and payload.
    pub kind: CIEventKind,

    /// Optional additional metadata.
    pub metadata: serde_json::Value,
}

impl CIEvent {
    /// Create a new CI event.
    pub fn new(seq: u64, kind: CIEventKind) -> Self {
        Self {
            seq,
            timestamp: Utc::now(),
            kind,
            metadata: serde_json::json!({}),
        }
    }

    /// Attach metadata to the event.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ci_event_kind_run_started_serde() {
        let kind = CIEventKind::RunStarted {
            run_id: Uuid::new_v4(),
            stages: vec!["fmt".to_string(), "test".to_string()],
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
        assert!(json.contains("\"type\":\"run_started\""));
    }

    #[test]
    fn test_ci_event_kind_stage_finished_serde() {
        let kind = CIEventKind::StageFinished {
            run_id: Uuid::new_v4(),
            stage: "clippy".to_string(),
            passed: true,
            duration_ms: 4500,
            cache_hit: false,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_run_finished_serde() {
        let kind = CIEventKind::RunFinished {
            run_id: Uuid::new_v4(),
            passed: false,
            total_duration_ms: 12000,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_diagnostics_produced_serde() {
        let kind = CIEventKind::DiagnosticsProduced {
            run_id: Uuid::new_v4(),
            diagnostics_digest: "abc123def456".to_string(),
            count: 5,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_repair_planned_serde() {
        let kind = CIEventKind::RepairPlanned {
            run_id: Uuid::new_v4(),
            plan_digest: "plan-digest-789".to_string(),
            patch_count: 2,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_patch_applied_serde() {
        let kind = CIEventKind::PatchApplied {
            run_id: Uuid::new_v4(),
            patch_digest: "patch-abc".to_string(),
            changed_paths: vec!["src/main.rs".to_string()],
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_verification_finished_serde() {
        let kind = CIEventKind::VerificationFinished {
            run_id: Uuid::new_v4(),
            verification_run_id: Uuid::new_v4(),
            passed: true,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_kind_gate_evaluated_serde() {
        let kind = CIEventKind::GateEvaluated {
            run_id: Uuid::new_v4(),
            gate_id: "promote-gate-1".to_string(),
            passed: false,
            violations_count: 3,
        };
        let json = serde_json::to_string(&kind).expect("serialize");
        let deserialized: CIEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, deserialized);
    }

    #[test]
    fn test_ci_event_serde_roundtrip() {
        let event = CIEvent::new(
            1,
            CIEventKind::StageStarted {
                run_id: Uuid::new_v4(),
                stage: "test".to_string(),
            },
        );

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: CIEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_ci_event_with_metadata() {
        let event = CIEvent::new(
            0,
            CIEventKind::RunStarted {
                run_id: Uuid::new_v4(),
                stages: vec!["fmt".to_string()],
            },
        )
        .with_metadata(serde_json::json!({"trigger": "manual"}));

        assert_eq!(event.metadata["trigger"], "manual");
    }

    #[test]
    fn test_ci_event_defaults() {
        let event = CIEvent::new(
            0,
            CIEventKind::RunStarted {
                run_id: Uuid::new_v4(),
                stages: vec![],
            },
        );
        assert_eq!(event.seq, 0);
        assert_eq!(event.metadata, serde_json::json!({}));
    }
}
