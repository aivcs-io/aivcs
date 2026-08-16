//! `aivcs oci publish` — Nix OCI → GAR (GitLab CI sovereign path).

use anyhow::{Context, Result};
use std::path::PathBuf;

use aivcs_core::infra::oci::{
    nix_build_image, package_version_from_manifest, resolve_gar_access_token, resolve_oci_target,
    resolve_push_tags, skopeo_push_image,
};

#[derive(clap::Subcommand)]
pub enum OciAction {
    /// Build Nix OCI image and push to GAR via skopeo
    Publish {
        /// dockworker.toml path
        #[arg(long, default_value = "dockworker.toml")]
        manifest: PathBuf,

        /// Target name from `[build].targets` (default: aivcs-cli)
        #[arg(long, default_value = "aivcs-cli")]
        target: String,

        /// Nix system triple (default: x86_64-linux)
        #[arg(long, default_value = "x86_64-linux")]
        system: String,

        /// Build only; skip registry push
        #[arg(long)]
        dry_run: bool,

        /// Extra tags (comma-separated)
        #[arg(long, value_delimiter = ',')]
        tag: Vec<String>,
    },
}

pub fn run(action: OciAction) -> Result<()> {
    match action {
        OciAction::Publish {
            manifest,
            target,
            system,
            dry_run,
            tag,
        } => publish(manifest, &target, &system, dry_run, &tag),
    }
}

fn publish(
    manifest: PathBuf,
    target_name: &str,
    system: &str,
    dry_run: bool,
    extra_tags: &[String],
) -> Result<()> {
    let repo_root = std::env::current_dir().context("cwd")?;
    let (target, registry) =
        resolve_oci_target(&manifest, target_name).context("resolve OCI target")?;
    let version = package_version_from_manifest(&manifest)?;
    let tags = resolve_push_tags(&version, &registry.default_tags, extra_tags);
    let gar_image = format!("{}/{}", registry.url, target.image);

    eprintln!(
        "building {} → {gar_image} (tags: {})",
        target.nix_output,
        tags.join(", ")
    );

    let result = nix_build_image(&repo_root, &target.nix_output, Some(system))?;

    if dry_run {
        eprintln!("dry-run: built {} — skipping push", result.display());
        return Ok(());
    }

    let token = resolve_gar_access_token()?;
    skopeo_push_image(&result, &gar_image, &tags, &token)?;
    eprintln!("✓ pushed {gar_image} ({})", tags.join(", "));
    Ok(())
}
