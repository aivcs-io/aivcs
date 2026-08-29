//! Logic hashing - hash Rust source code for version control
//!
//! Generates content-addressable hashes from Rust source code,
//! enabling versioning of agent logic alongside state and environment.

use crate::error::NixError;
use crate::Result;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::debug;

/// Generate a hash of Rust source code
///
/// This function recursively walks a directory and hashes all .rs files,
/// creating a single hash that represents the logic of the agent.
///
/// # TDD: test_changing_rust_source_changes_logic_hash
pub fn generate_logic_hash(source_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    if source_path.is_file() {
        // Single file
        hash_rust_file(source_path, &mut hasher)?;
    } else if source_path.is_dir() {
        // Directory - hash all .rs files
        hash_rust_directory(source_path, &mut hasher)?;
    } else {
        return Err(NixError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Source path not found: {:?}", source_path),
        )));
    }

    let hash = hex::encode(hasher.finalize());
    debug!("Logic hash: {}", hash.get(..12).unwrap_or(&hash));
    Ok(hash)
}

/// Hash a single Rust file
fn hash_rust_file(path: &Path, hasher: &mut Sha256) -> Result<()> {
    let content = std::fs::read(path)?;

    // Include the filename in the hash for uniqueness
    if let Some(name) = path.file_name() {
        hasher.update(name.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }

    // Normalize line endings and hash content
    let normalized = normalize_source(&content);
    hasher.update(&normalized);
    hasher.update(b"\0");

    Ok(())
}

/// Recursively hash all Rust files in a directory
fn hash_rust_directory(dir: &Path, hasher: &mut Sha256) -> Result<()> {
    let mut entries = collect_rust_files(dir)?;

    // Sort for deterministic ordering
    entries.sort();

    for path in entries {
        // Hash relative path for consistency across machines
        let relative = path.strip_prefix(dir).unwrap_or(&path);
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(b"\0");

        let content = std::fs::read(&path)?;
        let normalized = normalize_source(&content);
        hasher.update(&normalized);
        hasher.update(b"\0");
    }

    Ok(())
}

/// Collect all Rust files in a directory recursively
fn collect_rust_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    collect_rust_files_recursive(dir, &mut files)?;
    Ok(files)
}

fn collect_rust_files_recursive(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        // Skip hidden files, target dir, and other non-source directories
        if name.starts_with('.') || name == "target" || name == "node_modules" || name == ".git" {
            continue;
        }

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "rs" {
                    files.push(path);
                }
            }
        } else if path.is_dir() {
            collect_rust_files_recursive(&path, files)?;
        }
    }

    Ok(())
}

/// Normalize source code for consistent hashing
///
/// - Convert CRLF to LF
/// - Remove trailing whitespace
/// - Ensure single newline at end
fn normalize_source(content: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(content);

    let normalized: String = text
        .lines()
        .map(|line| line.trim_end()) // Remove trailing whitespace
        .collect::<Vec<_>>()
        .join("\n");

    let mut result = normalized.into_bytes();
    if !result.is_empty() && result.last() != Some(&b'\n') {
        result.push(b'\n');
    }

    result
}

/// Generate hash for a Cargo.toml file (dependencies affect logic)
#[allow(dead_code)]
pub fn generate_cargo_hash(cargo_path: &Path) -> Result<String> {
    let content = std::fs::read(cargo_path)?;

    let mut hasher = Sha256::new();
    hasher.update(&content);

    Ok(hex::encode(hasher.finalize()))
}

/// Generate a combined hash of source + dependencies
#[allow(dead_code)]
pub fn generate_full_logic_hash(project_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();

    // Hash Cargo.toml
    let cargo_toml = project_path.join("Cargo.toml");
    if cargo_toml.exists() {
        let content = std::fs::read(&cargo_toml)?;
        hasher.update(b"Cargo.toml:");
        hasher.update(&content);
    }

    // Hash Cargo.lock (pinned dependencies)
    let cargo_lock = project_path.join("Cargo.lock");
    if cargo_lock.exists() {
        let content = std::fs::read(&cargo_lock)?;
        hasher.update(b"Cargo.lock:");
        hasher.update(&content);
    }

    // Hash source files
    let src_dir = project_path.join("src");
    if src_dir.exists() {
        let rust_files = collect_rust_files(&src_dir)?;
        for path in rust_files {
            let relative = path.strip_prefix(project_path).unwrap_or(&path);
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(b":");

            let content = std::fs::read(&path)?;
            let normalized = normalize_source(&content);
            hasher.update(&normalized);
        }
    }

    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_logic_hash_single_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("main.rs");
        std::fs::write(&file_path, "fn main() { println!(\"hello\"); }").unwrap();

        let hash = generate_logic_hash(&file_path).unwrap();
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // SHA256 hex = 64 chars
    }

    #[test]
    fn test_generate_logic_hash_deterministic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn foo() {}").unwrap();

        let hash1 = generate_logic_hash(&file_path).unwrap();
        let hash2 = generate_logic_hash(&file_path).unwrap();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_changing_rust_source_changes_logic_hash() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("lib.rs");

        std::fs::write(&file_path, "fn version_1() {}").unwrap();
        let hash1 = generate_logic_hash(&file_path).unwrap();

        std::fs::write(&file_path, "fn version_2() {}").unwrap();
        let hash2 = generate_logic_hash(&file_path).unwrap();

        assert_ne!(
            hash1, hash2,
            "Different source should produce different hash"
        );
    }

    #[test]
    fn test_hash_directory_multiple_files() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();

        std::fs::write(src.join("lib.rs"), "pub fn lib() {}").unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let hash = generate_logic_hash(&src).unwrap();
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_normalize_source_crlf() {
        let content = b"line1\r\nline2\r\n";
        let normalized = normalize_source(content);
        assert_eq!(normalized, b"line1\nline2\n");
    }

    #[test]
    fn test_normalize_source_trailing_whitespace() {
        let content = b"line1   \nline2\t\n";
        let normalized = normalize_source(content);
        assert_eq!(normalized, b"line1\nline2\n");
    }

    #[test]
    fn test_generate_full_logic_hash() {
        let dir = tempdir().unwrap();

        // Create minimal project structure
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let hash = generate_full_logic_hash(dir.path()).unwrap();
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_skips_target_directory() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        // Create target directory with compiled files
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("compiled.rs"), "// should be ignored").unwrap();

        let files = collect_rust_files(dir.path()).unwrap();

        // Should only find src/main.rs
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("main.rs"));
    }
}
