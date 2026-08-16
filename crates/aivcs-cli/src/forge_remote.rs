//! Forge remote CAS operations for the AIVCS CLI.
//!
//! Parallel `publish`, `fetch`, `clone`, `push`, and `pull` against `aivcsd-lite`
//! (`/api/v1/*` content-addressed forge).

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};
use tracing::info;

/// In-cluster forge URL — ClusterIP service port 80 → pod 8080 (no port-forward).
pub const IN_CLUSTER_FORGE_URL: &str = "http://aivcsd-lite.aivcs-repo.svc.cluster.local";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub digest: String,
    pub executable: bool,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchResolution {
    pub repo: String,
    pub branch: String,
    pub head_commit_id: String,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecordResponse {
    pub commit_id: String,
    pub repo: String,
    #[serde(default)]
    pub tree_digest: String,
    #[serde(default)]
    pub manifest: Vec<ManifestEntry>,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CreateCommitResponse {
    commit_id: String,
    #[allow(dead_code)]
    tree_digest: String,
}

pub struct ForgeRemoteClient {
    remote_url: String,
    token: Option<String>,
    /// Short timeout for branch/manifest/commit metadata.
    http: reqwest::Client,
    /// Long timeout for blob upload/download transfers.
    blob_http: reqwest::Client,
}

impl ForgeRemoteClient {
    pub fn new(remote_url: Option<&str>, token: Option<&str>) -> Self {
        let base_url = remote_url
            .map(|s| s.trim_end_matches('/').to_string())
            .or_else(default_forge_url);

        let auth_token = token
            .map(|s| s.to_string())
            .or_else(|| std::env::var("AIVCS_TOKEN").ok())
            .or_else(|| {
                let token_path = dirs_or_home_token();
                fs::read_to_string(&token_path)
                    .ok()
                    .map(|s| s.trim().to_string())
            });

        Self {
            remote_url: base_url.unwrap_or_else(default_forge_url_or_empty),
            token: auth_token,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(forge_metadata_timeout())
                .pool_max_idle_per_host(32)
                .build()
                .unwrap_or_default(),
            blob_http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(forge_blob_timeout())
                .pool_max_idle_per_host(32)
                .build()
                .unwrap_or_default(),
        }
    }

    fn forge_url(&self) -> Result<&str> {
        if self.remote_url.is_empty() {
            return Err(anyhow!(
                "no forge URL configured — run `aivcs login` or set AIVCS_FORGE_URL"
            ));
        }
        Ok(&self.remote_url)
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(ref t) = self.token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    fn raw_auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        if let Some(ref t) = self.token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    /// Walk a directory tree and compute the deterministic CAS manifest
    pub fn walk_manifest(&self, root: &Path) -> Result<(Vec<ManifestEntry>, String, usize)> {
        let mut entries = Vec::new();
        let mut total_bytes = 0;

        fn visit_dir(
            root: &Path,
            current: &Path,
            entries: &mut Vec<ManifestEntry>,
            total_bytes: &mut usize,
        ) -> Result<()> {
            for entry in fs::read_dir(current)? {
                let entry = entry?;
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                if file_name == ".aivcs"
                    || file_name == ".git"
                    || file_name == ".github"
                    || file_name == "target"
                    || file_name == "result"
                    || file_name == "node_modules"
                    || file_name == "build"
                    || file_name == "dist"
                    || file_name == ".cache"
                {
                    continue;
                }

                if path.is_dir() {
                    visit_dir(root, &path, entries, total_bytes)?;
                } else if path.is_file() {
                    let rel_path = path
                        .strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let content = fs::read(&path)?;
                    let digest = hex::encode(Sha256::digest(&content));
                    let is_exec = {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let meta = entry.metadata()?;
                            (meta.permissions().mode() & 0o111) != 0
                        }
                        #[cfg(not(unix))]
                        {
                            false
                        }
                    };

                    *total_bytes += content.len();
                    entries.push(ManifestEntry {
                        path: rel_path,
                        digest,
                        executable: is_exec,
                        size: content.len(),
                    });
                }
            }
            Ok(())
        }

        visit_dir(root, root, &mut entries, &mut total_bytes)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let digest = tree_digest_from_manifest(&entries);
        Ok((entries, digest, total_bytes))
    }

    /// Publish a tree to the AIVCS forge with parallel blob uploads.
    /// Fails closed: any blob, commit, or branch step error aborts the publish.
    pub async fn publish(
        &self,
        tree_path: &Path,
        repo: &str,
        message: &str,
        author: &str,
        branch: &str,
        private: Option<bool>,
    ) -> Result<String> {
        let (manifest, tree_digest, total_bytes) = self.walk_manifest(tree_path)?;
        info!(
            "Walked {} files ({:.2} MB), tree_digest: {}",
            manifest.len(),
            total_bytes as f64 / 1024.0 / 1024.0,
            tree_digest
        );

        if manifest.is_empty() {
            return Err(anyhow!("refusing to publish empty tree"));
        }

        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);

        // 1. Ensure repository exists
        let repo_url = format!("{forge}/api/v1/repos");
        let repo_create_resp = self
            .http
            .post(&repo_url)
            .headers(self.auth_headers())
            .json(&serde_json::json!({
                "repo": repo,
                "source_url": format!("aivcs://{}", repo),
                "private": private,
            }))
            .send()
            .await
            .context("repo create request failed")?;

        if !repo_create_resp.status().is_success() && repo_create_resp.status().as_u16() != 409 {
            let status = repo_create_resp.status();
            let body = repo_create_resp.text().await.unwrap_or_default();
            return Err(anyhow!("repo create failed: HTTP {status}: {body}"));
        }

        // 2. Incremental upload: skip parent digests only when blob is present in CAS
        let parent_head = self.resolve_branch(repo, branch).await.ok();
        let parent_digests = if let Some(ref head) = parent_head {
            self.parent_digests_present_in_cas(repo, head).await?
        } else {
            HashSet::new()
        };
        let to_upload = blobs_to_upload(&manifest, &parent_digests);
        info!(
            "Uploading {} unique blobs ({} skipped vs parent)",
            to_upload.len(),
            manifest.len().saturating_sub(to_upload.len())
        );
        self.upload_blobs(tree_path, &to_upload, &enc_repo).await?;

        // 3. Post commit — server computes commit_id; link parent when branch existed
        let parents: Vec<String> = parent_head.into_iter().collect();
        let commit_id = self
            .post_commit(repo, &manifest, message, author, &parents, None)
            .await
            .context("commit POST failed")?;

        self.update_branch(&enc_repo, branch, &commit_id)
            .await
            .context("branch PUT failed")?;

        Ok(commit_id)
    }

    async fn upload_blobs(
        &self,
        tree_path: &Path,
        manifest: &[ManifestEntry],
        enc_repo: &str,
    ) -> Result<()> {
        if manifest.is_empty() {
            return Ok(());
        }
        let forge = self.forge_url()?.to_string();
        let sem = Arc::new(Semaphore::new(forge_upload_concurrency()));
        let mut join_set = JoinSet::new();

        for entry in manifest {
            let file_path = tree_path.join(&entry.path);
            let data =
                fs::read(&file_path).with_context(|| format!("read local file {}", entry.path))?;
            let permit = sem.clone().acquire_owned().await.unwrap();
            let http = self.blob_http.clone();
            let remote_url = forge.clone();
            let enc_repo = enc_repo.to_string();
            let raw_headers = self.raw_auth_headers();
            let path_str = entry.path.clone();
            let digest = entry.digest.clone();

            join_set.spawn(async move {
                let _permit = permit;
                if skip_existing_blobs()
                    && blob_exists(&http, &remote_url, &raw_headers, &digest, &enc_repo).await?
                {
                    return Ok(());
                }
                let blob_url = format!("{remote_url}/api/v1/blobs?repo={enc_repo}");
                post_blob_with_retry(&http, &blob_url, &raw_headers, data, &path_str).await
            });
        }

        while let Some(res) = join_set.join_next().await {
            res.context("blob upload task join error")??;
        }
        Ok(())
    }

    /// Parent manifest digests that are actually present in CAS (not phantom).
    async fn parent_digests_present_in_cas(
        &self,
        repo: &str,
        parent_head: &str,
    ) -> Result<HashSet<String>> {
        let manifest = self.load_manifest(repo, parent_head).await?;
        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let raw_headers = self.raw_auth_headers();
        let mut present = HashSet::new();
        for entry in dedupe_blobs_by_digest(&manifest) {
            if blob_exists(
                &self.http,
                forge,
                &raw_headers,
                &entry.digest,
                &enc_repo,
            )
            .await
            .unwrap_or(false)
            {
                present.insert(entry.digest);
            }
        }
        Ok(present)
    }

    async fn commit_exists(&self, repo: &str, commit_id: &str) -> Result<bool> {
        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let url = format!("{forge}/api/v1/commits/{commit_id}?repo={enc_repo}");
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .context("commit existence probe")?;
        Ok(resp.status().is_success())
    }

    async fn post_commit(
        &self,
        repo: &str,
        manifest: &[ManifestEntry],
        message: &str,
        author: &str,
        parents: &[String],
        branch: Option<&str>,
    ) -> Result<String> {
        let forge = self.forge_url()?;
        let commit_url = format!("{forge}/api/v1/commits");
        let mut commit_payload = serde_json::json!({
            "repo": repo,
            "manifest": manifest,
            "parents": parents,
            "message": message,
            "author": author,
        });
        if let Some(b) = branch {
            commit_payload["branch"] = serde_json::json!(b);
        }
        let expected_id = expected_commit_id(repo, manifest, message, author, parents);
        let commit_timeout = Duration::from_secs(
            std::env::var("AIVCS_FORGE_COMMIT_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(600),
        );
        let retries = forge_upload_retries();

        for attempt in 1..=retries {
            let resp = match self
                .http
                .post(&commit_url)
                .headers(self.auth_headers())
                .timeout(commit_timeout)
                .json(&commit_payload)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if self
                        .commit_exists(repo, &expected_id)
                        .await
                        .unwrap_or(false)
                    {
                        info!(
                            "commit POST errored but commit {} exists — continuing",
                            expected_id
                        );
                        return Ok(expected_id);
                    }
                    if attempt < retries && e.is_timeout() {
                        sleep(Duration::from_millis(500 * 2u64.pow(attempt - 1))).await;
                        continue;
                    }
                    return Err(e).context("commit POST network error");
                }
            };

            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status.is_success() {
                let parsed: CreateCommitResponse = serde_json::from_str(&body)
                    .with_context(|| format!("parse commit response: {body}"))?;
                return Ok(parsed.commit_id);
            }
            if matches!(status.as_u16(), 429 | 502 | 503 | 504) && attempt < retries {
                if self
                    .commit_exists(repo, &expected_id)
                    .await
                    .unwrap_or(false)
                {
                    info!(
                        "commit POST HTTP {} but commit {} exists — continuing",
                        status, expected_id
                    );
                    return Ok(expected_id);
                }
                sleep(Duration::from_millis(500 * 2u64.pow(attempt - 1))).await;
                continue;
            }
            if self
                .commit_exists(repo, &expected_id)
                .await
                .unwrap_or(false)
            {
                info!(
                    "commit POST HTTP {} but commit {} exists — continuing",
                    status, expected_id
                );
                return Ok(expected_id);
            }
            return Err(anyhow!("commit POST failed: HTTP {status}: {body}"));
        }

        if self
            .commit_exists(repo, &expected_id)
            .await
            .unwrap_or(false)
        {
            return Ok(expected_id);
        }
        Err(anyhow!("commit POST failed after {retries} attempts"))
    }

    /// Update a branch head to point to a new commit.
    async fn update_branch(&self, enc_repo: &str, branch: &str, commit_id: &str) -> Result<()> {
        let forge = self.forge_url()?;
        let branch_url = format!("{forge}/api/v1/repos/{enc_repo}/branches/{branch}");
        let branch_payload = serde_json::json!({ "commit_id": commit_id });

        let resp = self
            .http
            .put(&branch_url)
            .headers(self.auth_headers())
            .json(&branch_payload)
            .send()
            .await
            .context("branch PUT network error")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("branch PUT failed: HTTP {status}: {body}"));
        }
        Ok(())
    }

    /// Load manifest for a commit.
    ///
    /// Prefer `GET …/manifest` first — commit metadata is heavier and often flakes
    /// (HTTP 500) while the manifest endpoint succeeds on the same forge.
    pub async fn load_manifest(&self, repo: &str, commit_id: &str) -> Result<Vec<ManifestEntry>> {
        if let Ok(entries) = self.fetch_manifest_direct(repo, commit_id).await {
            if !entries.is_empty() {
                return Ok(entries);
            }
        }

        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let commit_url = format!("{forge}/api/v1/commits/{commit_id}?repo={enc_repo}");
        let resp = self
            .http
            .get(&commit_url)
            .headers(self.auth_headers())
            .send()
            .await
            .context("fetch commit metadata")?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "commit '{}' not found (HTTP {})",
                commit_id,
                resp.status()
            ));
        }

        let commit_rec: CommitRecordResponse = resp.json().await?;
        if !commit_rec.manifest.is_empty() {
            return Ok(commit_rec.manifest);
        }

        self.fetch_manifest_direct(repo, commit_id)
            .await
            .context("fetch commit manifest")
    }

    async fn fetch_manifest_direct(
        &self,
        repo: &str,
        commit_id: &str,
    ) -> Result<Vec<ManifestEntry>> {
        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let manifest_url = format!("{forge}/api/v1/commits/{commit_id}/manifest?repo={enc_repo}");
        let m_resp = self
            .http
            .get(&manifest_url)
            .headers(self.auth_headers())
            .send()
            .await
            .context("fetch commit manifest")?;

        if !m_resp.status().is_success() {
            return Err(anyhow!(
                "manifest for commit '{}' not found (HTTP {})",
                commit_id,
                m_resp.status()
            ));
        }

        m_resp.json().await.context("parse manifest response")
    }

    /// Resolve a branch to its head commit ID
    pub async fn resolve_branch(&self, repo: &str, branch: &str) -> Result<String> {
        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let url = format!("{forge}/api/v1/repos/{enc_repo}/branches/{branch}");

        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .context("Failed to query branch from remote forge")?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "Branch '{}' not found for repository '{}' (HTTP {})",
                branch,
                repo,
                resp.status()
            ));
        }

        let resolution: BranchResolution = resp
            .json()
            .await
            .context("Failed to parse branch resolution response")?;
        Ok(resolution.head_commit_id)
    }

    /// Fetch a commit manifest and materialize files into target directory in parallel.
    /// Errors if any blob is missing (fail-closed).
    pub async fn fetch(&self, repo: &str, git_ref: &str, output_dir: &Path) -> Result<()> {
        let commit_id = if git_ref.len() == 64 && git_ref.chars().all(|c| c.is_ascii_hexdigit()) {
            git_ref.to_string()
        } else {
            self.resolve_branch(repo, git_ref).await?
        };

        info!("Fetching commit {} for repo {}", commit_id, repo);
        let manifest = self.load_manifest(repo, &commit_id).await?;
        if manifest.is_empty() {
            return Err(anyhow!(
                "commit '{}' has empty manifest — refusing to materialize phantom repo",
                commit_id
            ));
        }

        fs::create_dir_all(output_dir)?;
        let forge = self.forge_url()?;
        let enc_repo = urlencoding::encode(repo);
        let output_root = fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());

        // Download each unique digest once, write to all paths (agent-scale fetch).
        let mut by_digest: HashMap<String, Vec<(PathBuf, bool)>> = HashMap::new();
        for entry in manifest {
            ensure_safe_repo_path(&entry.path)?;
            let dest = output_root.join(&entry.path);
            if !dest.starts_with(&output_root) {
                return Err(anyhow!(
                    "manifest path escapes output directory: {}",
                    entry.path
                ));
            }
            by_digest
                .entry(entry.digest.clone())
                .or_default()
                .push((dest, entry.executable));
        }

        let sem = Arc::new(Semaphore::new(forge_download_concurrency()));
        let mut join_set = JoinSet::new();
        let unique_blobs = by_digest.len();

        for (digest, paths) in by_digest {
            let permit = sem.clone().acquire_owned().await.unwrap();
            let http = self.blob_http.clone();
            let remote_url = forge.to_string();
            let enc = enc_repo.to_string();
            let raw_headers = self.raw_auth_headers();

            join_set.spawn(async move {
                let _permit = permit;
                let bytes = download_blob(&http, &remote_url, &raw_headers, &digest, &enc).await?;
                Ok::<_, anyhow::Error>(((), bytes, paths))
            });
        }

        let mut paths_written = 0usize;
        while let Some(res) = join_set.join_next().await {
            let (_, bytes, paths) = res.context("blob download task join error")??;
            for (dest, is_exec) in paths {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dest, &bytes)
                    .with_context(|| format!("write file '{}'", dest.display()))?;
                #[cfg(unix)]
                if is_exec {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
                }
                paths_written += 1;
            }
        }

        if paths_written == 0 {
            return Err(anyhow!(
                "materialized zero files for commit '{}' — forge returned a non-empty manifest but no blobs were written",
                commit_id
            ));
        }

        info!(
            "materialized {} files ({} unique blobs) into {}",
            paths_written,
            unique_blobs,
            output_root.display()
        );
        Ok(())
    }

    /// Clone a repository from `aivcs://org/repo[@branch]` or bare slug into a directory.
    pub async fn clone(
        &self,
        url_or_slug: &str,
        target_dir: Option<&Path>,
        branch: &str,
    ) -> Result<PathBuf> {
        let (repo_slug, resolved_branch) = parse_clone_target(url_or_slug, branch)?;
        let default_dir_name = repo_slug
            .split('/')
            .nth(1)
            .unwrap_or("repo");
        let dest = target_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(default_dir_name));

        self.fetch(&repo_slug, &resolved_branch, &dest).await?;
        Ok(dest)
    }
}

