//! GitHub integration for autonomous version control operations.
//!
//! Provides high-level APIs for branch creation, file commits, and Pull Request
//! management, including automated reviewer assignment (Librarian Agent).

use anyhow::{Context, Result};
use octocrab::Octocrab;
use tracing::info;

/// Client for performing GitHub operations.
pub struct GitHubClient {
    octocrab: Octocrab,
    owner: String,
    repo: String,
}

impl GitHubClient {
    /// Create a new GitHub client using a bearer token (PAT or GitHub App installation token).
    pub fn new(token: String, owner: String, repo: String) -> Result<Self> {
        let octocrab = Octocrab::builder()
            .personal_token(token)
            .build()
            .context("failed to initialize GitHub client")?;

        Ok(Self {
            octocrab,
            owner,
            repo,
        })
    }

    /// Create a new branch from a base reference (usually "main").
    pub async fn create_branch(&self, branch_name: &str, base: &str) -> Result<String> {
        info!("Creating branch '{}' from '{}'", branch_name, base);

        // Get base branch SHA
        let base_ref = self
            .octocrab
            .repos(&self.owner, &self.repo)
            .get_ref(&octocrab::params::repos::Reference::Branch(
                base.to_string(),
            ))
            .await
            .context(format!("failed to get base ref '{}'", base))?;

        let sha = match base_ref.object {
            octocrab::models::repos::Object::Commit { sha, .. } => sha,
            octocrab::models::repos::Object::Tag { sha, .. } => sha,
            _ => return Err(anyhow::anyhow!("unsupported base ref type")),
        };

        // Create new ref
        self.octocrab
            .repos(&self.owner, &self.repo)
            .create_ref(
                &octocrab::params::repos::Reference::Branch(branch_name.to_string()),
                &sha,
            )
            .await
            .context(format!("failed to create branch '{}'", branch_name))?;

        Ok(sha)
    }

    /// Commit a single file to a specific branch. Returns the resulting commit SHA.
    ///
    /// Supports both text and binary files by base64-encoding the content for
    /// the GitHub Contents API.
    pub async fn commit_file(
        &self,
        branch: &str,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<String> {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;

        info!("Committing file '{}' to branch '{}'", path, branch);

        // We use a manual PUT request to the Contents API to support binary data
        // and ensure we don't have UTF-8 overhead or double-encoding issues.
        let url = format!(
            "/repos/{}/{}/contents/{}",
            self.owner,
            self.repo,
            encode_contents_path(path)
        );

        let mut body = serde_json::json!({
            "message": message,
            "content": STANDARD.encode(content),
            "branch": branch,
        });

        // Check if file exists to get its SHA for an update
        if let Ok(content) = self
            .octocrab
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(path)
            .r#ref(branch)
            .send()
            .await
        {
            if let Some(item) = content.items.first() {
                body["sha"] = serde_json::json!(item.sha);
            }
        }

        let update: serde_json::Value = self
            .octocrab
            .put(url, Some(&body))
            .await
            .context(format!("failed to commit file '{}' via Contents API", path))?;

        let sha = update["commit"]["sha"].as_str().ok_or_else(|| {
            anyhow::anyhow!("GitHub API response missing commit SHA for '{}'", path)
        })?;

        Ok(sha.to_string())
    }

    /// Open a Pull Request and optionally request review from the Librarian Agent.
    pub async fn open_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        request_librarian: bool,
    ) -> Result<u64> {
        info!("Opening PR: '{}' ({} -> {})", title, head, base);

        let pr = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .context("failed to create pull request")?;

        let pr_number = pr.number;
        info!("PR #{} opened successfully", pr_number);

        if request_librarian {
            self.request_librarian_review(pr_number).await?;
        }

        Ok(pr_number)
    }

    /// Add a comment to an existing issue or Pull Request.
    pub async fn add_comment(&self, issue_number: u64, body: &str) -> Result<u64> {
        info!("Adding comment to #{}", issue_number);

        let comment = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(issue_number, body)
            .await
            .context(format!("failed to add comment to #{}", issue_number))?;

        Ok(comment.id.0)
    }

    /// Request a review from the Librarian Agent.
    ///
    /// The Librarian's GitHub username is read from `RELIC_LIBRARIAN_USERNAME`.
    /// This is required when called: a missing or empty env var aborts before any
    /// API call, rather than silently requesting review from a placeholder user
    /// that may not exist and failing partway through a multi-step pipeline.
    pub async fn request_librarian_review(&self, pr_number: u64) -> Result<()> {
        let librarian = resolve_librarian_username(std::env::var("RELIC_LIBRARIAN_USERNAME"))?;

        info!(
            "Requesting review from Librarian Agent ('{}') on PR #{}",
            librarian, pr_number
        );

        // octocrab 0.41 request_reviews takes (pr_number, reviewers, team_reviewers)
        self.octocrab
            .pulls(&self.owner, &self.repo)
            .request_reviews(pr_number, vec![librarian], vec![])
            .await
            .context(format!("failed to request review for PR #{}", pr_number))?;

        Ok(())
    }
}

