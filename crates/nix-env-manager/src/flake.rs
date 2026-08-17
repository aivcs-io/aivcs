//! Nix Flake hashing and metadata
//!
//! Provides functions to generate content-addressable hashes from Nix Flakes,
//! ensuring environment reproducibility.

use crate::error::NixError;
use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info, warn};

/// Nix environment hash - content-addressable identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NixHash {
    /// The SHA256 hash
    pub hash: String,
    /// Source of the hash (flake.lock, flake.nix, or metadata)
    pub source: HashSource,
}

/// Source of the Nix hash
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashSource {
    /// Hash computed from flake.lock file
    FlakeLock,
    /// Hash computed from flake.nix file (fallback)
    FlakeNix,
    /// Hash from nix flake metadata command
    Metadata,
    /// Hash from directory contents (no flake)
    Directory,
}

impl std::fmt::Display for NixHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash)
    }
}

impl NixHash {
    /// Create a new NixHash
    pub fn new(hash: String, source: HashSource) -> Self {
        NixHash { hash, source }
    }

    /// Get short hash (first 12 characters)
    pub fn short(&self) -> String {
        self.hash.chars().take(12).collect()
    }
}

/// Metadata from a Nix Flake
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeMetadata {
    /// Flake description
    pub description: Option<String>,
    /// Last modified timestamp
    #[serde(rename = "lastModified")]
    pub last_modified: Option<u64>,
    /// Locked inputs
    pub locks: Option<FlakeLocks>,
    /// Original flake URL
    pub original_url: Option<String>,
    /// Resolved URL
    pub resolved_url: Option<String>,
    /// Revision (if from git)
    pub revision: Option<String>,
}

/// Flake lock file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeLocks {
    /// Lock file version
    pub version: u32,
    /// Root node name
    pub root: String,
    /// Nodes in the lock file
    pub nodes: BTreeMap<String, FlakeLockNode>,
}

/// A node in the flake.lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeLockNode {
    /// Inputs this node depends on
    pub inputs: Option<BTreeMap<String, serde_json::Value>>,
    /// Locked reference
    pub locked: Option<LockedRef>,
    /// Original reference
    pub original: Option<serde_json::Value>,
}

/// A locked reference in flake.lock
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedRef {
    /// Owner (for GitHub)
    pub owner: Option<String>,
    /// Repository name
    pub repo: Option<String>,
    /// Git revision
    pub rev: Option<String>,
    /// Reference type (github, git, path, etc.)
    #[serde(rename = "type")]
    pub ref_type: Option<String>,
    /// NAR hash
    #[serde(rename = "narHash")]
    pub nar_hash: Option<String>,
    /// Last modified timestamp
    #[serde(rename = "lastModified")]
    pub last_modified: Option<u64>,
}

/// Generate environment hash from a Nix Flake
///
/// This function attempts to generate a reproducible hash in the following order:
/// 1. Parse and hash the flake.lock file (most reliable)
/// 2. Run `nix flake metadata --json` and hash the output
/// 3. Hash the flake.nix file directly (fallback)
/// 4. Hash the directory contents (last resort)
///
/// # TDD: test_changing_flake_input_changes_hash
pub fn generate_environment_hash(flake_path: &Path) -> Result<NixHash> {
    info!("Generating environment hash for {:?}", flake_path);

    // Strategy 1: Use flake.lock (most reliable for reproducibility)
    let lock_path = flake_path.join("flake.lock");
    if lock_path.exists() {
        debug!("Found flake.lock, using for hash");
        return hash_flake_lock(&lock_path);
    }

    // Strategy 2: Use nix flake metadata
    let flake_nix = flake_path.join("flake.nix");
    if flake_nix.exists() {
        debug!("No flake.lock, trying nix flake metadata");
        if let Ok(hash) = hash_from_nix_metadata(flake_path) {
            return Ok(hash);
        }

        // Strategy 3: Hash flake.nix directly
        warn!("nix flake metadata failed, hashing flake.nix directly");
        return hash_flake_nix(&flake_nix);
    }

    // Strategy 4: Hash directory (no flake found)
    warn!("No flake found, hashing directory contents");
    hash_directory(flake_path)
}