/// Parse `aivcs://org/repo`, `org/repo`, or `org/repo@branch` into slug + branch.
pub fn parse_clone_target(url_or_slug: &str, branch_override: &str) -> Result<(String, String)> {
    let mut slug = url_or_slug.trim();
    for prefix in [
        "aivcs://",
        "https://aivcs.io/",
        "https://www.aivcs.io/",
        "https://future.aivcs.io/",
        "https://aivcsd.aivcs.io/",
    ] {
        if let Some(rest) = slug.strip_prefix(prefix) {
            slug = rest;
            break;
        }
    }
    slug = slug.trim_end_matches('/').trim_end_matches(".git");

    let (repo_part, branch_from_url) = if let Some(at) = slug.rfind('@') {
        let (repo, branch) = slug.split_at(at);
        if repo.is_empty() || branch.len() <= 1 {
            return Err(anyhow!(
                "invalid repository URI or slug (expected org/repo): {url_or_slug}"
            ));
        }
        (repo, Some(&branch[1..]))
    } else {
        (slug, None)
    };

    let parts: Vec<&str> = repo_part.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Err(anyhow!(
            "invalid repository URI or slug (expected org/repo): {url_or_slug}"
        ));
    }
    let repo_slug = format!("{}/{}", parts[0], parts[1]);
    let branch = branch_from_url
        .filter(|b| !b.is_empty())
        .unwrap_or(branch_override)
        .to_string();
    Ok((repo_slug, branch))
}

