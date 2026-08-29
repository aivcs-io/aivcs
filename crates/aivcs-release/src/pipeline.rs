//! Full public-release orchestration (cargo / docker / gh / forge).

use crate::audit::audit_binary_file;
use crate::changelog::extract_section;
use crate::checksum::write_sha256sums;
use crate::expected_cli_version_line;
use crate::semver_lock::{normalize_semver, verify_semver_lockstep};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct ReleaseOptions {
    pub version: String,
    pub repo_root: PathBuf,
    pub build_dir: PathBuf,
    pub github_repo: String,
    pub homebrew_tap_dir: Option<PathBuf>,
    pub verify_only: bool,
    pub skip_linux: bool,
    pub skip_github: bool,
    pub skip_homebrew: bool,
    pub skip_forge: bool,
    pub docker_release: bool,
    pub homebrew_test: bool,
}

const ARTIFACTS: &[&str] = &[
    "aivcs-darwin-arm64",
    "aivcs-linux-arm64",
    "aivcs-linux-x86_64",
];

pub fn run_release(opts: ReleaseOptions) -> Result<()> {
    let version = normalize_semver(&opts.version)?;
    let tag = format!("v{version}");
    let root = opts.repo_root.canonicalize().unwrap_or(opts.repo_root.clone());

    info!("AIVCS public release {tag}");
    info!("repository={}", root.display());
    info!("artifacts={}", opts.build_dir.display());

    // Gate 1
    info!("==> [Gate 1] Semver lockstep");
    verify_semver_lockstep(&root, &version)?;
    info!("  [OK] Cargo.toml + CHANGELOG + docs pins = {version}");

    if std::env::var_os("CARGO_REGISTRIES_AIVCS_TOKEN").is_none() {
        warn!("CARGO_REGISTRIES_AIVCS_TOKEN unset; cargo may use ~/.cargo/credentials");
    }

    if opts.verify_only {
        info!("verify-only: skipping build/publish gates");
        return Ok(());
    }

    fs::create_dir_all(&opts.build_dir)?;
    run_checked(
        Command::new("cargo")
            .args(["check", "-p", "aivcs-cli", "--quiet"])
            .current_dir(&root),
        "cargo check -p aivcs-cli",
    )?;

    // Gate 2 — host (Darwin) binary
    info!("==> [Gate 2] Build host release binary (aivcs-cli)");
    run_checked(
        Command::new("cargo")
            .args(["build", "--release", "-p", "aivcs-cli"])
            .current_dir(&root),
        "cargo build --release -p aivcs-cli",
    )?;
    let host_bin = root.join("target/release/aivcs");
    let darwin_out = opts.build_dir.join("aivcs-darwin-arm64");
    fs::copy(&host_bin, &darwin_out).context("copy host aivcs → aivcs-darwin-arm64")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&darwin_out)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&darwin_out, perms)?;
    }

    // Gate 3 — Linux via Docker (optional skip for local dry runs)
    if opts.skip_linux {
        warn!("==> [Gate 3] SKIP Linux builds (--skip-linux)");
        // Still require placeholders only when not publishing github
    } else {
        info!("==> [Gate 3] Build Linux arm64 + amd64 via Docker");
        docker_linux_build(&root, &opts.build_dir, "linux-arm64", None, "aivcs-linux-arm64")?;
        docker_linux_build(
            &root,
            &opts.build_dir,
            "linux-amd64",
            Some("linux/amd64"),
            "aivcs-linux-x86_64",
        )?;
    }

    let expected = expected_cli_version_line(&version);

    // Gate 4 — version strings
    info!("==> [Gate 4] Runtime --version assertions");
    assert_version_output(&darwin_out, &expected, "aivcs-darwin-arm64")?;
    if !opts.skip_linux {
        assert_linux_version_via_docker(
            &opts.build_dir,
            "aivcs-linux-arm64",
            &expected,
            None,
        )?;
        assert_linux_version_via_docker(
            &opts.build_dir,
            "aivcs-linux-x86_64",
            &expected,
            Some("linux/amd64"),
        )?;
    }

    // Gate 5 — strings audit
    info!("==> [Gate 5] Binary deployment-hostname audit");
    let to_audit: Vec<PathBuf> = if opts.skip_linux {
        vec![darwin_out.clone()]
    } else {
        ARTIFACTS
            .iter()
            .map(|n| opts.build_dir.join(n))
            .collect()
    };
    for path in &to_audit {
        audit_binary_file(path)?;
        info!("  [OK] {} clean", path.file_name().unwrap().to_string_lossy());
    }

    // Gate 6 — checksums
    info!("==> [Gate 6] SHA256SUMS");
    let names: Vec<&str> = if opts.skip_linux {
        vec!["aivcs-darwin-arm64"]
    } else {
        ARTIFACTS.to_vec()
    };
    let digests = write_sha256sums(&opts.build_dir, &names)?;
    for (name, dig) in &digests {
        info!("  {dig}  {name}");
    }

    let notes = {
        let changelog = fs::read_to_string(root.join("CHANGELOG.md"))?;
        extract_section(&changelog, &version)?
    };

    // Gate 7 — GitHub
    if opts.skip_github {
        warn!("==> [Gate 7] SKIP GitHub release (--skip-github)");
    } else {
        info!("==> [Gate 7] GitHub release {tag}");
        publish_github_release(&opts, &tag, &version, &notes, &names)?;
    }

    // Gate 8 — Homebrew
    if opts.skip_homebrew {
        warn!("==> [Gate 8] SKIP Homebrew (--skip-homebrew)");
    } else {
        update_homebrew(&opts, &version, &digests)?;
    }

    write_in_repo_formula(&root, &version)?;

    // Gate 9 — Docker images
    if opts.docker_release && !opts.skip_linux {
        info!("==> [Gate 9] Docker images");
        docker_images(&opts.build_dir, &version)?;
    } else {
        info!("==> [Gate 9] SKIP Docker (pass --docker-release)");
    }

    // Gate 10 — forge
    if opts.skip_forge {
        warn!("==> [Gate 10] SKIP forge publish (--skip-forge)");
    } else {
        info!("==> [Gate 10] Forge publish");
        ensure_forge_configured()?;
        run_checked(
            Command::new(host_bin)
                .args([
                    "publish",
                    "--repo",
                    "aivcs/aivcs",
                    "--branch",
                    "main",
                    "-m",
                    &format!("chore(release): v{version}"),
                    ".",
                ])
                .current_dir(&root),
            "aivcs publish",
        )?;
    }

    info!("SUCCESS: {tag} — version, changelog, binaries, and notes are locked.");
    Ok(())
}

