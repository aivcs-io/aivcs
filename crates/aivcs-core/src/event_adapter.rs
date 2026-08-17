//! Adapter bridging oxidizedgraph's `EventBus` lifecycle events into the
//! AIVCS `RunLedger` persistence layer.
//!
//! The [`LedgerHandler`] implements oxidizedgraph's [`EventHandler`] trait,
//! mapping each [`Event`] to a [`RunEvent`] and persisting it via
//! [`RunLedger::append_event`].
//!
//! # Usage
//!
//! ```ignore
//! use aivcs_core::event_adapter::subscribe_ledger_to_bus;
//!
//! let bus = Arc::new(EventBus::new());
//! let ledger = Arc::new(MemoryRunLedger::new());
//! let handler = subscribe_ledger_to_bus(
//!     &bus,
//!     ledger,
//!     spec_digest,
//!     metadata,
//! ).await;
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{instrument, warn};

use crate::metrics::METRICS;

use oxidizedgraph::events::{
    spawn_handler, CheckpointEvent, Event, EventBus, EventHandler, EventKind, GraphEvent,
    NodeEvent, StateEvent,
};

use oxidized_state::storage_traits::{
    ContentDigest, RunEvent, RunId, RunLedger, RunMetadata, RunSummary,
};

/// Implements oxidizedgraph's `EventHandler` to persist graph lifecycle
/// events into an AIVCS `RunLedger`.
///
/// The handler:
/// - Creates a run on `on_start()`
/// - Maps each `Event` to a `RunEvent` and appends it on `handle()`
/// - Completes or fails the run on `on_stop()` based on whether errors occurred
pub struct LedgerHandler<L: RunLedger> {
    ledger: Arc<L>,
    run_id: RwLock<Option<RunId>>,
    seq: AtomicU64,
    spec_digest: ContentDigest,
    metadata: RunMetadata,
    saw_error: AtomicBool,
    start_time: RwLock<Option<std::time::Instant>>,
}

impl<L: RunLedger> LedgerHandler<L> {
    /// Create a new handler that will persist events to the given ledger.
    pub fn new(ledger: Arc<L>, spec_digest: ContentDigest, metadata: RunMetadata) -> Self {
        Self {
            ledger,
            run_id: RwLock::new(None),
            seq: AtomicU64::new(1),
            spec_digest,
            metadata,
            saw_error: AtomicBool::new(false),
            start_time: RwLock::new(None),
        }
    }

    /// Get the run ID (available after `on_start` is called).
    pub async fn run_id(&self) -> Option<RunId> {
        self.run_id.read().await.clone()
    }

    /// Whether any error events have been observed.
    pub fn saw_error(&self) -> bool {
        self.saw_error.load(Ordering::SeqCst)
    }

    /// Current sequence number.
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Bump the sequence counter and return the new value.
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }
}

