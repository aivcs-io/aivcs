//! Pause / edit / continue intervention controls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An operator intervention on a running pipeline or checkpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intervention {
    /// Unique identifier for this intervention.
    pub intervention_id: String,
    /// The run affected by this intervention.
    pub run_id: Uuid,
    /// Checkpoint that triggered the intervention, if any.
    pub checkpoint_id: Option<String>,
    /// Who initiated the intervention.
    pub operator: String,
    /// The type of intervention.
    pub action: InterventionAction,
    /// When the intervention was initiated.
    pub initiated_at: DateTime<Utc>,
    /// When the intervention was resolved (resumed/completed).
    pub resolved_at: Option<DateTime<Utc>>,
    /// Operator-supplied notes.
    pub notes: Option<String>,
}

/// Types of operator intervention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionAction {
    /// Pause execution at this point.
    Pause,
    /// Edit parameters or state before continuing.
    Edit {
        /// Description of what was changed.
        change_summary: String,
    },
    /// Resume execution after a pause or edit.
    Continue,
    /// Abort the run entirely.
    Abort { reason: String },
}

impl InterventionAction {
    /// Whether this action pauses execution.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::Pause | Self::Edit { .. })
    }

    /// Whether this action resumes execution.
    pub fn is_resume(&self) -> bool {
        matches!(self, Self::Continue)
    }

    /// Whether this action terminates execution.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abort { .. })
    }
}

impl Intervention {
    /// Create a new intervention.
    pub fn new(
        run_id: Uuid,
        checkpoint_id: Option<String>,
        operator: impl Into<String>,
        action: InterventionAction,
        notes: Option<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let resolved_at = if action.is_resume() || action.is_terminal() {
            Some(now)
        } else {
            None
        };
        Self {
            intervention_id: Uuid::new_v4().to_string(),
            run_id,
            checkpoint_id,
            operator: operator.into(),
            action,
            initiated_at: now,
            resolved_at,
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intervention_action_blocking() {
        assert!(InterventionAction::Pause.is_blocking());
        assert!(InterventionAction::Edit {
            change_summary: "x".into()
        }
        .is_blocking());
        assert!(!InterventionAction::Continue.is_blocking());
        assert!(!InterventionAction::Abort {
            reason: "done".into()
        }
        .is_blocking());
    }

    #[test]
    fn test_intervention_action_resume() {
        assert!(InterventionAction::Continue.is_resume());
        assert!(!InterventionAction::Pause.is_resume());
    }

    #[test]
    fn test_intervention_action_terminal() {
        assert!(InterventionAction::Abort {
            reason: "done".into()
        }
        .is_terminal());
        assert!(!InterventionAction::Pause.is_terminal());
    }

    #[test]
    fn test_new_pause_not_resolved() {
        let iv = Intervention::new(
            Uuid::new_v4(),
            None,
            "alice",
            InterventionAction::Pause,
            None,
            Utc::now(),
        );
        assert!(iv.resolved_at.is_none());
    }

    #[test]
    fn test_new_continue_auto_resolved() {
        let iv = Intervention::new(
            Uuid::new_v4(),
            None,
            "alice",
            InterventionAction::Continue,
            None,
            Utc::now(),
        );
        assert!(iv.resolved_at.is_some());
    }

    #[test]
    fn test_serde_roundtrip() {
        let iv = Intervention::new(
            Uuid::new_v4(),
            Some("cp-1".into()),
            "bob",
            InterventionAction::Edit {
                change_summary: "changed timeout".into(),
            },
            Some("adjusted from 30s to 60s".into()),
            Utc::now(),
        );
        let json = serde_json::to_string(&iv).unwrap();
        let back: Intervention = serde_json::from_str(&json).unwrap();
        assert_eq!(iv, back);
    }
}