fn run_checked(cmd: &mut Command, label: &str) -> Result<()> {
    let status = cmd
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawn {label}"))?;
    if !status.success() {
        bail!("{label} failed with {status}");
    }
    Ok(())
}

fn docker_linux_build(
    root: &Path,
    build_dir: &Path,
    target_dir_name: &str,
    platform: Option<&str>,
    out_name: &str,
) -> Result<()> {
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/src", root.display()),
        "-v".into(),
        format!("{}:/out", build_dir.display()),
        "-w".into(),
        "/src".into(),
    ];
    // Prefer an explicit Bearer token. Host `~/.cargo/config.toml` [env] overrides
    // without a scheme break sparse registry auth inside Docker.
    let registry_token = registry_aivcs_token();
    if let Some(ref tok) = registry_token {
        args.push("-e".into());
        args.push(format!("CARGO_REGISTRIES_AIVCS_TOKEN={tok}"));
    } else {
        warn!("no aivcs registry token; Linux Docker builds may 401");
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        let cfg = home.join(".cargo/config.toml");
        if cfg.is_file() {
            args.push("-v".into());
            args.push(format!("{}:/root/.cargo/config.toml:ro", cfg.display()));
        }
    }
    if let Some(p) = platform {
        args.push("--platform".into());
        args.push(p.into());
    }
    args.push("rust:latest".into());
    args.push("bash".into());
    args.push("-c".into());
    args.push(format!(
        "apt-get update -qq && apt-get install -qq -y pkg-config libssl-dev cmake git protobuf-compiler && \
         cargo build --release -p aivcs-cli --target-dir /tmp/cargo-target-{target_dir_name} && \
         cp /tmp/cargo-target-{target_dir_name}/release/aivcs /out/{out_name}"
    ));

    run_checked(Command::new("docker").args(&args), &format!("docker build {out_name}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let out = build_dir.join(out_name);
        let mut perms = fs::metadata(&out)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&out, perms)?;
    }
    Ok(())
}

fn assert_version_output(bin: &Path, expected: &str, label: &str) -> Result<()> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .with_context(|| format!("run {label} --version"))?;
    if !out.status.success() {
        bail!("{label} --version failed: {}", out.status);
    }
    let got = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_end_matches('\r')
        .to_string();
    if got != expected {
        bail!("{label} reported '{got}', expected '{expected}'");
    }
    info!("  [OK] {label}: {got}");
    Ok(())
}