/// Hash the flake.lock file
fn hash_flake_lock(lock_path: &Path) -> Result<NixHash> {
    let content = std::fs::read(lock_path)?;

    // Parse to validate and normalize
    let locks: FlakeLocks =
        serde_json::from_slice(&content).map_err(|e| NixError::InvalidFlakeLock(e.to_string()))?;

    // Re-serialize for consistent hashing (handles formatting differences)
    let normalized = serde_json::to_vec(&locks)?;

    let mut hasher = Sha256::new();
    hasher.update(&normalized);
    let hash = hex::encode(hasher.finalize());

    debug!("Flake.lock hash: {}", hash.get(..12).unwrap_or(&hash));
    Ok(NixHash::new(hash, HashSource::FlakeLock))
}

/// Hash using nix flake metadata command
fn hash_from_nix_metadata(flake_path: &Path) -> Result<NixHash> {
    let output = Command::new("nix")
        .args(["flake", "metadata", "--json", "--no-update-lock-file"])
        .current_dir(flake_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NixError::NixCommandFailed(stderr.to_string()));
    }

    let mut hasher = Sha256::new();
    hasher.update(&output.stdout);
    let hash = hex::encode(hasher.finalize());

    debug!("Metadata hash: {}", hash.get(..12).unwrap_or(&hash));
    Ok(NixHash::new(hash, HashSource::Metadata))
}

/// Hash the flake.nix file directly
fn hash_flake_nix(flake_nix: &Path) -> Result<NixHash> {
    let content = std::fs::read(flake_nix)?;

    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hex::encode(hasher.finalize());

    debug!("Flake.nix hash: {}", hash.get(..12).unwrap_or(&hash));
    Ok(NixHash::new(hash, HashSource::FlakeNix))
}

/// Hash directory contents (fallback for non-flake directories)
fn hash_directory(dir: &Path) -> Result<NixHash> {
    let mut hasher = Sha256::new();
    hash_directory_recursive(dir, &mut hasher)?;
    let hash = hex::encode(hasher.finalize());

    debug!("Directory hash: {}", hash.get(..12).unwrap_or(&hash));
    Ok(NixHash::new(hash, HashSource::Directory))
}

/// Recursively hash directory contents
fn hash_directory_recursive(dir: &Path, hasher: &mut Sha256) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();

    // Sort for deterministic ordering
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        // Skip hidden files and common non-source directories
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }

        // Hash the relative path with separator
        hasher.update(name.as_bytes());
        hasher.update(b"\0");

        if path.is_file() {
            let content = std::fs::read(&path)?;
            hasher.update(&content);
            hasher.update(b"\0");
        } else if path.is_dir() {
            hash_directory_recursive(&path, hasher)?;
        }
    }

    Ok(())
}

/// Get full flake metadata using nix command
pub fn get_flake_metadata(flake_path: &Path) -> Result<FlakeMetadata> {
    let output = Command::new("nix")
        .args(["flake", "metadata", "--json", "--no-update-lock-file"])
        .current_dir(flake_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NixError::NixCommandFailed(stderr.to_string()));
    }

    let metadata: FlakeMetadata = serde_json::from_slice(&output.stdout)?;
    Ok(metadata)
}

/// Lock flake inputs (equivalent to `nix flake lock`)
#[allow(dead_code)]
pub fn lock_flake(flake_path: &Path) -> Result<()> {
    let output = Command::new("nix")
        .args(["flake", "lock"])
        .current_dir(flake_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NixError::NixCommandFailed(stderr.to_string()));
    }

    Ok(())
}