/// Map an oxidizedgraph `Event` into a `(kind, payload)` pair for `RunEvent`.
fn map_event(event: &Event) -> (String, serde_json::Value) {
    match &event.kind {
        EventKind::Graph(g) => match g {
            GraphEvent::Started {
                graph_name,
                entry_point,
            } => (
                "graph_started".into(),
                json!({ "graph_name": graph_name, "entry_point": entry_point }),
            ),
            GraphEvent::Completed {
                iterations,
                duration_ms,
            } => (
                "graph_completed".into(),
                json!({ "iterations": iterations, "duration_ms": duration_ms }),
            ),
            GraphEvent::Error { error } => ("graph_failed".into(), json!({ "error": error })),
            GraphEvent::Interrupted { reason, node_id } => (
                "graph_interrupted".into(),
                json!({ "reason": reason, "node_id": node_id }),
            ),
        },
        EventKind::Node(n) => match n {
            NodeEvent::Entered { node_id, iteration } => (
                "node_entered".into(),
                json!({ "node_id": node_id, "iteration": iteration }),
            ),
            NodeEvent::Exited {
                node_id,
                next_node,
                duration_ms,
            } => (
                "node_exited".into(),
                json!({ "node_id": node_id, "next_node": next_node, "duration_ms": duration_ms }),
            ),
            NodeEvent::Error { node_id, error } => (
                "node_failed".into(),
                json!({ "node_id": node_id, "error": error }),
            ),
            NodeEvent::Retrying {
                node_id,
                attempt,
                delay_ms,
            } => (
                "node_retrying".into(),
                json!({ "node_id": node_id, "attempt": attempt, "delay_ms": delay_ms }),
            ),
        },
        EventKind::Checkpoint(c) => match c {
            CheckpointEvent::Saved {
                checkpoint_id,
                node_id,
            } => (
                "checkpoint_saved".into(),
                json!({ "checkpoint_id": checkpoint_id, "node_id": node_id }),
            ),
            CheckpointEvent::Restored {
                checkpoint_id,
                node_id,
            } => (
                "checkpoint_restored".into(),
                json!({ "checkpoint_id": checkpoint_id, "node_id": node_id }),
            ),
            CheckpointEvent::Deleted { checkpoint_id } => (
                "checkpoint_deleted".into(),
                json!({ "checkpoint_id": checkpoint_id }),
            ),
        },
        EventKind::State(s) => match s {
            StateEvent::Updated {
                node_id,
                keys_changed,
            } => (
                "state_updated".into(),
                json!({ "node_id": node_id, "keys_changed": keys_changed }),
            ),
            StateEvent::MessageAdded {
                role,
                content_length,
            } => (
                "message_added".into(),
                json!({ "role": role, "content_length": content_length }),
            ),
        },
        EventKind::Custom { name, payload } => (format!("Custom:{name}"), payload.clone()),
    }
}

#[async_trait]
impl<L: RunLedger + 'static> EventHandler for LedgerHandler<L> {
    #[instrument(skip(self), name = "ledger_handler_on_start")]
    async fn on_start(&self) {
        *self.start_time.write().await = Some(std::time::Instant::now());

        match self
            .ledger
            .create_run(&self.spec_digest, self.metadata.clone())
            .await
        {
            Ok(id) => {
                *self.run_id.write().await = Some(id);
            }
            Err(e) => {
                warn!(error = %e, "LedgerHandler: failed to create run");
            }
        }
    }

    #[instrument(skip(self, event), name = "ledger_handler_handle", level = "debug")]
    async fn handle(&self, event: &Event) {
        let run_id = {
            let guard = self.run_id.read().await;
            match guard.as_ref() {
                Some(id) => id.clone(),
                None => return,
            }
        };

        METRICS.inc_events_processed();

        let (kind, payload) = map_event(event);

        // Track errors
        if matches!(
            &event.kind,
            EventKind::Graph(GraphEvent::Error { .. }) | EventKind::Node(NodeEvent::Error { .. })
        ) {
            self.saw_error.store(true, Ordering::SeqCst);
        }

        let run_event = RunEvent {
            seq: self.next_seq(),
            kind,
            payload,
            timestamp: event.timestamp,
        };

        if let Err(e) = self.ledger.append_event(&run_id, run_event).await {
            warn!(error = %e, run_id = %run_id, "LedgerHandler: failed to append event");
        }
    }

    #[instrument(skip(self), name = "ledger_handler_on_stop")]
    async fn on_stop(&self) {
        let run_id = {
            let guard = self.run_id.read().await;
            match guard.as_ref() {
                Some(id) => id.clone(),
                None => return,
            }
        };

        let total_events = self.seq.load(Ordering::SeqCst) - 1;
        let duration_ms = self
            .start_time
            .read()
            .await
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let success = !self.saw_error.load(Ordering::SeqCst);

        let summary = RunSummary {
            total_events,
            final_state_digest: None,
            duration_ms,
            success,
        };

        let result = if success {
            self.ledger.complete_run(&run_id, summary).await
        } else {
            self.ledger.fail_run(&run_id, summary).await
        };

        if let Err(e) = result {
            warn!(error = %e, run_id = %run_id, "LedgerHandler: failed to finalize run");
        }
    }
}

