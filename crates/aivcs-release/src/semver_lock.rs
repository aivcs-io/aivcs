//! Gate 1: Cargo.toml + CHANGELOG + docs download pins must match the target semver.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;

/// Strip optional leading `v` and validate `X.Y.Z` (+ optional pre-release).
pub fn normalize_semver(raw: &str) -> Result<String> {
    let s = raw.trim().trim_start_matches('v');
    let re = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$").unwrap();
    if !re.is_match(s) {
        bail!("'{raw}' is not a valid semver (expected X.Y.Z)");
    }
    Ok(s.to_string())
}

/// Read `[workspace.package].version` from a workspace root `Cargo.toml`.
pub fn read_workspace_package_version(cargo_toml: &str) -> Result<String> {
    let value: toml::Value = toml::from_str(cargo_toml).context("parse Cargo.toml")?;
    let ver = value
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Cargo.toml missing [workspace.package] version"))?;
    Ok(ver.to_string())
}

pub fn changelog_has_version(changelog: &str, version: &str) -> bool {
    changelog
        .lines()
        .any(|l| l.starts_with(&format!("## [{version}]")))
}

/// Ensure README / getting-started download pins only reference `v{version}`.
pub fn verify_docs_download_pins(docs: &[(&str, &str)], version: &str) -> Result<()> {
    let pin_re = Regex::new(r"releases/download/v([0-9]+\.[0-9]+\.[0-9]+)").unwrap();
    let docker_re = Regex::new(r"ghcr\.io/aivcs-io/aivcs:([0-9]+\.[0-9]+\.[0-9]+)").unwrap();
    let expected = format!("v{version}");

    for (label, body) in docs {
        for cap in pin_re.captures_iter(body) {
            let found = format!("v{}", &cap[1]);
            if found != expected {
                bail!("{label} pins {found}, expected {expected}");
            }
        }
        for cap in docker_re.captures_iter(body) {
            if &cap[1] != version {
                bail!("{label} Docker tag :{}, expected :{version}", &cap[1]);
            }
        }
    }
    Ok(())
}

pub fn verify_semver_lockstep(repo_root: &Path, target: &str) -> Result<()> {
    let cargo_path = repo_root.join("Cargo.toml");
    let cargo = fs::read_to_string(&cargo_path)
        .with_context(|| format!("read {}", cargo_path.display()))?;
    let current = read_workspace_package_version(&cargo)?;
    if current != target {
        bail!(
            "Cargo.toml version is '{current}', expected '{target}'. \
             Bump workspace.package.version and CHANGELOG before releasing."
        );
    }

    let changelog_path = repo_root.join("CHANGELOG.md");
    let changelog = fs::read_to_string(&changelog_path)
        .with_context(|| format!("read {}", changelog_path.display()))?;
    if !changelog_has_version(&changelog, target) {
        bail!("CHANGELOG.md missing '## [{target}]' section");
    }

    let readme = fs::read_to_string(repo_root.join("README.md")).unwrap_or_default();
    let getting = fs::read_to_string(repo_root.join("docs/getting-started.md")).unwrap_or_default();
    verify_docs_download_pins(&[("README.md", &readme), ("docs/getting-started.md", &getting)], target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn normalize_strips_v() {
        assert_eq!(normalize_semver("v0.5.0").unwrap(), "0.5.0");
        assert!(normalize_semver("nope").is_err());
    }

    #[test]
    fn reads_workspace_version() {
        let toml = r#"
[workspace]
members = ["crates/a"]
[workspace.package]
version = "0.5.0"
edition = "2021"
"#;
        assert_eq!(read_workspace_package_version(toml).unwrap(), "0.5.0");
    }

    #[test]
    fn docs_pins_must_match() {
        let ok = "curl …/releases/download/v0.5.0/aivcs-darwin-arm64\nghcr.io/aivcs-io/aivcs:0.5.0";
        verify_docs_download_pins(&[("README", ok)], "0.5.0").unwrap();
        let bad = "releases/download/v0.4.4/aivcs-darwin-arm64";
        assert!(verify_docs_download_pins(&[("README", bad)], "0.5.0").is_err());
    }

    #[test]
    fn lockstep_on_fixture_tree() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers=[]\n[workspace.package]\nversion = \"0.5.0\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("CHANGELOG.md"),
            "## [0.5.0] - 2026-08-28\n\n### Added\n- x\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("README.md"),
            "releases/download/v0.5.0/aivcs\nghcr.io/aivcs-io/aivcs:0.5.0\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(
            dir.path().join("docs/getting-started.md"),
            "releases/download/v0.5.0/aivcs\n",
        )
        .unwrap();
        verify_semver_lockstep(dir.path(), "0.5.0").unwrap();
    }
}
