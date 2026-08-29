//! Signed merge-event POST to the og-builds#119 `serve` receiver.
//!
//! **B1** of `DESIGN_BUILD_ON_CLEAN_MERGE` (code-governance#801): when an
//! `AivcsRepository` tracking `main` advances its head, POST an
//! HMAC-authenticated payload matching og-builds `MergeEvent` so the build
//! plane can dispatch OCI publishes. Idempotency key is sanitized `repo@sha`.
//!
//! Enabled when `OG_BUILDS_MERGE_URL` is set. Requires `AIVCS_WEBHOOK_SECRET`
//! (≥32 bytes), shared with the receiver.

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const SIGNATURE_HEADER: &str = "x-aivcs-signature-256";

/// Default branch a merge-to-main trigger watches.
pub const DEFAULT_BRANCH: &str = "main";

/// Env: absolute URL of the og-builds merge-event receiver.
pub const ENV_MERGE_URL: &str = "OG_BUILDS_MERGE_URL";
/// Env: shared HMAC secret (must match og-builds `AIVCS_WEBHOOK_SECRET`).
pub const ENV_WEBHOOK_SECRET: &str = "AIVCS_WEBHOOK_SECRET";

/// Wire form matching `og_builds::merge_event::MergeEvent`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MergeEventPayload {
    pub event_id: String,
    pub repository: String,
    pub target_branch: String,
    pub commit_sha: String,
    pub pull_request: String,
    pub merged: bool,
    pub approved: bool,
    pub checks_passed: bool,
    pub flake: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl MergeEventPayload {
    /// Build a receiver payload from a clean `main` head advance.
    ///
    /// Trust model for this increment: protected `main` / swarm `save_branch`
    /// already gated the write, so `checks_passed` and `approved` are true when
    /// emitting for `main`.
    pub fn from_main_advance(organization: &str, repo: &str, commit_sha: &str) -> Result<Self> {
        let repository = format!("{organization}/{repo}");
        let event_id = event_id_for(&repository, commit_sha)?;
        Ok(Self {
            event_id,
            repository,
            target_branch: DEFAULT_BRANCH.to_string(),
            commit_sha: commit_sha.to_string(),
            pull_request: format!("aivcs@{commit_sha}"),
            merged: true,
            approved: true,
            checks_passed: true,
            flake: ".#container".into(),
            image: None,
        })
    }

    /// Source-agnostic dedup key `repo@revision` (build-once).
    pub fn dedup_key(&self) -> String {
        format!("{}@{}", self.repository, self.commit_sha)
    }
}

/// Idempotency key `repo@sha` sanitized for the receiver's `event_id` grammar
/// (ASCII alnum / `-` / `_`, ≤200 chars).
pub fn event_id_for(repository: &str, commit_sha: &str) -> Result<String> {
    let raw = format!("{repository}@{commit_sha}");
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized.len() > 200 {
        bail!(
            "event_id out of bounds after sanitize (len={})",
            sanitized.len()
        );
    }
    Ok(sanitized)
}

/// HMAC-SHA256 over the raw body — `sha256=<hex>`, matching og-builds#119.
pub fn sign_body(secret: &[u8], body: &[u8]) -> Result<String> {
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| anyhow!("invalid HMAC key: {e}"))?;
    mac.update(body);
    Ok(format!(
        "sha256={}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

/// Should this reconcile emit a merge event? Only on a **new** `main` head
/// (not restart republishes of an unchanged revision).
pub fn should_emit(branch: &str, head_advanced: bool) -> bool {
    head_advanced && branch == DEFAULT_BRANCH
}

/// Optional emitter config from the environment. `None` means emit is disabled.
#[derive(Debug, Clone)]
pub struct MergeEmitter {
    pub url: String,
    pub secret: Vec<u8>,
}

impl MergeEmitter {
    pub fn from_env() -> Result<Option<Self>> {
        let Ok(url) = std::env::var(ENV_MERGE_URL) else {
            return Ok(None);
        };
        let url = url.trim().to_string();
        if url.is_empty() {
            return Ok(None);
        }
        let secret = std::env::var(ENV_WEBHOOK_SECRET).with_context(|| {
            format!("{ENV_WEBHOOK_SECRET} required when {ENV_MERGE_URL} is set")
        })?;
        if secret.len() < 32 {
            bail!("{ENV_WEBHOOK_SECRET} must be at least 32 bytes");
        }
        Ok(Some(Self {
            url,
            secret: secret.into_bytes(),
        }))
    }

    /// POST a signed merge event. Returns the receiver response body on 2xx/202.
    pub async fn emit(&self, payload: &MergeEventPayload) -> Result<String> {
        let body = serde_json::to_vec(payload).context("serialize merge event")?;
        let signature = sign_body(&self.secret, &body)?;
        let client = reqwest::Client::new();
        let resp = client
            .post(&self.url)
            .header(SIGNATURE_HEADER, signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .with_context(|| format!("POST {}", self.url))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !(status.is_success() || status.as_u16() == 202) {
            bail!("og-builds merge receiver returned {status}: {text}");
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_sanitizes_repo_at_sha() {
        let id = event_id_for(
            "lornu-ai/og-builds",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
        assert!(id.starts_with("lornu-ai-og-builds-01234567"));
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    #[test]
    fn payload_from_main_is_green_and_receiver_shaped() {
        let p = MergeEventPayload::from_main_advance(
            "lornu-ai",
            "worker",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
        assert_eq!(p.repository, "lornu-ai/worker");
        assert_eq!(p.target_branch, "main");
        assert!(p.merged && p.approved && p.checks_passed);
        assert!(p.pull_request.starts_with("aivcs@"));
        assert_eq!(
            p.dedup_key(),
            "lornu-ai/worker@0123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn should_emit_only_on_main_head_advance() {
        assert!(should_emit("main", true));
        assert!(!should_emit("main", false));
        assert!(!should_emit("develop", true));
    }

    #[test]
    fn signature_matches_og_builds_receiver_vector() {
        // Same vector as og-builds `merge_event::tests::signature_is_authenticated…`.
        let secret = b"secret";
        let body = b"payload";
        assert_eq!(
            sign_body(secret, body).unwrap(),
            "sha256=b82fcb791acec57859b989b430a826488ce2e479fdf92326bd0a2e8375a42ba4"
        );
    }
}