fn default_forge_url_or_empty() -> String {
    default_forge_url().unwrap_or_default()
}

fn ensure_safe_repo_path(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(anyhow!("unsafe manifest path: {path:?}"));
    }
    for component in Path::new(path).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow!("path traversal in manifest: {path}"));
        }
    }
    Ok(())
}

async fn download_blob(
    http: &reqwest::Client,
    remote_url: &str,
    headers: &HeaderMap,
    digest: &str,
    enc_repo: &str,
) -> Result<Vec<u8>> {
    let url = format!("{remote_url}/api/v1/blobs/{digest}?repo={enc_repo}");
    let retries = forge_download_retries();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let resp = http
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .with_context(|| format!("blob GET network error for digest {digest}"))?;
        let status = resp.status();
        if status.is_success() {
            return resp
                .bytes()
                .await
                .map(|b| b.to_vec())
                .with_context(|| format!("read blob bytes for digest {digest}"));
        }
        let retryable = matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504);
        if retryable && attempt <= retries {
            sleep(Duration::from_millis(250 * 2u64.pow(attempt - 1))).await;
            continue;
        }
        return Err(anyhow!(
            "blob missing: digest {} HTTP {}",
            digest,
            status
        ));
    }
}

/// Deterministic tree digest shared with forge-cas flat manifest mode.
pub fn tree_digest_from_manifest(manifest: &[ManifestEntry]) -> String {
    let mut lines = String::new();
    for entry in manifest {
        lines.push_str(&format!("{}:{}\n", entry.digest, entry.path));
    }
    hex::encode(Sha256::digest(lines.as_bytes()))
}