/// Subscribe a [`LedgerHandler`] to an [`EventBus`], spawning it as a
/// background task via [`spawn_handler`].
///
/// Returns the handler so callers can inspect `run_id()`, `seq()`, etc.
/// The background task runs until the `EventBus` sender is dropped.
pub fn subscribe_ledger_to_bus<L: RunLedger + 'static>(
    bus: &EventBus,
    ledger: Arc<L>,
    spec_digest: ContentDigest,
    metadata: RunMetadata,
) -> Arc<LedgerHandler<L>> {
    let handler = Arc::new(LedgerHandler::new(ledger, spec_digest, metadata));
    let receiver = bus.subscribe();
    spawn_handler(handler.clone(), receiver);
    handler
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxidized_state::fakes::MemoryRunLedger;
    use oxidized_state::storage_traits::RunStatus;
    use std::time::Duration;

    fn test_digest() -> ContentDigest {
        ContentDigest::from_bytes(b"test-spec")
    }

    fn test_metadata() -> RunMetadata {
        RunMetadata {
            git_sha: None,
            agent_name: "test-agent".into(),
            tags: json!({}),
            evaluation: Default::default(),
        }
    }

    #[tokio::test]
    async fn map_event_covers_all_variants() {
        let cases = vec![
            (
                Event::graph_started("t", Some("g".into()), "entry".into()),
                "graph_started",
            ),
            (
                Event::graph_completed("t", 5, Duration::from_millis(100)),
                "graph_completed",
            ),
            (Event::graph_error("t", "boom".into()), "graph_failed"),
            (Event::node_entered("t", "n".into(), 1), "node_entered"),
            (
                Event::node_exited("t", "n".into(), Some("m".into()), Duration::from_millis(50)),
                "node_exited",
            ),
            (
                Event::node_error("t", "n".into(), "fail".into()),
                "node_failed",
            ),
            (
                Event::checkpoint_saved("t", "cp1".into(), "n".into()),
                "checkpoint_saved",
            ),
            (
                Event::checkpoint_restored("t", "cp1".into(), "n".into()),
                "checkpoint_restored",
            ),
            (
                Event::state_updated("t", "n".into(), vec!["key".into()]),
                "state_updated",
            ),
        ];

        for (event, expected_kind) in cases {
            let (kind, _) = map_event(&event);
            assert_eq!(kind, expected_kind, "wrong kind for {expected_kind}");
        }
    }

    #[tokio::test]
    async fn handler_creates_and_completes_run() {
        let ledger = Arc::new(MemoryRunLedger::new());
        let handler = LedgerHandler::new(ledger.clone(), test_digest(), test_metadata());

        handler.on_start().await;
        let run_id = handler.run_id().await.expect("run_id should be set");

        // Feed an event
        let event = Event::graph_started("t", Some("g".into()), "entry".into());
        handler.handle(&event).await;
        // After 1 event, the next seq is 2
        assert_eq!(handler.seq(), 2);

        handler.on_stop().await;

        let record = ledger.get_run(&run_id).await.unwrap();
        assert_eq!(record.status, RunStatus::Completed);
        assert!(record.summary.as_ref().unwrap().success);
    }

    #[tokio::test]
    async fn handler_marks_run_failed_on_error_event() {
        let ledger = Arc::new(MemoryRunLedger::new());
        let handler = LedgerHandler::new(ledger.clone(), test_digest(), test_metadata());

        handler.on_start().await;
        let run_id = handler.run_id().await.unwrap();

        let event = Event::graph_error("t", "kaboom".into());
        handler.handle(&event).await;

        handler.on_stop().await;

        let record = ledger.get_run(&run_id).await.unwrap();
        assert_eq!(record.status, RunStatus::Failed);
        assert!(!record.summary.as_ref().unwrap().success);
    }

    #[tokio::test]
    async fn custom_event_mapping() {
        let event = Event::new(
            "t",
            EventKind::Custom {
                name: "MyCustom".into(),
                payload: json!({"foo": "bar"}),
            },
        );
        let (kind, payload) = map_event(&event);
        assert_eq!(kind, "Custom:MyCustom");
        assert_eq!(payload, json!({"foo": "bar"}));
    }
}
