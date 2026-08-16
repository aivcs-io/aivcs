//! `aivcs import` — one-shot GitHub read-only bootstrap → AIVCS forge publish.
//!
//! Mirrors the og-agent-forge `onboard` flow without a separate tool or temp scripts:
//! shallow git clone (bootstrap only) → strip `.git` → publish → delete clone.

use crate::forge_remote::ForgeRemoteClient;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::info;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub source: String,
    pub repo: Option<String>,
    pub git_branch: String,
    pub forge_branch: String,
    pub message: String,
    pub author: Option<String>,
    pub remote: Option<String>,
    pub token: Option<String>,
    pub keep_dir: Option<PathBuf>,
    pub forge_provenance: bool,
}

pub fn parse_github_slug(source: &str) -> Result<(String, String)> {
    let s = source.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = s.strip_prefix("https://github.com/") {
        return slug_from_path(rest);
    }
    if let Some(rest) = s.strip_prefix("http://github.com/") {
        return slug_from_path(rest);
    }
    if let Some(rest) = s.strip_prefix("github.com/") {
        return slug_from_path(rest);
    }
    if s.contains('/') && !s.contains(':') && !s.starts_with('.') {
        return slug_from_path(s);
    }
    Err(anyhow!(
        "source must be github.com/org/repo, https://github.com/org/repo, or org/repo — got {source:?}"
    ))
}

fn slug_from_path(path: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return Err(anyhow!("expected org/name, got {path:?}"));
    }
    let slug = format!("{}/{}", parts[0], parts[1]);
    let url = format!("https://github.com/{slug}");
    Ok((slug, url))
}

/// Default forge target for GitHub bootstrap: `lornu-ai/<name>` (or any `*/<name>`)
/// → `aivcs/<name>`. GitHub org is not copied to the forge org.
pub fn default_forge_slug_from_github(source: &str) -> Result<String> {
    let (github_slug, _) = parse_github_slug(source)?;
    let name = github_slug
        .split('/')
        .nth(1)
        .ok_or_else(|| anyhow!("expected org/name in GitHub slug, got {github_slug:?}"))?;
    Ok(format!("aivcs/{name}"))
}

pub async fn run_import(opts: ImportOptions) -> Result<()> {
    let (github_slug, github_url) = parse_github_slug(&opts.source)?;
    let repo = match opts.repo {
        Some(r) => r,
        None => default_forge_slug_from_github(&opts.source)?,
    };
    if !repo.contains('/') {
        return Err(anyhow!(
            "--repo must be org/name (e.g. aivcs/agent-envelope-ai)"
        ));
    }

    let tmp = TempDir::new().context("create temp dir for github bootstrap")?;
    let checkout = tmp.path().join("tree");
    info!(
        "Cloning {github_url} (branch {}) — read-only bootstrap → aivcs://{repo}",
        opts.git_branch
    );
    shallow_clone(&github_url, &opts.git_branch, &checkout)?;

    strip_git_metadata(&checkout)?;
    if opts.forge_provenance {
        patch_forge_provenance(&checkout, &repo, &github_slug)?;
    }

    let author = opts
        .author
        .or_else(|| std::env::var("AIVCS_AUTHOR").ok())
        .unwrap_or_else(|| "aivcs import <import@aivcs.io>".to_string());

    let client = ForgeRemoteClient::new(opts.remote.as_deref(), opts.token.as_deref());
    let commit_id = client
        .publish(&checkout, &repo, &opts.message, &author, &opts.forge_branch, Some(true))
        .await
        .context("forge publish after github bootstrap")?;

    if let Some(ref keep) = opts.keep_dir {
        copy_tree_without_git(&checkout, keep)?;
        info!("Working copy (no .git): {}", keep.display());
    }

    println!(
        "Imported {github_url} → aivcs://{repo}@{}",
        opts.forge_branch
    );
    println!("Commit ID: {commit_id}");
    Ok(())
}

fn shallow_clone(url: &str, branch: &str, dest: &Path) -> Result<()> {
    let status = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            branch,
            "--single-branch",
            url,
        ])
        .arg(dest)
        .status()
        .context("git clone failed — is git installed?")?;
    if !status.success() {
        return Err(anyhow!("git clone exited with {status}"));
    }
    Ok(())
}

fn strip_git_metadata(root: &Path) -> Result<()> {
    let git_dir = root.join(".git");
    if git_dir.exists() {
        fs::remove_dir_all(&git_dir).with_context(|| format!("remove {}", git_dir.display()))?;
    }
    Ok(())
}

fn patch_forge_provenance(root: &Path, forge_repo: &str, github_slug: &str) -> Result<()> {
    let aivcs_url = format!("aivcs://{forge_repo}");
    let github_https = format!("https://github.com/{github_slug}");
    for rel in ["Cargo.toml", "propel.toml", "flake.nix", "README.md"] {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        if content.contains(&github_https) {
            let updated = content.replace(&github_https, &aivcs_url);
            fs::write(&path, updated)?;
        }
    }
    Ok(())
}

fn copy_tree_without_git(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| format!("remove existing {}", dest.display()))?;
    }
    fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;
    copy_dir_recursive(src, dest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(name);
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_https_github_url() {
        let (slug, url) =
            parse_github_slug("https://github.com/lornu-ai/agent-envelope-ai").unwrap();
        assert_eq!(slug, "lornu-ai/agent-envelope-ai");
        assert_eq!(url, "https://github.com/lornu-ai/agent-envelope-ai");
    }

    #[test]
    fn parse_bare_slug() {
        let (slug, _) = parse_github_slug("lornu-ai/og-crab").unwrap();
        assert_eq!(slug, "lornu-ai/og-crab");
    }

    #[test]
    fn default_forge_slug_maps_github_to_aivcs_org() {
        assert_eq!(
            default_forge_slug_from_github("https://github.com/lornu-ai/infra-code").unwrap(),
            "aivcs/infra-code"
        );
        assert_eq!(
            default_forge_slug_from_github("lornu-ai/sandlot").unwrap(),
            "aivcs/sandlot"
        );
    }
}
