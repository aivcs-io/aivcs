//! Integration tests for observability: metrics counters and flush emission.
//!
//! All tests use local `Metrics` instances to avoid cross-test interference
//! from the global `METRICS` singleton (which other tests may bump via
//! instrumented code paths running in parallel).

use std::sync::{Arc, Mutex};
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

/// A minimal tracing layer that captures event field values.
struct CapturingLayer {
    captured: Arc<Mutex<Vec<String>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CapturingLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        if let Some(metric) = visitor.metric {
            let mut entry = metric;
            if let Some(ep) = visitor.events_processed {
                entry = format!("flush:events_processed={ep}");
            }
            self.captured.lock().unwrap().push(entry);
        }
    }
}

#[derive(Default)]
struct FieldVisitor {
    metric: Option<String>,
    events_processed: Option<u64>,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "metric" {
            self.metric = Some(value.to_string());
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "events_processed" {
            self.events_processed = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
}

#[test]
fn flush_emits_aggregated_metric_event() {
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));

    let layer = CapturingLayer {
        captured: captured.clone(),
    };

    let subscriber = tracing_subscriber::registry().with(layer).with(
        tracing_subscriber::filter::LevelFilter::from_level(Level::INFO),
    );

    // Use a local Metrics instance to avoid cross-test interference.
    let m = aivcs_core::metrics::Metrics::new();

    tracing::subscriber::with_default(subscriber, || {
        m.inc_events_processed();
        m.inc_events_processed();
        m.inc_replays();
        m.inc_forks();
        // inc_* emits at trace! level — the INFO filter suppresses them.
        // Only flush() emits at info! and should be captured.
        m.flush();
    });

    let events = captured.lock().unwrap();
    assert!(
        events.contains(&"flush:events_processed=2".to_string()),
        "expected flush with events_processed=2 in {:?}",
        *events,
    );
}

#[test]
fn metric_counters_reflect_increments() {
    // Use a local Metrics instance to avoid cross-test interference.
    let m = aivcs_core::metrics::Metrics::new();

    m.inc_events_processed();
    m.inc_events_processed();
    m.inc_events_processed();
    m.inc_events_processed();
    m.inc_events_processed();
    m.inc_replays();
    m.inc_forks();
    m.inc_forks();

    assert_eq!(m.events_processed(), 5);
    assert_eq!(m.replays_executed(), 1);
    assert_eq!(m.forks_created(), 2);
}
