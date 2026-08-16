//! `aivcs-source-controller` — watch `AivcsRepository` CRs, resolve branch heads
//! via aivcsd forge-cas, and on a clean `main` advance emit a signed merge event
//! to og-builds#119 (DESIGN_BUILD_ON_CLEAN_MERGE B1).
//!
//! Homesteaded in apps-middle-ware alongside `aivcsd` (aivcs.io monorepo retired
//! from the lornu-ai org SoT path). ExternalArtifact packaging remains a
//! follow-up; this service owns the **build trigger** seam first.

use aivcs_merge_emit::{should_emit, MergeEmitter, MergeEventPayload};
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, CustomResource, ResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn, Level};

#[derive(CustomResource, Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "aivcs.lornu.ai",
    version = "v1alpha1",
    kind = "AivcsRepository",
    namespaced,
    status = "AivcsRepositoryStatus",
    shortname = "aivcsrepo",
    printcolumn = r#"{"name":"Repo", "type":"string", "jsonPath":".spec.repo"}"#,
    printcolumn = r#"{"name":"Branch", "type":"string", "jsonPath":".spec.branch"}"#,
    printcolumn = r#"{"name":"Revision", "type":"string", "jsonPath":".status.artifactRevision"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct AivcsRepositorySpec {
    #[serde(default = "default_organization")]
    pub organization: String,
    pub repo: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AivcsRepositoryStatus {
    pub artifact_revision: Option<String>,
}

/// Matches og-builds `AIVCS_ALLOWED_OWNER` (infra-code apps/og-builds).
fn default_organization() -> String {
    "aivcs".into()
}
fn default_branch() -> String {
    "main".into()
}
fn default_interval_seconds() -> u64 {
    30
}

struct ContextData {
    client: Client,
    /// Base URL for aivcsd forge API (no trailing slash), e.g. `http://aivcsd…`.
    aivcsd_url: String,
    merge_emitter: Option<MergeEmitter>,
    http: reqwest::Client,
}

#[derive(thiserror::Error, Debug)]
enum ReconcileError {
    #[error("{0}")]
    Any(#[from] anyhow::Error),
    #[error("kube: {0}")]
    Kube(#[from] kube::Error),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_max_level(Level::INFO)
        .init();

    let aivcsd_url = std::env::var("AIVCSD_URL")
        .context("AIVCSD_URL is required (forge branch-head resolution)")?
        .trim_end_matches('/')
        .to_string();

    let merge_emitter = match MergeEmitter::from_env()? {
        Some(e) => {
            info!(url = %e.url, "og-builds merge-event emit enabled");
            Some(e)
        }
        None => {
            warn!("OG_BUILDS_MERGE_URL unset — build-on-clean-merge emit disabled");
            None
        }
    };

    let client = Client::try_default()
        .await
        .context("failed to build kube client")?;
    let api: Api<AivcsRepository> = Api::all(client.clone());
    let ctx = Arc::new(ContextData {
        client,
        aivcsd_url,
        merge_emitter,
        http: reqwest::Client::new(),
    });

    info!("aivcs-source-controller starting (apps-middle-ware homestead)");
    Controller::new(api, Default::default())
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => info!(?o, "reconciled"),
                Err(e) => warn!(error = %e, "reconcile failed"),
            }
        })
        .await;
    Ok(())
}

