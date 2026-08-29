//! Portable public-release gates for the `aivcs` CLI.
//!
//! Pure checks (semver lockstep, CHANGELOG notes, binary strings audit, checksums)
//! live here so they can be unit-tested without Docker/`gh`. The `aivcs-release-public`
//! binary orchestrates builds and publishing around these gates.

pub mod audit;
pub mod changelog;
pub mod checksum;
pub mod pipeline;
pub mod semver_lock;

pub use audit::{audit_binary_bytes, audit_binary_file, FORBIDDEN_DEPLOYMENT_PATTERNS};
pub use changelog::extract_section;
pub use checksum::{sha256_file, write_sha256sums};
pub use pipeline::{run_release, ReleaseOptions};
pub use semver_lock::{normalize_semver, verify_semver_lockstep};

/// Expected `aivcs --version` stdout for a release cut.
pub fn expected_cli_version_line(version: &str) -> String {
    format!("aivcs {version}")
}
