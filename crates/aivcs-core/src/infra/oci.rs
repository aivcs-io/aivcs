//! OCI publish — Nix flake image → GAR via skopeo (GitLab CI path; no GHA).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Resolved dockworker build target (legacy `[build].targets` or `[[targets]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciTarget {
    pub name: String,
    pub nix_output: String,
    pub image: String,
}

/// Registry coordinates from `dockworker.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciRegistry {
    pub url: String,
    pub default_tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DockworkerManifest {
    package: Option<PackageSection>,
    registry: Option<RegistrySection>,
    defaults: Option<DefaultsSection>,
    build: Option<BuildSection>,
    targets: Option<Vec<TargetSection>>,
}

#[derive(Debug, Deserialize)]
struct PackageSection {
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrySection {
    url: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct DefaultsSection {
    registry: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BuildSection {
    targets: Option<Vec<TargetSection>>,
}

#[derive(Debug, Deserialize)]
struct TargetSection {
    name: String,
    #[serde(default)]
    nix_target: Option<String>,
    #[serde(default)]
    nix_output: Option<String>,
    image: String,
}

/// Parse `dockworker.toml` and return the named OCI target + registry.
pub fn resolve_oci_target(
    manifest_path: &Path,
    target_name: &str,
) -> Result<(OciTarget, OciRegistry)> {
    let content = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("read dockworker manifest at {}", manifest_path.display()))?;
    let manifest: DockworkerManifest = toml::from_str(&content).context("parse dockworker.toml")?;

    let targets = collect_targets(&manifest);
    let target = targets
        .into_iter()
        .find(|t| t.name == target_name)
        .with_context(|| format!("OCI target '{target_name}' not found in dockworker.toml"))?;

    let registry_url = manifest
        .registry
        .as_ref()
        .and_then(|r| r.url.clone())
        .or_else(|| manifest.defaults.as_ref().and_then(|d| d.registry.clone()))
        .filter(|u| !u.contains("${"))
        .context(
            "dockworker.toml must set [registry].url or [defaults].registry (no template vars)",
        )?;

    let default_tags = manifest
        .registry
        .as_ref()
        .and_then(|r| r.tags.clone())
        .unwrap_or_default();

    let registry = OciRegistry {
        url: registry_url.trim_end_matches('/').to_string(),
        default_tags,
    };

    Ok((target, registry))
}

fn collect_targets(manifest: &DockworkerManifest) -> Vec<OciTarget> {
    let mut out = Vec::new();
    if let Some(build) = &manifest.build {
        if let Some(targets) = &build.targets {
            for t in targets {
                out.push(section_to_target(t));
            }
        }
    }
    if let Some(targets) = &manifest.targets {
        for t in targets {
            out.push(section_to_target(t));
        }
    }
    out
}

fn section_to_target(t: &TargetSection) -> OciTarget {
    let nix_output = t
        .nix_output
        .clone()
        .or_else(|| t.nix_target.clone())
        .map(|s| normalize_nix_output(&s))
        .unwrap_or_else(|| t.name.clone());

    OciTarget {
        name: t.name.clone(),
        nix_output,
        image: t.image.clone(),
    }
}

/// Strip optional `.#` prefix from flake attribute references.
pub fn normalize_nix_output(raw: &str) -> String {
    raw.trim()
        .strip_prefix(".#")
        .unwrap_or(raw.trim())
        .to_string()
}

/// Resolve push tags: explicit env override, then version + CI metadata.
pub fn resolve_push_tags(
    package_version: &str,
    registry_defaults: &[String],
    extra: &[String],
) -> Vec<String> {
    if let Ok(raw) = std::env::var("AIVCS_OCI_TAGS") {
        let tags: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if !tags.is_empty() {
            return tags;
        }
    }

    let mut tags = Vec::new();
    if let Ok(sha) = std::env::var("CI_COMMIT_SHA") {
        if sha.len() >= 7 {
            tags.push(format!("sha-{}", &sha[..7]));
        }
    } else if let Ok(sha) = std::env::var("GITHUB_SHA") {
        if sha.len() >= 7 {
            tags.push(format!("sha-{}", &sha[..7]));
        }
    }

    tags.push(package_version.to_string());

    if let Ok(branch) = std::env::var("CI_COMMIT_REF_NAME") {
        if branch == "develop" {
            tags.push("develop".to_string());
            tags.push("latest".to_string());
        }
        tags.push(branch);
    } else if let Ok(branch) = std::env::var("GITHUB_REF_NAME") {
        if branch == "develop" {
            tags.push("develop".to_string());
            tags.push("latest".to_string());
        }
        tags.push(branch);
    }

    for t in registry_defaults {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    for t in extra {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

/// Package version from manifest or workspace `CARGO_PKG_VERSION`.
pub fn package_version_from_manifest(manifest_path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(manifest_path)?;
    let manifest: DockworkerManifest = toml::from_str(&content)?;
    Ok(manifest
        .package
        .and_then(|p| p.version)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()))
}

/// Build the Nix OCI derivation; returns path to `./result` symlink target.
pub fn nix_build_image(
    repo_root: &Path,
    nix_output: &str,
    system: Option<&str>,
) -> Result<PathBuf> {
    let attr = normalize_nix_output(nix_output);
    let flake_ref = format!(".#{attr}");
    let mut cmd = Command::new("nix");
    cmd.arg("build")
        .arg(&flake_ref)
        .arg("-L")
        .current_dir(repo_root);
    if let Some(sys) = system {
        cmd.args(["--system", sys]);
    }
    let status = cmd
        .status()
        .context("failed to spawn nix build — is Nix installed?")?;
    if !status.success() {
        bail!("nix build {flake_ref} failed with exit {status}");
    }
    let result = repo_root.join("result");
    if !result.exists() {
        bail!("nix build succeeded but ./result is missing");
    }
    Ok(result)
}

/// Push a Nix-built docker archive (`./result` loader) to GAR with skopeo.
pub fn skopeo_push_image(
    result_path: &Path,
    gar_image_base: &str,
    tags: &[String],
    access_token: &str,
) -> Result<()> {
    if tags.is_empty() {
        bail!("no tags to push");
    }
    for tag in tags {
        let dest = format!("{gar_image_base}:{tag}");
        eprintln!("→ skopeo push {dest}");
        let script = format!(
            "{result} | nix shell nixpkgs#skopeo -c skopeo copy \
             --dest-creds=oauth2accesstoken:{token} \
             docker-archive:/dev/stdin \
             docker://{dest}",
            result = result_path.display(),
            token = access_token,
            dest = dest,
        );
        let status = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .status()
            .context("spawn skopeo push pipeline")?;
        if !status.success() {
            bail!("skopeo push failed for {dest}");
        }
    }
    Ok(())
}

/// Resolve GCP access token for GAR push.
pub fn resolve_gar_access_token() -> Result<String> {
    if let Ok(token) = std::env::var("GCP_ACCESS_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    if let Ok(token) = std::env::var("AIVCS_GAR_ACCESS_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    bail!("GCP_ACCESS_TOKEN or AIVCS_GAR_ACCESS_TOKEN must be set for GAR push (GitLab CI / WIF)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_nix_output_strips_flake_prefix() {
        assert_eq!(normalize_nix_output(".#aivcs-cli-image"), "aivcs-cli-image");
        assert_eq!(normalize_nix_output("aivcs-cli-image"), "aivcs-cli-image");
    }

    #[test]
    fn resolve_push_tags_deduplicates() {
        let tags = resolve_push_tags("0.3.2", &["latest".to_string()], &["0.3.2".to_string()]);
        assert!(tags.contains(&"0.3.2".to_string()));
        assert!(tags.contains(&"latest".to_string()));
        assert_eq!(tags.iter().filter(|t| *t == "0.3.2").count(), 1);
    }

    #[test]
    fn resolve_push_tags_honors_env_override() {
        std::env::set_var("AIVCS_OCI_TAGS", "canary,0.3.2");
        let tags = resolve_push_tags("0.3.1", &[], &[]);
        std::env::remove_var("AIVCS_OCI_TAGS");
        assert_eq!(tags, vec!["canary".to_string(), "0.3.2".to_string()]);
    }
}
