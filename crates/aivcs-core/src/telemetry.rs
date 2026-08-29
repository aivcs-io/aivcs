//! Centralised tracing initialisation for AIVCS binaries.
//!
//! Call [`init_tracing`] once at program start to configure the global
//! subscriber with an `EnvFilter` and optional JSON formatting.
//!
//! Safe to call more than once — subsequent calls are silently ignored
//! (the global subscriber can only be set once per process).

use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialise the global tracing subscriber.
///
/// * `json` — when `true`, emit newline-delimited JSON log lines
///   (useful for log aggregation pipelines).
/// * `level` — default verbosity when `RUST_LOG` is not set.
///
/// Respects the `RUST_LOG` environment variable for fine-grained filtering.
/// If `RUST_LOG` is not set, falls back to the supplied `level`.
///
/// Safe to call multiple times; only the first call takes effect.
pub fn init_tracing(json: bool, level: Level) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.as_str()));

    if json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(false).json())
            .try_init()
            .ok();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(false))
            .try_init()
            .ok();
    }
}
