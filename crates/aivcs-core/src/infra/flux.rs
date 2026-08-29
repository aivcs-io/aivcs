//! Flux reconcile helpers — in-cluster GitOps without GitHub Actions.

use std::process::Command;

use anyhow::{bail, Context, Result};

/// Validate Flux Kustomization names before shelling out (imperative guardrail).
pub fn validate_kustomization_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 63 {
        bail!("invalid kustomization name length");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("kustomization name must be alphanumeric + hyphen");
    }
    Ok(())
}

pub fn validate_namespace(ns: &str) -> Result<()> {
    if ns.is_empty() || ns.len() > 63 {
        bail!("invalid namespace");
    }
    Ok(())
}

/// Build a `flux reconcile kustomization … --with-source` command (does not execute).
pub fn build_reconcile_command(
    kustomization: &str,
    namespace: &str,
    with_source: bool,
) -> Result<Vec<String>> {
    validate_kustomization_name(kustomization)?;
    validate_namespace(namespace)?;
    let mut args = vec![
        "reconcile".to_string(),
        "kustomization".to_string(),
        kustomization.to_string(),
        "--namespace".to_string(),
        namespace.to_string(),
    ];
    if with_source {
        args.push("--with-source".to_string());
    }
    Ok(args)
}

/// Run `flux reconcile` with optional `--context` from `FLUX_CONTEXT` / `KUBECTL_CONTEXT`.
pub fn run_reconcile(kustomization: &str, namespace: &str, with_source: bool) -> Result<()> {
    let args = build_reconcile_command(kustomization, namespace, with_source)?;
    let mut cmd = Command::new("flux");
    cmd.args(&args);
    if let Ok(ctx) = std::env::var("FLUX_CONTEXT").or_else(|_| std::env::var("KUBECTL_CONTEXT")) {
        if !ctx.trim().is_empty() {
            cmd.args(["--context", ctx.trim()]);
        }
    }
    let output = cmd
        .output()
        .context("failed to spawn flux — is flux CLI installed and on PATH?")?;
    if !output.status.success() {
        bail!(
            "flux reconcile failed (exit {}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shell_metacharacters_in_ks_name() {
        assert!(validate_kustomization_name("cloudflare-lb-lornu-ai").is_ok());
        assert!(validate_kustomization_name("bad;name").is_err());
    }

    #[test]
    fn build_reconcile_includes_with_source() {
        let args = build_reconcile_command("infra-eso", "flux-system", true).unwrap();
        assert_eq!(
            args,
            vec![
                "reconcile",
                "kustomization",
                "infra-eso",
                "--namespace",
                "flux-system",
                "--with-source"
            ]
        );
    }
}
