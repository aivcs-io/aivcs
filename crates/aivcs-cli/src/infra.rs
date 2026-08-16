//! `aivcs infra` — sovereign infra reconcilers (no GitHub Actions).

use anyhow::{Context, Result};
use std::path::PathBuf;

use aivcs_core::infra::cloudflare_lb::{
    build_audit_report, fetch_load_balancers, fetch_pools, parse_allowlist, prune_orphans,
    render_audit_markdown, resolve_cf_credentials,
};

#[derive(clap::Subcommand)]
pub enum InfraAction {
    /// Cloudflare Load Balancer pool hygiene
    CloudflareLb {
        #[command(subcommand)]
        action: CloudflareLbAction,
    },
    /// Flux GitOps reconcile (in-cluster; replaces GHA kubectl jobs)
    Flux {
        #[command(subcommand)]
        action: FluxAction,
    },
}

#[derive(clap::Subcommand)]
pub enum CloudflareLbAction {
    /// Compare live CF pools against a git allowlist
    Audit {
        /// Allowlist file (one pool name per line; `#` comments)
        #[arg(short, long)]
        allowlist: PathBuf,

        /// Emit JSON instead of Markdown
        #[arg(long)]
        json: bool,
    },
    /// Delete unreferenced orphan pools (respects LB references)
    Prune {
        #[arg(short, long)]
        allowlist: PathBuf,

        /// List candidates without calling DELETE
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum FluxAction {
    /// `flux reconcile kustomization <name> --with-source`
    Reconcile {
        #[arg(short, long)]
        kustomization: String,

        #[arg(short, long, default_value = "flux-system")]
        namespace: String,

        #[arg(long, default_value_t = true)]
        with_source: bool,
    },
}

pub async fn run(action: InfraAction) -> Result<()> {
    match action {
        InfraAction::CloudflareLb { action } => run_cloudflare_lb(action).await,
        InfraAction::Flux { action } => run_flux(action),
    }
}

async fn run_cloudflare_lb(action: CloudflareLbAction) -> Result<()> {
    match action {
        CloudflareLbAction::Audit { allowlist, json } => {
            let content = std::fs::read_to_string(&allowlist)
                .with_context(|| format!("read allowlist {:?}", allowlist))?;
            let allow = parse_allowlist(&content);
            let (token, account_id) = resolve_cf_credentials()?;
            let http = reqwest::Client::new();
            let pools = fetch_pools(&http, &account_id, &token).await?;
            let lbs = fetch_load_balancers(&http, &account_id, &token).await?;
            let report = build_audit_report(&allow, &pools, &lbs);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", render_audit_markdown(&report));
            }
            if !report.orphans.is_empty() {
                std::process::exit(2);
            }
            Ok(())
        }
        CloudflareLbAction::Prune { allowlist, dry_run } => {
            let content = std::fs::read_to_string(&allowlist)
                .with_context(|| format!("read allowlist {:?}", allowlist))?;
            let allow = parse_allowlist(&content);
            let (token, account_id) = resolve_cf_credentials()?;
            let http = reqwest::Client::new();
            let pools = fetch_pools(&http, &account_id, &token).await?;
            let lbs = fetch_load_balancers(&http, &account_id, &token).await?;
            let report = build_audit_report(&allow, &pools, &lbs);
            let deleted = prune_orphans(&http, &account_id, &token, &report, dry_run).await?;
            for name in &deleted {
                if dry_run {
                    println!("would prune: {name}");
                } else {
                    println!("pruned: {name}");
                }
            }
            Ok(())
        }
    }
}

fn run_flux(action: FluxAction) -> Result<()> {
    match action {
        FluxAction::Reconcile {
            kustomization,
            namespace,
            with_source,
        } => {
            aivcs_core::run_reconcile(&kustomization, &namespace, with_source)?;
            println!("✓ reconciled kustomization {kustomization} in {namespace}");
            Ok(())
        }
    }
}