/// Tree digest as computed by forge-cas `create_commit` (sorted JSON tuples).
pub fn server_tree_digest(manifest: &[ManifestEntry]) -> String {
    let mut sorted: Vec<&ManifestEntry> = manifest.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let canonical: Vec<(&str, &str, bool, u64)> = sorted
        .iter()
        .map(|e| {
            (
                e.path.as_str(),
                e.digest.as_str(),
                e.executable,
                e.size as u64,
            )
        })
        .collect();
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical).unwrap_or_default(),
    ))
}

/// Commit id forge-cas returns for a publish payload (for timeout recovery).
pub fn expected_commit_id(
    repo: &str,
    manifest: &[ManifestEntry],
    message: &str,
    author: &str,
    parents: &[String],
) -> String {
    let tree = server_tree_digest(manifest);
    let identity = serde_json::json!({
        "repo": repo,
        "tree_digest": tree,
        "manifest": manifest,
        "parents": parents,
        "message": message,
        "author": author,
    });
    hex::encode(Sha256::digest(
        serde_json::to_vec(&identity).unwrap_or_default(),
    ))
}

/// One representative entry per content digest (first path wins, sorted input assumed).
pub fn dedupe_blobs_by_digest(manifest: &[ManifestEntry]) -> Vec<ManifestEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in manifest {
        if seen.insert(entry.digest.clone()) {
            out.push(entry.clone());
        }
    }
    out
}

