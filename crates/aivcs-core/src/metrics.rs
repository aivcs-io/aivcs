//! Global atomic counters for AIVCS observability.
//!
//! Counters are incremented silently at the call site. Call
//! [`Metrics::flush`] to emit current values as a single
//! `tracing::info!` event (e.g. at the end of a run).

use std::sync::atomic::{AtomicU64, Ordering};

/// Global metrics singleton.
pub static METRICS: Metrics = Metrics::new();

/// Lightweight atomic counters — no allocations, no locking.
pub struct Metrics {
    events_processed: AtomicU64,
    replays_executed: AtomicU64,
    forks_created: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub const fn new() -> Self {
        Self {
            events_processed: AtomicU64::new(0),
            replays_executed: AtomicU64::new(0),
            forks_created: AtomicU64::new(0),
        }
    }

    /// Increment the events-processed counter by one.
    pub fn inc_events_processed(&self) {
        self.events_processed.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(metric = "events_processed", "counter incremented");
    }

    /// Increment the replays-executed counter by one.
    pub fn inc_replays(&self) {
        self.replays_executed.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(metric = "replays_executed", "counter incremented");
    }

    /// Increment the forks-created counter by one.
    pub fn inc_forks(&self) {
        self.forks_created.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(metric = "forks_created", "counter incremented");
    }

    /// Emit all current counter values as a single `info!` event.
    ///
    /// Call this at natural boundaries (end of a run, daemon tick, etc.)
    /// rather than on every increment.
    pub fn flush(&self) {
        tracing::info!(
            metric = "flush",
            events_processed = self.events_processed(),
            replays_executed = self.replays_executed(),
            forks_created = self.forks_created(),
        );
    }

    /// Read the current events-processed count.
    pub fn events_processed(&self) -> u64 {
        self.events_processed.load(Ordering::Relaxed)
    }

    /// Read the current replays-executed count.
    pub fn replays_executed(&self) -> u64 {
        self.replays_executed.load(Ordering::Relaxed)
    }

    /// Read the current forks-created count.
    pub fn forks_created(&self) -> u64 {
        self.forks_created.load(Ordering::Relaxed)
    }

    /// Reset all counters to zero (useful in tests).
    pub fn reset(&self) {
        self.events_processed.store(0, Ordering::Relaxed);
        self.replays_executed.store(0, Ordering::Relaxed);
        self.forks_created.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment() {
        let m = Metrics::new();
        assert_eq!(m.events_processed(), 0);
        m.inc_events_processed();
        m.inc_events_processed();
        assert_eq!(m.events_processed(), 2);

        m.inc_replays();
        assert_eq!(m.replays_executed(), 1);

        m.inc_forks();
        m.inc_forks();
        m.inc_forks();
        assert_eq!(m.forks_created(), 3);
    }

    #[test]
    fn reset_zeroes_all() {
        let m = Metrics::new();
        m.inc_events_processed();
        m.inc_replays();
        m.inc_forks();
        m.reset();
        assert_eq!(m.events_processed(), 0);
        assert_eq!(m.replays_executed(), 0);
        assert_eq!(m.forks_created(), 0);
    }
}