/// Resolve a GitHub bearer token for autonomous agent Jobs.
///
/// Prefers `GITHUB_TOKEN` (typical ESO projected env var). Falls back to
/// `GITHUB_TOKEN_FILE` (Kubernetes secret volume mount path) so tokens never
/// need to be written to shell history.
pub fn resolve_github_token() -> Result<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Ok(path) = std::env::var("GITHUB_TOKEN_FILE") {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read GITHUB_TOKEN_FILE at '{path}'"))?;
        let trimmed = content.trim();
        anyhow::ensure!(
            !trimmed.is_empty(),
            "GITHUB_TOKEN_FILE at '{path}' is empty"
        );
        return Ok(trimmed.to_string());
    }

    anyhow::bail!("GITHUB_TOKEN or GITHUB_TOKEN_FILE must be set for GitHub API access")
}

/// Validate the raw `RELIC_LIBRARIAN_USERNAME` env-var lookup result.
///
/// Split out so the validation contract can be unit-tested without an HTTP
/// client. A missing or whitespace-only value is rejected eagerly: the
/// alternative is silently requesting review from a placeholder user that may
/// not exist and failing partway through a multi-step pipeline.
fn resolve_librarian_username(
    raw: std::result::Result<String, std::env::VarError>,
) -> Result<String> {
    let value = match raw {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => {
            anyhow::bail!(
                "RELIC_LIBRARIAN_USERNAME must be set to request a Librarian Agent review"
            )
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("RELIC_LIBRARIAN_USERNAME contains non-UTF-8 bytes")
        }
    };
    anyhow::ensure!(
        !value.trim().is_empty(),
        "RELIC_LIBRARIAN_USERNAME is set but empty"
    );
    Ok(value.trim().to_string())
}

fn encode_contents_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_path_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::VarError;

    #[test]
    fn resolve_librarian_username_missing_env_is_rejected() {
        let err = resolve_librarian_username(Err(VarError::NotPresent)).unwrap_err();
        assert!(
            format!("{err:#}").contains("RELIC_LIBRARIAN_USERNAME must be set"),
            "expected missing-env error, got: {err:#}"
        );
    }

    #[test]
    fn resolve_librarian_username_empty_string_is_rejected() {
        let err = resolve_librarian_username(Ok(String::new())).unwrap_err();
        assert!(
            format!("{err:#}").contains("set but empty"),
            "expected empty-value error, got: {err:#}"
        );
    }

    #[test]
    fn resolve_librarian_username_whitespace_only_is_rejected() {
        let err = resolve_librarian_username(Ok("   \t\n".to_string())).unwrap_err();
        assert!(
            format!("{err:#}").contains("set but empty"),
            "expected whitespace-only to be treated as empty, got: {err:#}"
        );
    }

    #[test]
    fn resolve_librarian_username_valid_value_is_returned_verbatim() {
        let username = resolve_librarian_username(Ok("librarian-bot".to_string())).unwrap();
        assert_eq!(username, "librarian-bot");
    }

    #[test]
    fn resolve_librarian_username_trims_secret_projection_newline() {
        let username = resolve_librarian_username(Ok(" librarian-bot\n".to_string())).unwrap();
        assert_eq!(username, "librarian-bot");
    }

    #[test]
    fn resolve_librarian_username_not_unicode_is_rejected_with_distinct_message() {
        // NotUnicode means the env var IS set but contains invalid UTF-8 —
        // distinguish it from "missing" so an operator debugging an ESO
        // projection sees the real failure mode.
        let err = resolve_librarian_username(Err(VarError::NotUnicode(std::ffi::OsString::from(
            "ignored",
        ))))
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("non-UTF-8"),
            "expected non-UTF-8-specific error, got: {rendered}"
        );
        assert!(
            !rendered.contains("must be set"),
            "non-UTF-8 error must not claim the variable is unset, got: {rendered}"
        );
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: tests run serially within this module; each guard restores on drop.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    struct EnvUnsetGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvUnsetGuard {
        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvUnsetGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn resolve_github_token_scenarios() {
        // Prefer env var over file path.
        {
            let _token = EnvGuard::set("GITHUB_TOKEN", "ghp_from_env");
            let _file = EnvGuard::set("GITHUB_TOKEN_FILE", "/should/not/read");
            assert_eq!(resolve_github_token().unwrap(), "ghp_from_env");
        }

        // Fall back to file when env var is whitespace-only.
        {
            let _token = EnvGuard::set("GITHUB_TOKEN", "   ");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("token");
            std::fs::write(&path, "ghp_from_file\n").unwrap();
            let _file = EnvGuard::set("GITHUB_TOKEN_FILE", path.to_str().unwrap());
            assert_eq!(resolve_github_token().unwrap(), "ghp_from_file");
        }

        // Reject when neither source is usable.
        {
            let _token = EnvUnsetGuard::unset("GITHUB_TOKEN");
            let _file = EnvUnsetGuard::unset("GITHUB_TOKEN_FILE");
            let err = resolve_github_token().unwrap_err();
            assert!(
                format!("{err:#}").contains("GITHUB_TOKEN or GITHUB_TOKEN_FILE"),
                "expected missing-token error, got: {err:#}"
            );
        }
    }

    #[test]
    fn test_base64_encoding_for_binary_files() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        // PNG header
        let data = b"\x89PNG\r\n\x1a\n";
        let encoded = STANDARD.encode(data);
        assert_eq!(encoded, "iVBORw0KGgo=");
    }

    #[test]
    fn encode_contents_path_preserves_slashes_and_escapes_segments() {
        assert_eq!(
            encode_contents_path("docs/release notes/#1 \u{00FC}.md"),
            "docs/release%20notes/%231%20%C3%BC.md"
        );
    }
}
