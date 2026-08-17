//! Graph lifecycle adapter: bridges domain `Event` types to `RunLedger` persistence.

use std::sync::Arc;

use oxidized_state::{
    ContentDigest, RunEvent, RunId, RunLedger, RunMetadata, RunSummary, StorageResult,
};

use crate::domain::run::{Event, EventKind};

/// Extract the snake_case kind string from an `EventKind` via its serde tag.
fn event_kind_str(kind: &EventKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v["type"].as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Adapter that records graph lifecycle [`Event`]s into a [`RunLedger`].
///
/// Usage:
/// 1. Call [`GraphRunRecorder::start`] to create a new run.
/// 2. Call [`GraphRunRecorder::record`] for each domain event.
/// 3. Call [`GraphRunRecorder::finish_ok`] or [`GraphRunRecorder::finish_err`] to finalize.
pub struct GraphRunRecorder {
    ledger: Arc<dyn RunLedger>,
    run_id: RunId,
}

impl GraphRunRecorder {
    /// Start a new run in the ledger, returning a recorder bound to that run.
    pub async fn start(
        ledger: Arc<dyn RunLedger>,
        spec_digest: &ContentDigest,
        metadata: RunMetadata,
    ) -> StorageResult<Self> {
        let run_id = ledger.create_run(spec_digest, metadata.clone()).await?;
        crate::obs::emit_run_started(run_id.to_string().as_str(), &metadata.agent_name);
        Ok(Self { ledger, run_id })
    }

    /// Record a single domain event into the ledger.
    pub async fn record(&self, event: &Event) -> StorageResult<()> {
        let kind_str = event_kind_str(&event.kind);

        // Merge fields from EventKind into payload so they are preserved in the ledger.
        // This ensures that tool_name, node_id, etc. are queryable from the payload.
        let mut payload = event.payload.clone();
        if let serde_json::Value::Object(ref mut map) = payload {
            if let Ok(serde_json::Value::Object(kind_map)) = serde_json::to_value(&event.kind) {
                for (k, v) in kind_map {
                    if k != "type" {
                        map.insert(k, v);
                    }
                }
            }
        }

        let run_event = RunEvent {
            seq: event.seq,
            kind: kind_str.clone(),
            payload,
            timestamp: event.timestamp,
        };
        self.ledger.append_event(&self.run_id, run_event).await?;
        crate::obs::emit_event_appended(&self.run_id.to_string(), &kind_str, event.seq);
        Ok(())
    }

    /// Finalize the run as completed.
    pub async fn finish_ok(self, summary: RunSummary) -> StorageResult<()> {
        let duration_ms = summary.duration_ms;
        let total_events = summary.total_events;
        self.ledger.complete_run(&self.run_id, summary).await?;
        crate::obs::emit_run_finished(&self.run_id.to_string(), duration_ms, total_events, true);
        Ok(())
    }

    /// Finalize the run as failed.
    pub async fn finish_err(self, summary: RunSummary) -> StorageResult<()> {
        let duration_ms = summary.duration_ms;
        let total_events = summary.total_events;
        self.ledger.fail_run(&self.run_id, summary).await?;
        crate::obs::emit_run_finished(&self.run_id.to_string(), duration_ms, total_events, false);
        Ok(())
    }

    /// Return a reference to the run ID.
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::{Event, EventKind};
    use oxidized_state::SurrealRunLedger;
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_tool_name_is_preserved_in_ledger() {
        let ledger = Arc::new(SurrealRunLedger::in_memory().await.unwrap());
        let spec_digest = oxidized_state::ContentDigest::from_bytes(b"spec");
        let metadata = oxidized_state::RunMetadata {
            git_sha: None,
            agent_name: "test".to_string(),
            tags: json!({}),
            evaluation: Default::default(),
        };

        let recorder = GraphRunRecorder::start(ledger.clone(), &spec_digest, metadata)
            .await
            .unwrap();
        let run_id = recorder.run_id().clone();

        let event = Event::new(
            Uuid::new_v4(),
            1,
            EventKind::ToolCalled {
                tool_name: "my_tool".to_string(),
            },
            json!({"param": "value"}),
        );

        recorder.record(&event).await.unwrap();

        let events = ledger.get_events(&run_id).await.unwrap();
        assert_eq!(events.len(), 1);
        let recorded_event = &events[0];

        assert_eq!(recorded_event.kind, "tool_called");

        // Regression check: tool_name must be persisted in payload for downstream diffing.
        assert_eq!(
            recorded_event
                .payload
                .get("tool_name")
                .and_then(|v| v.as_str()),
            Some("my_tool"),
            "tool_name missing from recorded payload"
        );
    }
}
