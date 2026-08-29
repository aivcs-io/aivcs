//! `aivcs-release-public` — portable semver-accurate public release pipeline.

use aivcs_release::{run_release, ReleaseOptions};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "aivcs-release-public",
    about = "Cut an aivcs CLI public release with strict semver lockstep (no invented versions, no baked deployment URLs)"
)]
struct Cli {
    /// Target semver already present in Cargo.toml + CHANGELOG (e.g. 0.5.0 or v0.5.0)
    version: String,

    /// Repository root (default: cwd)
    #[arg(long, default_value = ".")]
    repo_root: PathBuf,

    /// Artifact output directory
    #[arg(long, env = "AIVCS_RELEASE_DIR")]
    build_dir: Option<PathBuf>,

    /// GitHub repo slug for `gh release`
    #[arg(long, default_value = "aivcs-io/aivcs", env = "GH_REPO")]
    github_repo: String,

    /// Homebrew tap checkout (optional)
    #[arg(long, env = "HOMEBREW_TAP_DIR")]
    homebrew_tap_dir: Option<PathBuf>,

    /// Only run Gate 1 (Cargo/CHANGELOG/docs lockstep)
    #[arg(long)]
    verify_only: bool,

    /// Skip Docker Linux cross-builds
    #[arg(long)]
    skip_linux: bool,

    /// Skip GitHub release upload
    #[arg(long)]
    skip_github: bool,

    /// Skip Homebrew tap update
    #[arg(long)]
    skip_homebrew: bool,

    /// Skip forge publish
    #[arg(long)]
    skip_forge: bool,

    /// Build/tag Docker images
    #[arg(long, env = "DOCKER_RELEASE")]
    docker_release: bool,

    /// Run `brew reinstall` + `brew test` after formula push
    #[arg(long, env = "HOMEBREW_TEST")]
    homebrew_test: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .without_time()
        .init();

    let cli = Cli::parse();
    let version = aivcs_release::normalize_semver(&cli.version)?;
    let build_dir = cli
        .build_dir
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/aivcs-release-v{version}")));

    let homebrew = cli.homebrew_tap_dir.or_else(|| {
        let p = PathBuf::from("/opt/homebrew/Library/Taps/aivcs-io/homebrew-tap");
        p.is_dir().then_some(p)
    });

    run_release(ReleaseOptions {
        version,
        repo_root: cli.repo_root,
        build_dir,
        github_repo: cli.github_repo,
        homebrew_tap_dir: homebrew,
        verify_only: cli.verify_only,
        skip_linux: cli.skip_linux,
        skip_github: cli.skip_github,
        skip_homebrew: cli.skip_homebrew,
        skip_forge: cli.skip_forge,
        docker_release: cli.docker_release,
        homebrew_test: cli.homebrew_test,
    })
}