fn assert_linux_version_via_docker(
    build_dir: &Path,
    name: &str,
    expected: &str,
    platform: Option<&str>,
) -> Result<()> {
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/b", build_dir.display()),
    ];
    if let Some(p) = platform {
        args.push("--platform".into());
        args.push(p.into());
    }
    args.push("debian:trixie-slim".into());
    args.push(format!("/b/{name}"));
    args.push("--version".into());

    let out = Command::new("docker")
        .args(&args)
        .output()
        .with_context(|| format!("docker --version for {name}"))?;
    if !out.status.success() {
        bail!(
            "docker run {name} --version failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let got = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_end_matches('\r')
        .to_string();
    if got != expected {
        bail!("{name} reported '{got}', expected '{expected}'");
    }
    info!("  [OK] {name}: {got}");
    Ok(())
}

fn publish_github_release(
    opts: &ReleaseOptions,
    tag: &str,
    version: &str,
    notes: &str,
    artifact_names: &[&str],
) -> Result<()> {
    let notes_path = opts.build_dir.join("RELEASE_NOTES.md");
    fs::write(&notes_path, notes)?;

    let exists = Command::new("gh")
        .args(["release", "view", tag])
        .env("GH_REPO", &opts.github_repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let mut asset_args: Vec<String> = artifact_names.iter().map(|s| s.to_string()).collect();
    asset_args.push("SHA256SUMS".into());

    if !exists {
        let mut args = vec![
            "release".into(),
            "create".into(),
            tag.into(),
            "--title".into(),
            format!("v{version}"),
            "--notes-file".into(),
            notes_path.display().to_string(),
        ];
        args.extend(asset_args);
        run_checked(
            Command::new("gh")
                .args(&args)
                .env("GH_REPO", &opts.github_repo)
                .current_dir(&opts.build_dir),
            "gh release create",
        )?;
    } else {
        let mut args = vec!["release".into(), "upload".into(), tag.into()];
        args.extend(asset_args);
        args.push("--clobber".into());
        run_checked(
            Command::new("gh")
                .args(&args)
                .env("GH_REPO", &opts.github_repo)
                .current_dir(&opts.build_dir),
            "gh release upload",
        )?;
        run_checked(
            Command::new("gh")
                .args([
                    "release",
                    "edit",
                    tag,
                    "--notes-file",
                    &notes_path.display().to_string(),
                ])
                .env("GH_REPO", &opts.github_repo),
            "gh release edit notes",
        )?;
    }
    Ok(())
}

fn update_homebrew(
    opts: &ReleaseOptions,
    version: &str,
    digests: &[(String, String)],
) -> Result<()> {
    let Some(tap) = opts.homebrew_tap_dir.as_ref() else {
        info!("==> [Gate 8] SKIP Homebrew (no tap dir)");
        return Ok(());
    };
    if !tap.is_dir() {
        info!("==> [Gate 8] SKIP Homebrew (missing {})", tap.display());
        return Ok(());
    }
    info!("==> [Gate 8] Homebrew formula → {}", tap.display());

    let digest = |name: &str| -> Result<&str> {
        digests
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.as_str())
            .with_context(|| format!("missing digest for {name}"))
    };

    let formula = format!(
        r##"# typed: false
# frozen_string_literal: true

class Aivcs < Formula
  desc "AI Version Control System for Autonomous Agent Swarms"
  homepage "https://github.com/aivcs-io/aivcs"
  version "{version}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/aivcs-io/aivcs/releases/download/v#{{version}}/aivcs-darwin-arm64"
      sha256 "{sha_darwin}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/aivcs-io/aivcs/releases/download/v#{{version}}/aivcs-linux-arm64"
      sha256 "{sha_linux_arm}"
    end
    on_intel do
      url "https://github.com/aivcs-io/aivcs/releases/download/v#{{version}}/aivcs-linux-x86_64"
      sha256 "{sha_linux_amd}"
    end
  end

  def install
    binary = if OS.mac?
      "aivcs-darwin-arm64"
    elsif Hardware::CPU.arm?
      "aivcs-linux-arm64"
    else
      "aivcs-linux-x86_64"
    end
    bin.install binary => "aivcs"
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/aivcs --version")
  end
end
"##,
        version = version,
        sha_darwin = digest("aivcs-darwin-arm64").unwrap_or("SKIP"),
        sha_linux_arm = digest("aivcs-linux-arm64").unwrap_or("SKIP"),
        sha_linux_amd = digest("aivcs-linux-x86_64").unwrap_or("SKIP"),
    );

    let formula_path = tap.join("Formula/aivcs.rb");
    fs::create_dir_all(formula_path.parent().unwrap())?;
    fs::write(&formula_path, formula)?;

    run_checked(
        Command::new("git")
            .args(["add", "Formula/aivcs.rb"])
            .current_dir(tap),
        "git add formula",
    )?;
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(tap)
        .status()?;
    if !staged.success() {
        run_checked(
            Command::new("git")
                .args([
                    "commit",
                    "-m",
                    &format!("chore(release): bump aivcs formula to {version}"),
                ])
                .current_dir(tap),
            "git commit formula",
        )?;
        run_checked(
            Command::new("git")
                .args(["push", "origin", "HEAD"])
                .current_dir(tap),
            "git push tap",
        )?;
    }

    if opts.homebrew_test {
        run_checked(
            Command::new("brew").args(["reinstall", "aivcs-io/tap/aivcs"]),
            "brew reinstall",
        )?;
        run_checked(Command::new("brew").args(["test", "aivcs"]), "brew test")?;
    }
    Ok(())
}

fn write_in_repo_formula(root: &Path, version: &str) -> Result<()> {
    let formula = format!(
        r##"# typed: false
# frozen_string_literal: true

class Aivcs < Formula
  desc "AI Version Control System for Autonomous Agent Swarms"
  homepage "https://github.com/aivcs-io/aivcs"
  url "https://github.com/aivcs-io/aivcs/archive/refs/tags/v{version}.tar.gz"
  version "{version}"
  license "Apache-2.0"
  # Prefer the aivcs-io/homebrew-tap bottle formula for prebuilt binaries.

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/aivcs-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/aivcs --version")
  end
end
"##,
        version = version
    );
    let path = root.join("Formula/aivcs.rb");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, formula)?;
    Ok(())
}