/// Blobs that must be uploaded: unique digests not present in the parent commit.
pub fn blobs_to_upload(
    manifest: &[ManifestEntry],
    parent_digests: &HashSet<String>,
) -> Vec<ManifestEntry> {
    dedupe_blobs_by_digest(manifest)
        .into_iter()
        .filter(|e| !parent_digests.contains(&e.digest))
        .collect()
}

fn forge_metadata_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(
        std::env::var("AIVCS_FORGE_METADATA_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(45),
    )
}

fn forge_blob_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(
        std::env::var("AIVCS_FORGE_BLOB_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(180),
    )
}

fn forge_upload_concurrency() -> usize {
    std::env::var("AIVCS_FORGE_UPLOAD_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(32)
}

fn forge_download_retries() -> u32 {
    std::env::var("AIVCS_FORGE_DOWNLOAD_RETRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
}

fn forge_download_concurrency() -> usize {
    std::env::var("AIVCS_FORGE_DOWNLOAD_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(32)
}

fn skip_existing_blobs() -> bool {
    std::env::var("AIVCS_FORGE_SKIP_EXISTING_BLOBS")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(true)
}

fn forge_upload_retries() -> u32 {
    std::env::var("AIVCS_FORGE_UPLOAD_RETRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
}

async fn blob_exists(
    http: &reqwest::Client,
    remote_url: &str,
    headers: &HeaderMap,
    digest: &str,
    enc_repo: &str,
) -> Result<bool> {
    let url = format!("{remote_url}/api/v1/blobs/{digest}?repo={enc_repo}");
    let resp = http
        .get(&url)
        .headers(headers.clone())
        .send()
        .await
        .context("blob existence probe")?;
    Ok(resp.status().is_success())
}

async fn post_blob_with_retry(
    http: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    data: Vec<u8>,
    path_str: &str,
) -> Result<()> {
    let retries = forge_upload_retries();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let resp = http
            .post(url)
            .headers(headers.clone())
            .body(data.clone())
            .send()
            .await
            .with_context(|| format!("blob upload network error for '{path_str}'"))?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }
        let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
        let body = resp.text().await.unwrap_or_default();
        if retryable && attempt <= retries {
            sleep(Duration::from_millis(250 * 2u64.pow(attempt - 1))).await;
            continue;
        }
        return Err(anyhow!(
            "blob upload for '{path_str}' failed: HTTP {status}: {body}"
        ));
    }
}

fn default_forge_url() -> Option<String> {
    crate::forge_login::resolve_forge_url_from_config().or_else(|| {
        if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() {
            Some(IN_CLUSTER_FORGE_URL.to_string())
        } else {
            None
        }
    })
}

fn dirs_or_home_token() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".aivcs").join("token")
    } else {
        PathBuf::from("/tmp/aivcs-token")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parse_clone_target_accepts_aivcs_uri_and_branch() {
        let (repo, branch) =
            parse_clone_target("aivcs://aivcs/data-mesh@develop", "main").unwrap();
        assert_eq!(repo, "aivcs/data-mesh");
        assert_eq!(branch, "develop");

        let (repo, branch) = parse_clone_target("aivcs/data-mesh", "main").unwrap();
        assert_eq!(repo, "aivcs/data-mesh");
        assert_eq!(branch, "main");

        let (repo, branch) =
            parse_clone_target("https://aivcsd.aivcs.io/aivcs/aivcs.git", "main").unwrap();
        assert_eq!(repo, "aivcs/aivcs");
        assert_eq!(branch, "main");
    }

    #[test]
    fn tree_digest_is_deterministic() {
        let entries = vec![
            ManifestEntry {
                path: "a.txt".into(),
                digest: "aa".into(),
                executable: false,
                size: 1,
            },
            ManifestEntry {
                path: "b.txt".into(),
                digest: "bb".into(),
                executable: false,
                size: 2,
            },
        ];
        let d1 = tree_digest_from_manifest(&entries);
        let d2 = tree_digest_from_manifest(&entries);
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);
    }

    #[tokio::test]
    async fn load_manifest_falls_back_to_manifest_endpoint() {
        let server = MockServer::start().await;
        let commit_id = "c".repeat(64);
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{commit_id}")))
            .and(query_param("repo", "org/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "commit_id": commit_id,
                "repo": "org/repo",
                "tree_digest": "abc",
                "parents": [],
                "message": "m",
                "author": "a"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{commit_id}/manifest")))
            .and(query_param("repo", "org/repo"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "path": "f.txt",
                    "digest": "d".repeat(64),
                    "executable": false,
                    "size": 3
                }])),
            )
            .mount(&server)
            .await;

        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        let manifest = client.load_manifest("org/repo", &commit_id).await.unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].path, "f.txt");
    }

    #[test]
    fn dedupe_blobs_by_digest_collapses_identical_content() {
        let manifest = vec![
            ManifestEntry {
                path: "a/x".into(),
                digest: "d1".repeat(32),
                executable: false,
                size: 1,
            },
            ManifestEntry {
                path: "b/y".into(),
                digest: "d1".repeat(32),
                executable: false,
                size: 1,
            },
        ];
        assert_eq!(dedupe_blobs_by_digest(&manifest).len(), 1);
    }

    #[test]
    fn blobs_to_upload_skips_parent_digests() {
        let parent: HashSet<String> = ["keep".to_string()].into_iter().collect();
        let manifest = vec![
            ManifestEntry {
                path: "old.txt".into(),
                digest: "keep".into(),
                executable: false,
                size: 1,
            },
            ManifestEntry {
                path: "new.txt".into(),
                digest: "fresh".into(),
                executable: false,
                size: 2,
            },
        ];
        let up = blobs_to_upload(&manifest, &parent);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].digest, "fresh");
    }

    #[tokio::test]
    async fn publish_aborts_when_blob_upload_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/repos"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/org%2Frepo/branches/main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/blobs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/blobs"))
            .respond_with(ResponseTemplate::new(500).set_body_string("storage full"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("one.txt"), b"x").unwrap();

        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        let err = client
            .publish(tmp.path(), "org/repo", "m", "a", "main", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("blob upload"));
    }

    #[tokio::test]
    async fn publish_uses_atomic_commit_with_branch() {
        let server = MockServer::start().await;
        let server_commit = "s".repeat(64);

        Mock::given(method("POST"))
            .and(path("/api/v1/repos"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/org%2Frepo/branches/main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/blobs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/blobs"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(serde_json::json!({"digest": "abc"})),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/commits"))
            .respond_with({
                let expected = server_commit.clone();
                move |_req: &wiremock::Request| {
                    ResponseTemplate::new(201).set_body_json(serde_json::json!({
                        "commit_id": expected,
                        "tree_digest": "t"
                    }))
                }
            })
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/api/v1/repos/org%2Frepo/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "repo": "org/repo",
                "branch": "main",
                "head_commit_id": server_commit
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("one.txt"), b"x").unwrap();

        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        let got = client
            .publish(tmp.path(), "org/repo", "m", "a", "main", None)
            .await
            .unwrap();
        assert_eq!(got, server_commit);
    }

    #[tokio::test]
    async fn fetch_materializes_files_from_manifest() {
        let server = MockServer::start().await;
        let commit_id = "c".repeat(64);
        let digest = "d".repeat(64);
        let content = b"hello forge";

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/repos/org%2Frepo/branches/main")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "repo": "org/repo",
                "branch": "main",
                "head_commit_id": commit_id
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{commit_id}/manifest")))
            .and(query_param("repo", "org/repo"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "path": "README.md",
                    "digest": digest,
                    "executable": false,
                    "size": content.len()
                }])),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/blobs/{digest}")))
            .and(query_param("repo", "org/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.to_vec()))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        client.fetch("org/repo", "main", tmp.path()).await.unwrap();
        let written = fs::read_to_string(tmp.path().join("README.md")).unwrap();
        assert_eq!(written, "hello forge");
    }

    #[tokio::test]
    async fn fetch_errors_when_blob_missing() {
        let server = MockServer::start().await;
        let commit_id = "c".repeat(64);
        let digest = "d".repeat(64);

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/repos/org%2Frepo/branches/main")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "repo": "org/repo",
                "branch": "main",
                "head_commit_id": commit_id
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{commit_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "commit_id": commit_id,
                "repo": "org/repo"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{commit_id}/manifest")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "path": "missing.txt",
                    "digest": digest,
                    "executable": false,
                    "size": 1
                }])),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/blobs/{digest}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        let err = client
            .fetch("org/repo", "main", tmp.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("blob missing"));
    }

    #[tokio::test]
    async fn incremental_publish_skips_parent_blobs() {
        let server = MockServer::start().await;
        let parent_commit = "p".repeat(64);
        let old_content = b"old";
        let new_content = b"new";
        let kept_digest = hex::encode(Sha256::digest(old_content));
        let new_digest = hex::encode(Sha256::digest(new_content));

        Mock::given(method("POST"))
            .and(path("/api/v1/repos"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/org%2Frepo/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "repo": "org/repo",
                "branch": "main",
                "head_commit_id": parent_commit
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{parent_commit}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "commit_id": parent_commit,
                "repo": "org/repo"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/commits/{parent_commit}/manifest")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "path": "old.txt",
                    "digest": kept_digest,
                    "executable": false,
                    "size": old_content.len()
                }])),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/blobs/{kept_digest}")))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/api/v1/blobs/{new_digest}")))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/blobs"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(serde_json::json!({"digest": new_digest})),
            )
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "commit_id": "c".repeat(64),
                "tree_digest": "t"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/api/v1/repos/org%2Frepo/branches/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "repo": "org/repo",
                "branch": "main",
                "head_commit_id": "c".repeat(64)
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("old.txt"), old_content).unwrap();
        std::fs::write(tmp.path().join("new.txt"), new_content).unwrap();

        let client = ForgeRemoteClient::new(Some(&server.uri()), None);
        client
            .publish(tmp.path(), "org/repo", "m", "a", "main", None)
            .await
            .unwrap();
    }
}
