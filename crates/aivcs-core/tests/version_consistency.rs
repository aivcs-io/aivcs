//! Ensures all workspace crates use `version.workspace = true` and that
//! the workspace version is consistent across all Cargo.toml files.

use std::path::Path;

/// Read the workspace version from the root Cargo.toml.
fn workspace_version() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let doc: toml::Value = root_toml.parse().unwrap();
    doc["workspace"]["package"]["version"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Read the resolved version from a crate's Cargo.toml.
/// If the crate uses `version.workspace = true`, Cargo resolves it at
/// compile time, so we check that the CARGO_PKG_VERSION matches.
fn crate_version(manifest_dir: &Path) -> String {
    let toml_str = std::fs::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
    let doc: toml::Value = toml_str.parse().unwrap();

    // Check if version.workspace = true
    if let Some(pkg) = doc.get("package") {
        if let Some(version) = pkg.get("version") {
            if let Some(table) = version.as_table() {
                if table.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
                    return "workspace".to_string();
                }
            }
            if let Some(v) = version.as_str() {
                return v.to_string();
            }
        }
    }
    panic!(
        "Could not read version from {}",
        manifest_dir.join("Cargo.toml").display()
    );
}

/// Workspace members, read from the root manifest rather than hardcoded.
///
/// A hardcoded list silently breaks whenever the member set changes — which is
/// exactly what happened when crates were removed for the public distribution,
/// leaving this test asserting the existence of crates that had been stripped.
fn workspace_members(root: &Path) -> Vec<String> {
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let doc: toml::Value = root_toml.parse().unwrap();
    doc["workspace"]["members"]
        .as_array()
        .expect("workspace.members must be an array")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("workspace.members entries must be strings")
                .to_string()
        })
        .collect()
}

#[test]
fn all_crates_use_workspace_version() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let crates = workspace_members(workspace_root);
    assert!(
        !crates.is_empty(),
        "workspace.members is empty; nothing would be checked"
    );

    for krate in &crates {
        let manifest_dir = workspace_root.join(krate);
        assert!(
            manifest_dir.join("Cargo.toml").is_file(),
            "{} is listed in workspace.members but has no Cargo.toml",
            krate
        );
        let version = crate_version(&manifest_dir);
        assert_eq!(
            version, "workspace",
            "{} should use version.workspace = true, got version = {:?}",
            krate, version
        );
    }
}

#[test]
fn workspace_version_matches_cargo_pkg() {
    let ws_version = workspace_version();
    let pkg_version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        ws_version, pkg_version,
        "workspace version ({}) != CARGO_PKG_VERSION ({})",
        ws_version, pkg_version
    );
}