/// Update flake inputs (equivalent to `nix flake update`)
#[allow(dead_code)]
pub fn update_flake(flake_path: &Path) -> Result<NixHash> {
    let output = Command::new("nix")
        .args(["flake", "update"])
        .current_dir(flake_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NixError::NixCommandFailed(stderr.to_string()));
    }

    // Return new hash after update
    generate_environment_hash(flake_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_nix_hash_display() {
        let hash = NixHash::new("abc123def456".to_string(), HashSource::FlakeLock);
        assert_eq!(format!("{}", hash), "abc123def456");
    }

    #[test]
    fn test_nix_hash_short() {
        let hash = NixHash::new(
            "abc123def456789012345678901234567890123456789012345678901234".to_string(),
            HashSource::FlakeLock,
        );
        assert_eq!(hash.short(), "abc123def456");
    }

    #[test]
    fn test_hash_flake_lock() {
        let dir = tempdir().unwrap();
        let lock_path = dir.path().join("flake.lock");

        let lock_content = r#"{
            "version": 7,
            "root": "root",
            "nodes": {
                "root": {
                    "inputs": {}
                },
                "nixpkgs": {
                    "locked": {
                        "type": "github",
                        "owner": "NixOS",
                        "repo": "nixpkgs",
                        "rev": "abc123"
                    }
                }
            }
        }"#;

        std::fs::write(&lock_path, lock_content).unwrap();

        let hash = hash_flake_lock(&lock_path).unwrap();
        assert!(!hash.hash.is_empty());
        assert_eq!(hash.source, HashSource::FlakeLock);
    }

    #[test]
    fn test_changing_flake_input_changes_hash() {
        let dir = tempdir().unwrap();
        let lock_path = dir.path().join("flake.lock");

        // First version
        let lock_v1 = r#"{"version": 7, "root": "root", "nodes": {"root": {"inputs": {}}, "nixpkgs": {"locked": {"rev": "v1"}}}}"#;
        std::fs::write(&lock_path, lock_v1).unwrap();
        let hash1 = hash_flake_lock(&lock_path).unwrap();

        // Second version (different rev)
        let lock_v2 = r#"{"version": 7, "root": "root", "nodes": {"root": {"inputs": {}}, "nixpkgs": {"locked": {"rev": "v2"}}}}"#;
        std::fs::write(&lock_path, lock_v2).unwrap();
        let hash2 = hash_flake_lock(&lock_path).unwrap();

        assert_ne!(
            hash1.hash, hash2.hash,
            "Different inputs should produce different hashes"
        );
    }

    #[test]
    fn test_hash_flake_nix() {
        let dir = tempdir().unwrap();
        let flake_nix = dir.path().join("flake.nix");

        std::fs::write(&flake_nix, r#"{ outputs = { self }: {}; }"#).unwrap();

        let hash = hash_flake_nix(&flake_nix).unwrap();
        assert!(!hash.hash.is_empty());
        assert_eq!(hash.source, HashSource::FlakeNix);
    }

    #[test]
    fn test_hash_directory() {
        let dir = tempdir().unwrap();

        // Create some files
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let hash = hash_directory(dir.path()).unwrap();
        assert!(!hash.hash.is_empty());
        assert_eq!(hash.source, HashSource::Directory);
    }

    #[test]
    fn test_hash_directory_deterministic() {
        let dir = tempdir().unwrap();

        std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();

        let hash1 = hash_directory(dir.path()).unwrap();
        let hash2 = hash_directory(dir.path()).unwrap();

        assert_eq!(hash1.hash, hash2.hash);
    }

    #[test]
    fn test_generate_environment_hash_prefers_lock() {
        let dir = tempdir().unwrap();

        // Create both flake.nix and flake.lock
        std::fs::write(dir.path().join("flake.nix"), "{ }").unwrap();
        std::fs::write(
            dir.path().join("flake.lock"),
            r#"{"version": 7, "root": "root", "nodes": {"root": {}}}"#,
        )
        .unwrap();

        let hash = generate_environment_hash(dir.path()).unwrap();
        assert_eq!(
            hash.source,
            HashSource::FlakeLock,
            "Should prefer flake.lock when available"
        );
    }
}
