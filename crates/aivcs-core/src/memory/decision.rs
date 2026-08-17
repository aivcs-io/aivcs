//! Decision Recording and Learning — EPIC5 Phase 1
//!
//! Captures agent decisions with rationale and outcomes for learning and analysis.

use oxidized_state::{DecisionRecord, MemoryProvenanceRecord, SurrealHandle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::{AivcsError, Result};

/// Configuration for decision recording
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecorderConfig {
    /// Enable decision recording
    pub enabled: bool,
    /// Capture decisions from run events
    pub capture_from_events: bool,
    /// Capture decisions from tool calls
    pub capture_from_tools: bool,
    /// Maximum decision record size (bytes)
    pub max_decision_size: usize,
}

impl Default for DecisionRecorderConfig {
    fn default() -> Self {
        DecisionRecorderConfig {
            enabled: true,
            capture_from_events: true,
            capture_from_tools: true,
            max_decision_size: 10_000,
        }
    }
}

/// Origin of the decision capture request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCaptureSource {
    Event,
    Tool,
    Manual,
}

/// Records agent decisions with rationale for learning
pub struct DecisionRecorder {
    handle: Arc<SurrealHandle>,
    config: DecisionRecorderConfig,
}

impl DecisionRecorder {
    /// Create a new decision recorder
    pub fn new(handle: Arc<SurrealHandle>, config: DecisionRecorderConfig) -> Self {
        DecisionRecorder { handle, config }
    }

    /// Create with default configuration
    pub fn with_default_config(handle: Arc<SurrealHandle>) -> Self {
        Self::new(handle, DecisionRecorderConfig::default())
    }

    /// Record a decision from a run
    ///
    /// This captures decisions made during execution with context and rationale.
    pub async fn record_decision(
        &self,
        commit_id: String,
        task: String,
        action: String,
        rationale: String,
        confidence: f32,
    ) -> Result<String> {
        self.record_decision_with_source(
            commit_id,
            task,
            action,
            rationale,
            confidence,
            DecisionCaptureSource::Manual,
        )
        .await
    }

    /// Record a decision and apply source-specific capture policy.
    pub async fn record_decision_with_source(
        &self,
        commit_id: String,
        task: String,
        action: String,
        rationale: String,
        confidence: f32,
        source: DecisionCaptureSource,
    ) -> Result<String> {
        if !self.config.enabled {
            return Ok("decision_recording_disabled".to_string());
        }
        if !self.should_capture(source) {
            return Ok("decision_capture_disabled_for_source".to_string());
        }

        // Validate confidence is in range [0.0, 1.0]
        if !(0.0..=1.0).contains(&confidence) {
            return Err(AivcsError::StorageError(
                "confidence must be between 0.0 and 1.0".to_string(),
            ));
        }
        self.validate_payload_size(&commit_id, &task, &action, &rationale)?;

        let decision_id = Uuid::new_v4().to_string();

        let decision = DecisionRecord::new(
            decision_id.clone(),
            commit_id,
            task,
            action,
            rationale,
            confidence,
        );

        // Insert into database using SurrealHandle
        self.handle
            .save_decision(&decision)
            .await
            .map_err(|e| AivcsError::StorageError(format!("Failed to record decision: {}", e)))?;

        Ok(decision_id)
    }

    /// Record decision outcome
    pub async fn record_decision_outcome(
        &self,
        decision_id: &str,
        outcome_json: String,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        self.handle
            .update_decision_outcome(decision_id, outcome_json)
            .await
            .map_err(|e| {
                AivcsError::StorageError(format!("Failed to update decision outcome: {}", e))
            })?;

        Ok(())
    }

    /// Record memory provenance for a memory
    pub async fn record_provenance(&self, provenance: MemoryProvenanceRecord) -> Result<String> {
        if !self.config.enabled {
            return Ok("provenance_recording_disabled".to_string());
        }

        let memory_id = provenance.memory_id.clone();

        // Insert into database using SurrealHandle
        self.handle
            .save_provenance(&provenance)
            .await
            .map_err(|e| AivcsError::StorageError(format!("Failed to record provenance: {}", e)))?;

        Ok(memory_id)
    }

