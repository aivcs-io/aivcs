//! Nix-Env-Manager: Nix Flakes and Attic Integration for AIVCS
//!
//! This crate provides the environment versioning layer for AIVCS.
//! It interfaces with Nix Flakes for deterministic builds and
//! Attic for binary caching.
//!
//! ## Layer 2 - Environment/Tooling
//!
//! Focus: Correct hash generation and dependency resolution.
//!
//! ## Features
//!
//! - Generate content-addressable hashes from Nix Flakes
//! - Interact with Attic binary cache for environment storage
//! - Hash Rust source code for logic versioning

mod attic;
mod error;
mod flake;
mod logic;

pub use attic::{AtticClient, AtticConfig};
pub use error::NixError;
pub use flake::{
    generate_environment_hash, get_flake_metadata, FlakeMetadata, HashSource, NixHash,
};
pub use logic::generate_logic_hash;

/// Result type for nix-env-manager operations
pub type Result<T> = std::result::Result<T, NixError>;

/// Check if Nix is available on the system
pub fn is_nix_available() -> bool {
    std::process::Command::new("nix")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if Attic CLI is available
pub fn is_attic_available() -> bool {
    std::process::Command::new("attic")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nix_availability_check() {
        // This test just verifies the function runs without panicking
        let _ = is_nix_available();
    }

    #[test]
    fn test_attic_availability_check() {
        let _ = is_attic_available();
    }
}
