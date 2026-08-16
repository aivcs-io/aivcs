//! Repair plan and patch commit types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strategy for automated repair.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairStrategy {
    /// Automatically apply fixes (e.g. `cargo fmt`).
    AutoFix,
    /// Suggest fixes without applying.
    Suggest,
    /// Skip repair entirely.
    Skip,
}

/// A single file patch proposed by a repair plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatchCommit {
    /// Path to the file being patched (relative to workspace root).
    pub file_path: String,

    /// Unified diff content.
    pub diff: String,

    /// Human-readable description of the change.
    pub description: String,
}

/// A bounded repair plan for a CI run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepairPlan {
    /// Run ID this repair targets.
    pub run_id: Uuid,

    /// Repair strategy.
    pub strategy: RepairStrategy,

    /// Proposed file patches.
    pub patches: Vec<PatchCommit>,

    /// Maximum allowed repair attempts.
    pub max_attempts: u32,

    /// Current attempt number (0-indexed).
    pub current_attempt: u32,
}

impl RepairPlan {
    /// Create a new repair plan.
    pub fn new(run_id: Uuid, strategy: RepairStrategy, max_attempts: u32) -> Self {
        Self {
            run_id,
            strategy,
            patches: Vec::new(),
            max_attempts,
            current_attempt: 0,
        }
    }

    /// Add a patch to the plan.
    pub fn with_patch(mut self, patch: PatchCommit) -> Self {
        self.patches.push(patch);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_strategy_serde() {
        let strategies = [
            RepairStrategy::AutoFix,
            RepairStrategy::Suggest,
            RepairStrategy::Skip,
        ];
        for s in &strategies {
            let json = serde_json::to_string(s).expect("serialize");
            let deserialized: RepairStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*s, deserialized);
        }
    }

    #[test]
    fn test_repair_plan_serde_roundtrip() {
        let run_id = Uuid::new_v4();
        let plan = RepairPlan::new(run_id, RepairStrategy::AutoFix, 3).with_patch(PatchCommit {
            file_path: "src/main.rs".to_string(),
            diff: "--- a/src/main.rs\n+++ b/src/main.rs\n".to_string(),
            description: "Fix formatting".to_string(),
        });

        let json = serde_json::to_string(&plan).expect("serialize");
        let deserialized: RepairPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plan, deserialized);
    }

    #[test]
    fn test_repair_plan_new_defaults() {
        let plan = RepairPlan::new(Uuid::new_v4(), RepairStrategy::Suggest, 5);
        assert!(plan.patches.is_empty());
        assert_eq!(plan.current_attempt, 0);
        assert_eq!(plan.max_attempts, 5);
    }

    #[test]
    fn test_patch_commit_serde_roundtrip() {
        let patch = PatchCommit {
            file_path: "lib.rs".to_string(),
            diff: "+use std::io;".to_string(),
            description: "Add import".to_string(),
        };

        let json = serde_json::to_string(&patch).expect("serialize");
        let deserialized: PatchCommit = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(patch, deserialized);
    }
}