fn docker_images(build_dir: &Path, version: &str) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("aivcs-docker-v{version}"));
    fs::create_dir_all(&tmp)?;
    fs::write(
        tmp.join("Dockerfile"),
        r#"FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git libssl3 && rm -rf /var/lib/apt/lists/*
ARG TARGETARCH
COPY aivcs-linux-${TARGETARCH} /usr/local/bin/aivcs
RUN chmod +x /usr/local/bin/aivcs
ENTRYPOINT ["/usr/local/bin/aivcs"]
"#,
    )?;
    fs::copy(
        build_dir.join("aivcs-linux-arm64"),
        tmp.join("aivcs-linux-arm64"),
    )?;
    fs::copy(
        build_dir.join("aivcs-linux-x86_64"),
        tmp.join("aivcs-linux-amd64"),
    )?;
    run_checked(
        Command::new("docker").args([
            "build",
            "--platform",
            "linux/arm64",
            "-t",
            &format!("ghcr.io/aivcs-io/aivcs:{version}-arm64"),
            "--build-arg",
            "TARGETARCH=arm64",
            &tmp.display().to_string(),
        ]),
        "docker build arm64",
    )?;
    run_checked(
        Command::new("docker").args([
            "build",
            "--platform",
            "linux/amd64",
            "-t",
            &format!("ghcr.io/aivcs-io/aivcs:{version}-amd64"),
            "--build-arg",
            "TARGETARCH=amd64",
            &tmp.display().to_string(),
        ]),
        "docker build amd64",
    )?;
    run_checked(
        Command::new("docker").args([
            "tag",
            &format!("ghcr.io/aivcs-io/aivcs:{version}-arm64"),
            &format!("ghcr.io/aivcs-io/aivcs:{version}"),
        ]),
        "docker tag version",
    )?;
    run_checked(
        Command::new("docker").args([
            "tag",
            &format!("ghcr.io/aivcs-io/aivcs:{version}-arm64"),
            "ghcr.io/aivcs-io/aivcs:latest",
        ]),
        "docker tag latest",
    )?;
    Ok(())
}

/// Resolve a Bearer-prefixed token for `registry.aivcs.io` (env or credentials.toml).
fn registry_aivcs_token() -> Option<String> {
    if let Ok(tok) = std::env::var("CARGO_REGISTRIES_AIVCS_TOKEN") {
        let t = tok.trim();
        if !t.is_empty() {
            return Some(ensure_bearer(t));
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let creds = home.join(".cargo/credentials.toml");
    let text = fs::read_to_string(creds).ok()?;
    let mut section = String::new();
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s.to_string();
            continue;
        }
        if section == "[registries.aivcs]" && s.starts_with("token") {
            if let Some((_, raw)) = s.split_once('=') {
                let val = raw.trim().trim_matches('"').trim_matches('\'').trim();
                if !val.is_empty() {
                    return Some(ensure_bearer(val));
                }
            }
        }
    }
    None
}

fn ensure_bearer(token: &str) -> String {
    let t = token.trim();
    if t.to_ascii_lowercase().starts_with("bearer ") {
        t.to_string()
    } else {
        format!("Bearer {t}")
    }
}

fn ensure_forge_configured() -> Result<()> {
    if std::env::var_os("AIVCS_FORGE_URL").is_some() {
        return Ok(());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if home.join(".aivcs/config.json").is_file() {
        return Ok(());
    }
    if std::env::var_os("AIVCS_HOME").map(PathBuf::from).is_some_and(|p| p.join("config.json").is_file())
    {
        return Ok(());
    }
    bail!("forge URL not configured (AIVCS_FORGE_URL or ~/.aivcs/config.json)");
}