async fn reconcile(
    obj: Arc<AivcsRepository>,
    ctx: Arc<ContextData>,
) -> Result<Action, ReconcileError> {
    let ns = obj.namespace().unwrap_or_else(|| "default".to_string());
    let name = obj.name_any();
    let spec = &obj.spec;
    let interval = Duration::from_secs(spec.interval_seconds.max(5));

    info!(
        namespace = %ns, name = %name, org = %spec.organization, repo = %spec.repo, branch = %spec.branch,
        "reconciling AivcsRepository"
    );

    let head = resolve_branch_head(&ctx, &spec.repo, &spec.branch).await?;
    let Some(head) = head else {
        info!(repo = %spec.repo, branch = %spec.branch, "branch not found yet — nothing to emit");
        return Ok(Action::requeue(interval));
    };

    let previous = obj
        .status
        .as_ref()
        .and_then(|s| s.artifact_revision.as_deref());
    let head_advanced = previous != Some(head.as_str());
    if !head_advanced {
        info!(revision = %head, "branch head unchanged — skip");
        return Ok(Action::requeue(interval));
    }

    // Emit **before** advancing status. If we wrote artifactRevision first and
    // the POST then failed, the next reconcile would see an unchanged head and
    // permanently drop the build trigger. Receiver claim is idempotent, so a
    // crash after a successful emit (before the status patch) only redelivers.
    if should_emit(&spec.branch, head_advanced) {
        if let Some(emitter) = ctx.merge_emitter.as_ref() {
            let payload =
                MergeEventPayload::from_main_advance(&spec.organization, &spec.repo, &head)?;
            match emitter.emit(&payload).await {
                Ok(body) => info!(
                    event_id = %payload.event_id,
                    repo = %payload.repository,
                    revision = %head,
                    response = %body,
                    "emitted merge event to og-builds"
                ),
                Err(e) => {
                    return Err(ReconcileError::Any(anyhow!(
                        "failed to emit merge event to og-builds (will retry; status not advanced): {e:#}"
                    )));
                }
            }
        }
    }

    let api: Api<AivcsRepository> = Api::namespaced(ctx.client.clone(), &ns);
    let status_patch = serde_json::json!({
        "status": { "artifactRevision": head }
    });
    api.patch_status(&name, &PatchParams::default(), &Patch::Merge(&status_patch))
        .await?;

    Ok(Action::requeue(interval))
}

async fn resolve_branch_head(
    ctx: &ContextData,
    repo: &str,
    branch: &str,
) -> Result<Option<String>> {
    let url = format!(
        "{}/api/v1/repos/{}/branches/{}",
        ctx.aivcsd_url,
        urlencoding_path(repo),
        urlencoding_path(branch)
    );
    let resp = ctx
        .http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail_status(resp.status(), &url).await?;
    }
    let body: serde_json::Value = resp.json().await.context("parse branch response")?;
    Ok(body
        .get("head_commit_id")
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

fn urlencoding_path(s: &str) -> String {
    // Repo / branch names are CR-validated alnum-ish; still escape for safety.
    s.split('/')
        .map(|p| {
            let mut out = String::new();
            for b in p.bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                        out.push(b as char)
                    }
                    _ => out.push_str(&format!("%{b:02X}")),
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}

async fn bail_status(status: reqwest::StatusCode, url: &str) -> Result<()> {
    Err(anyhow!("aivcsd {status} for {url}"))
}

fn error_policy(
    _obj: Arc<AivcsRepository>,
    err: &ReconcileError,
    _ctx: Arc<ContextData>,
) -> Action {
    warn!(error = %err, "reconcile error, retrying shortly");
    Action::requeue(Duration::from_secs(15))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivcs_merge_emit::should_emit;

    #[test]
    fn urlencoding_leaves_safe_repo_names() {
        assert_eq!(urlencoding_path("og-builds"), "og-builds");
        assert_eq!(urlencoding_path("lornu-ai/og-builds"), "lornu-ai/og-builds");
    }

    #[test]
    fn emit_gate_matches_library() {
        assert!(should_emit("main", true));
        assert!(!should_emit("main", false));
    }

    #[test]
    fn default_organization_matches_og_builds_allowed_owner() {
        assert_eq!(default_organization(), "aivcs");
    }

    #[test]
    fn payload_uses_organization_prefix_for_receiver_allowlist() {
        let p = MergeEventPayload::from_main_advance(
            &default_organization(),
            "infra-code",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
        assert_eq!(p.repository, "lornu-ai/infra-code");
    }
}
