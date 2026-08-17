//! GitLab REST API for zero-touch merge request pipelines (GitHub-free CI).

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use reqwest::Client;
use tracing::info;

/// GitLab project API client (`owner/repo` → URL-encoded project path).
pub struct GitLabClient {
    http: Client,
    api_base: String,
    token: String,
    project_path: String,
}

impl GitLabClient {
    pub fn new(token: String, owner: String, repo: String) -> Result<Self> {
        let project_path = format!("{owner}/{repo}");
        let api_base = std::env::var("GITLAB_API_URL")
            .unwrap_or_else(|_| "https://gitlab.com/api/v4".to_string())
            .trim_end_matches('/')
            .to_string();
        Ok(Self {
            http: Client::new(),
            api_base,
            token,
            project_path,
        })
    }

    fn project_enc(&self) -> String {
        urlencoding::encode(&self.project_path)
    }

    async fn project_id(&self) -> Result<u64> {
        let url = format!("{}/projects/{}", self.api_base, self.project_enc());
        let project: serde_json::Value = self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await
            .context("GitLab project lookup failed")?
            .error_for_status()
            .context(format!("GitLab project '{}' not found", self.project_path))?
            .json()
            .await?;
        project["id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("GitLab project response missing id"))
    }

    pub async fn create_branch(&self, branch_name: &str, base: &str) -> Result<String> {
        info!("Creating GitLab branch '{}' from '{}'", branch_name, base);
        let project_id = self.project_id().await?;
        let url = format!(
            "{}/projects/{project_id}/repository/branches",
            self.api_base
        );
        let body = serde_json::json!({
            "branch": branch_name,
            "ref": base,
        });
        let resp: serde_json::Value = self
            .http
            .post(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(&body)
            .send()
            .await
            .context("GitLab branch create failed")?
            .error_for_status()
            .context(format!("failed to create branch '{branch_name}'"))?
            .json()
            .await?;
        let sha = resp["commit"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("GitLab branch response missing commit id"))?;
        Ok(sha.to_string())
    }

    pub async fn commit_file(
        &self,
        branch: &str,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<String> {
        info!("Committing '{}' to GitLab branch '{}'", path, branch);
        let project_id = self.project_id().await?;
        let action = if self.file_exists(project_id, path, branch).await? {
            "update"
        } else {
            "create"
        };
        let url = format!("{}/projects/{project_id}/repository/commits", self.api_base);
        let body = serde_json::json!({
            "branch": branch,
            "commit_message": message,
            "actions": [{
                "action": action,
                "file_path": path,
                "content": STANDARD.encode(content),
                "encoding": "base64",
            }],
        });
        let resp: serde_json::Value = self
            .http
            .post(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(&body)
            .send()
            .await
            .context("GitLab commit failed")?
            .error_for_status()
            .context(format!("failed to commit file '{path}'"))?
            .json()
            .await?;
        let sha = resp["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("GitLab commit response missing id"))?;
        Ok(sha.to_string())
    }

    async fn file_exists(&self, project_id: u64, path: &str, branch: &str) -> Result<bool> {
        let encoded_path = urlencoding::encode(path);
        let url = format!(
            "{}/projects/{project_id}/repository/files/{encoded_path}?ref={branch}",
            self.api_base
        );
        let resp = self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    pub async fn open_mr(
        &self,
        title: &str,
        body: &str,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<u64> {
        info!(
            "Opening GitLab MR: '{}' ({} → {})",
            title, source_branch, target_branch
        );
        let project_id = self.project_id().await?;
        let url = format!("{}/projects/{project_id}/merge_requests", self.api_base);
        let payload = serde_json::json!({
            "title": title,
            "description": body,
            "source_branch": source_branch,
            "target_branch": target_branch,
            "remove_source_branch": false,
        });
        let resp: serde_json::Value = self
            .http
            .post(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(&payload)
            .send()
            .await
            .context("GitLab merge request create failed")?
            .error_for_status()
            .context("failed to create merge request")?
            .json()
            .await?;
        let iid = resp["iid"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("GitLab MR response missing iid"))?;
        Ok(iid)
    }
}

/// Resolve a GitLab token from env or mounted secret file.
pub fn resolve_gitlab_token() -> Result<String> {
    if let Ok(token) = std::env::var("GITLAB_TOKEN") {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(path) = std::env::var("GITLAB_TOKEN_FILE") {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read GITLAB_TOKEN_FILE at '{path}'"))?;
        let trimmed = content.trim();
        anyhow::ensure!(
            !trimmed.is_empty(),
            "GITLAB_TOKEN_FILE at '{path}' is empty"
        );
        return Ok(trimmed.to_string());
    }
    anyhow::bail!("GITLAB_TOKEN or GITLAB_TOKEN_FILE must be set for GitLab API access")
}

mod urlencoding {
    pub fn encode(path: &str) -> String {
        path.split('/')
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("%2F")
    }

    fn encode_segment(segment: &str) -> String {
        let mut encoded = String::new();
        for byte in segment.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        encoded
    }
}
