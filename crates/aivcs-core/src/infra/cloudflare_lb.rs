//! Cloudflare Load Balancer pool audit + orphan prune (GitOps allowlist driven).

use std::collections::HashSet;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const CF_API_BASE: &str = "https://api.cloudflare.com/client/v4";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfPool {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub origins: Vec<CfOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfOrigin {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfLoadBalancer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default_pools: Vec<String>,
    #[serde(default)]
    pub fallback_pool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanPool {
    pub name: String,
    pub id: String,
    pub referenced_by_lb: bool,
    pub lb_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub allowlist: Vec<String>,
    pub canonical_pools: Vec<String>,
    pub orphans: Vec<OrphanPool>,
    pub missing_from_cf: Vec<String>,
}

/// Parse allowlist file: one pool name per line; `#` starts comments.
pub fn parse_allowlist(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

pub fn pool_referenced(name: &str, lbs: &[CfLoadBalancer]) -> (bool, Vec<String>) {
    let mut refs = Vec::new();
    for lb in lbs {
        if lb.default_pools.iter().any(|pool| pool == name) {
            refs.push(lb.name.clone());
        }
        if lb.fallback_pool.as_deref() == Some(name) {
            refs.push(format!("{} (fallback)", lb.name));
        }
    }
    (!refs.is_empty(), refs)
}

pub fn build_audit_report(
    allowlist: &[String],
    pools: &[CfPool],
    lbs: &[CfLoadBalancer],
) -> AuditReport {
    let allowset: HashSet<&str> = allowlist.iter().map(String::as_str).collect();
    let pool_names: HashSet<&str> = pools.iter().map(|p| p.name.as_str()).collect();

    let canonical_pools: Vec<String> = pools
        .iter()
        .filter(|p| allowset.contains(p.name.as_str()))
        .map(|p| p.name.clone())
        .collect();

    let orphans: Vec<OrphanPool> = pools
        .iter()
        .filter(|p| !allowset.contains(p.name.as_str()))
        .map(|p| {
            let (referenced, lb_names) = pool_referenced(&p.name, lbs);
            OrphanPool {
                name: p.name.clone(),
                id: p.id.clone(),
                referenced_by_lb: referenced,
                lb_names,
            }
        })
        .collect();

    let missing_from_cf: Vec<String> = allowlist
        .iter()
        .filter(|name| !pool_names.contains(name.as_str()))
        .cloned()
        .collect();

    AuditReport {
        allowlist: allowlist.to_vec(),
        canonical_pools,
        orphans,
        missing_from_cf,
    }
}

pub fn render_audit_markdown(report: &AuditReport) -> String {
    let mut out = String::new();
    out.push_str("# Cloudflare LB audit\n\n");
    out.push_str("## Allowlist (git)\n");
    for name in &report.allowlist {
        out.push_str(&format!("- `{name}`\n"));
    }
    out.push('\n');
    out.push_str("## Canonical pools (live + allowlisted)\n");
    if report.canonical_pools.is_empty() {
        out.push_str("- _(none)_\n");
    } else {
        for name in &report.canonical_pools {
            out.push_str(&format!("- `{name}`\n"));
        }
    }
    out.push('\n');
    out.push_str("## Orphans (ClickOps drift)\n");
    if report.orphans.is_empty() {
        out.push_str("- _(none)_\n");
    } else {
        for orphan in &report.orphans {
            let refs = if orphan.referenced_by_lb {
                format!("referenced by: {}", orphan.lb_names.join(", "))
            } else {
                "unreferenced — safe to prune".to_string()
            };
            out.push_str(&format!("- `{}` ({refs})\n", orphan.name));
        }
    }
    out.push('\n');
    out.push_str("## Missing from Cloudflare (declared in git, absent live)\n");
    if report.missing_from_cf.is_empty() {
        out.push_str("- _(none)_\n");
    } else {
        for name in &report.missing_from_cf {
            out.push_str(&format!("- `{name}` — run sync reconciler\n"));
        }
    }
    out
}

pub async fn fetch_pools(client: &Client, account_id: &str, token: &str) -> Result<Vec<CfPool>> {
    let url = format!("{CF_API_BASE}/accounts/{account_id}/load_balancers/pools");
    let resp: serde_json::Value = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("Cloudflare pools list failed")?
        .error_for_status()
        .context("Cloudflare pools API error")?
        .json()
        .await?;
    Ok(parse_pools(&resp))
}

pub async fn fetch_load_balancers(
    client: &Client,
    account_id: &str,
    token: &str,
) -> Result<Vec<CfLoadBalancer>> {
    let url = format!("{CF_API_BASE}/accounts/{account_id}/load_balancers");
    let resp: serde_json::Value = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("Cloudflare load balancers list failed")?
        .error_for_status()
        .context("Cloudflare load balancers API error")?
        .json()
        .await?;
    Ok(parse_load_balancers(&resp))
}

fn parse_pools(resp: &serde_json::Value) -> Vec<CfPool> {
    resp["result"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(CfPool {
                        id: item["id"].as_str()?.to_string(),
                        name: item["name"].as_str()?.to_string(),
                        description: item["description"].as_str().unwrap_or("").to_string(),
                        origins: item["origins"]
                            .as_array()
                            .map(|origins| {
                                origins
                                    .iter()
                                    .filter_map(|o| {
                                        Some(CfOrigin {
                                            name: o["name"].as_str()?.to_string(),
                                            address: o["address"].as_str()?.to_string(),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_load_balancers(resp: &serde_json::Value) -> Vec<CfLoadBalancer> {
    resp["result"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let default_pools = item["default_pools"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(CfLoadBalancer {
                        id: item["id"].as_str()?.to_string(),
                        name: item["name"].as_str()?.to_string(),
                        default_pools,
                        fallback_pool: item["fallback_pool"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn delete_pool(
    client: &Client,
    account_id: &str,
    token: &str,
    pool_id: &str,
) -> Result<()> {
    let url = format!("{CF_API_BASE}/accounts/{account_id}/load_balancers/pools/{pool_id}");
    client
        .delete(&url)
        .bearer_auth(token)
        .send()
        .await
        .context(format!("Cloudflare delete pool {pool_id} failed"))?
        .error_for_status()
        .context(format!("Cloudflare rejected delete for pool {pool_id}"))?;
    Ok(())
}

/// Prune unreferenced orphan pools. Returns deleted pool names.
pub async fn prune_orphans(
    client: &Client,
    account_id: &str,
    token: &str,
    report: &AuditReport,
    dry_run: bool,
) -> Result<Vec<String>> {
    let mut deleted = Vec::new();
    for orphan in &report.orphans {
        if orphan.referenced_by_lb {
            continue;
        }
        if dry_run {
            deleted.push(format!("{} (dry-run)", orphan.name));
            continue;
        }
        delete_pool(client, account_id, token, &orphan.id).await?;
        deleted.push(orphan.name.clone());
    }
    Ok(deleted)
}

/// Resolve CF credentials from standard env vars (ESO-friendly).
pub fn resolve_cf_credentials() -> Result<(String, String)> {
    let token = std::env::var("CF_API_TOKEN")
        .or_else(|_| std::env::var("CLOUDFLARE_API_TOKEN"))
        .context("CF_API_TOKEN or CLOUDFLARE_API_TOKEN must be set")?;
    let trimmed = token.trim();
    anyhow::ensure!(!trimmed.is_empty(), "Cloudflare API token is empty");
    let account_id = std::env::var("CLOUDFLARE_ACCOUNT_ID")
        .or_else(|_| std::env::var("CF_ACCOUNT_ID"))
        .context("CLOUDFLARE_ACCOUNT_ID or CF_ACCOUNT_ID must be set")?;
    Ok((trimmed.to_string(), account_id.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allowlist_skips_comments_and_blanks() {
        let list = parse_allowlist("# canonical\nlornu-ai-origins\n\nstevedores-org-origins\n");
        assert_eq!(
            list,
            vec![
                "lornu-ai-origins".to_string(),
                "stevedores-org-origins".to_string()
            ]
        );
    }

    #[test]
    fn audit_finds_orphans_and_missing() {
        let allow = vec![
            "lornu-ai-origins".to_string(),
            "aivcs-io-origins".to_string(),
        ];
        let pools = vec![
            CfPool {
                id: "p1".into(),
                name: "lornu-ai-origins".into(),
                description: String::new(),
                origins: vec![],
            },
            CfPool {
                id: "p2".into(),
                name: "aks-lornu-hub".into(),
                description: String::new(),
                origins: vec![],
            },
        ];
        let lbs = vec![CfLoadBalancer {
            id: "lb1".into(),
            name: "lornu.ai".into(),
            default_pools: vec!["lornu-ai-origins".into()],
            fallback_pool: None,
        }];
        let report = build_audit_report(&allow, &pools, &lbs);
        assert_eq!(report.canonical_pools, vec!["lornu-ai-origins"]);
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].name, "aks-lornu-hub");
        assert!(!report.orphans[0].referenced_by_lb);
        assert_eq!(report.missing_from_cf, vec!["aivcs-io-origins"]);
    }
}
