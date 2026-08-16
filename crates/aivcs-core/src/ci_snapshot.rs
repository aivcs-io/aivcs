//! CI snapshot generation and verification for AIVCS.

use anyhow::{Context, Result};
use oxidized_state::CiSnapshot;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Compute deterministic hash of all files in the directory recursively.
/// Skips common non-source directories (e.g. .git, target, node_modules, .local-ci-cache, .direnv).
pub fn compute_workspace_hash(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files_recursive(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0)); // sort relative paths deterministically

    let mut hasher = Sha256::new();
    for (rel_path, abs_path) in files {
        hasher.update(rel_path.as_bytes());
        hasher.update(b"\0");
        if let Ok(content) = std::fs::read(&abs_path) {
            hasher.update(&content);
        }
        hasher.update(b"\0");
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files_recursive(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') && name != ".local-ci.toml" {
            // Skip hidden files/directories except .local-ci.toml
            continue;
        }
        if name == "target"
            || name == "node_modules"
            || name == "dist"
            || name == ".git"
            || name == ".local-ci-cache"
            || name == ".direnv"
        {
            continue;
        }
        if path.is_dir() {
            collect_files_recursive(root, &path, files)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                files.push((rel.to_string_lossy().to_string(), path));
            }
        }
    }
    Ok(())
}

/// Helper to get repo root using `git rev-parse --show-toplevel`.
pub fn find_repo_root() -> PathBuf {
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return PathBuf::from(path_str);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Run local-ci check on the workspace
pub fn run_local_ci(repo_root: &Path) -> Result<()> {
    println!("Executing local-ci against workspace: {:?}", repo_root);
    let status = std::process::Command::new("local-ci")
        .current_dir(repo_root)
        .status()
        .context(
            "Failed to execute 'local-ci' command. Make sure it is installed and on your PATH.",
        )?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("local-ci checks failed. Please run 'local-ci --fix' locally and resolve all issues before opening a PR.")
    }
}

/// Build a CiSnapshot for the current workspace
pub fn build_ci_snapshot(repo_root: &Path) -> Result<CiSnapshot> {
    // 1. Get repo commit SHA
    // Try GITHUB_SHA env var first (handy in GHA)
    let repo_sha = if let Ok(sha) = std::env::var("GITHUB_SHA") {
        sha
    } else if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
    {
        if output.status.success() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    // 2. Compute workspace hash
    let workspace_hash = compute_workspace_hash(repo_root)?;

    // 3. Compute local-ci config hash
    let local_ci_config_hash = {
        let config_path = repo_root.join(".local-ci.toml");
        if config_path.exists() {
            let content = std::fs::read(&config_path)?;
            let mut hasher = Sha256::new();
            hasher.update(&content);
            hex::encode(hasher.finalize())
        } else {
            let hasher = Sha256::new();
            hex::encode(hasher.finalize())
        }
    };

    // 4. Compute env hash using nix-env-manager
    let env_hash = match nix_env_manager::generate_environment_hash(repo_root) {
        Ok(nix_hash) => nix_hash.hash,
        Err(_) => {
            // Fallback to hashing workspace or a default
            "unknown_env".to_string()
        }
    };

    Ok(CiSnapshot {
        repo_sha,
        workspace_hash,
        local_ci_config_hash,
        env_hash,
    })
}