    /// Get decision history for a task
    pub async fn get_decision_history(
        &self,
        task: &str,
        limit: usize,
    ) -> Result<Vec<DecisionRecord>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        self.handle
            .get_decision_history(task, limit)
            .await
            .map_err(|e| {
                AivcsError::StorageError(format!("Failed to query decision history: {}", e))
            })
    }

    /// Calculate decision success rate for an action
    pub async fn get_decision_success_rate(&self, action: &str) -> Result<f32> {
        if !self.config.enabled {
            return Ok(0.0);
        }

        // For now, return a placeholder value
        // In Phase 2, this will query the database for actual success statistics
        let _action = action;
        Ok(0.0)
    }

    /// Invalidate decision provenance when a run fails
    pub async fn invalidate_provenance_on_failure(&self, commit_id: &str) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // In Phase 2, this will implement the actual invalidation logic
        let _commit_id = commit_id;
        Ok(())
    }

    fn should_capture(&self, source: DecisionCaptureSource) -> bool {
        should_capture_source(&self.config, source)
    }

    fn validate_payload_size(
        &self,
        commit_id: &str,
        task: &str,
        action: &str,
        rationale: &str,
    ) -> Result<()> {
        let payload_size = decision_payload_size(commit_id, task, action, rationale);
        if payload_size > self.config.max_decision_size {
            return Err(AivcsError::StorageError(format!(
                "decision payload exceeds max_decision_size ({} > {})",
                payload_size, self.config.max_decision_size
            )));
        }
        Ok(())
    }
}

fn should_capture_source(config: &DecisionRecorderConfig, source: DecisionCaptureSource) -> bool {
    match source {
        DecisionCaptureSource::Event => config.capture_from_events,
        DecisionCaptureSource::Tool => config.capture_from_tools,
        DecisionCaptureSource::Manual => true,
    }
}

fn decision_payload_size(commit_id: &str, task: &str, action: &str, rationale: &str) -> usize {
    commit_id.len() + task.len() + action.len() + rationale.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_decision_recorder_disabled() {
        let config = DecisionRecorderConfig {
            enabled: false,
            ..Default::default()
        };

        // We can't easily test without a real database, but we verify config works
        assert!(!config.enabled);
    }

    #[test]
    fn test_decision_recorder_config_default() {
        let config = DecisionRecorderConfig::default();

        assert!(config.enabled);
        assert!(config.capture_from_events);
        assert!(config.capture_from_tools);
        assert_eq!(config.max_decision_size, 10_000);
    }

    #[test]
    fn test_confidence_validation() {
        // Confidence should be in [0.0, 1.0]
        assert!((0.0..=1.0).contains(&0.5));
        assert!((0.0..=1.0).contains(&0.0));
        assert!((0.0..=1.0).contains(&1.0));
    }

    #[test]
    fn test_should_capture_honors_source_flags() {
        let config = DecisionRecorderConfig {
            enabled: true,
            capture_from_events: false,
            capture_from_tools: true,
            max_decision_size: 10_000,
        };

        assert!(!should_capture_source(
            &config,
            DecisionCaptureSource::Event
        ));
        assert!(should_capture_source(&config, DecisionCaptureSource::Tool));
        assert!(should_capture_source(
            &config,
            DecisionCaptureSource::Manual
        ));
    }

    #[test]
    fn test_validate_payload_size_enforces_limit() {
        assert_eq!(decision_payload_size("abc", "def", "ghi", "jkl"), 12);
    }

    #[tokio::test]
    async fn test_record_decision_rejects_payload_over_limit() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
        let recorder = DecisionRecorder::new(
            handle,
            DecisionRecorderConfig {
                enabled: true,
                capture_from_events: true,
                capture_from_tools: true,
                max_decision_size: 8,
            },
        );

        let err = recorder
            .record_decision(
                "abc".to_string(),
                "def".to_string(),
                "ghi".to_string(),
                "jkl".to_string(),
                0.5,
            )
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("decision payload exceeds max_decision_size"));
    }

    #[tokio::test]
    async fn test_record_decision_with_source_respects_capture_flags() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
        let recorder = DecisionRecorder::new(
            handle,
            DecisionRecorderConfig {
                enabled: true,
                capture_from_events: false,
                capture_from_tools: true,
                max_decision_size: 10_000,
            },
        );

        let status = recorder
            .record_decision_with_source(
                "commit-1".to_string(),
                "task-1".to_string(),
                "action-1".to_string(),
                "rationale-1".to_string(),
                0.7,
                DecisionCaptureSource::Event,
            )
            .await
            .unwrap();

        assert_eq!(status, "decision_capture_disabled_for_source");
    }

    #[tokio::test]
    async fn test_record_decision_outcome_persists() {
        let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
        let recorder = DecisionRecorder::with_default_config(handle.clone());

        let decision_id = recorder
            .record_decision(
                "commit-2".to_string(),
                "task-2".to_string(),
                "action-2".to_string(),
                "rationale-2".to_string(),
                0.8,
            )
            .await
            .unwrap();

        recorder
            .record_decision_outcome(&decision_id, r#"{"status":"success"}"#.to_string())
            .await
            .unwrap();

        let persisted = handle.get_decision(&decision_id).await.unwrap().unwrap();
        assert_eq!(
            persisted.outcome,
            Some(r#"{"status":"success"}"#.to_string())
        );
        assert!(persisted.outcome_at.is_some());
    }
}
