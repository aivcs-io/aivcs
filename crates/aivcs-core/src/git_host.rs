//! Git forge detection — GitHub today, GitLab for sovereign / post-GitHub CI.
//!
//! `owner/repo` CLI flags map to GitHub repos and to GitLab project paths
//! (`group/subgroup/project` encoded as `owner/repo` when nested groups are flat).

use std::process::Command;

/// Supported git forges for `aivcs pr pipeline` and A2A payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHost {
    GitHub,
    GitLab,
}

impl GitHost {
    pub fn detect() -> Self {
        std::env::var("AIVCS_GIT_HOST")
            .ok()
            .and_then(|value| match value.trim().to_lowercase().as_str() {
                "gitlab" | "gl" => Some(Self::GitLab),
                "github" | "gh" => Some(Self::GitHub),
                _ => None,
            })
            .unwrap_or_else(|| {
                if std::env::var("GITLAB_TOKEN").is_ok()
                    || std::env::var("GITLAB_TOKEN_FILE").is_ok()
                    || std::env::var("CI").is_ok() && std::env::var("CI_PROJECT_PATH").is_ok()
                {
                    Self::GitLab
                } else {
                    Self::GitHub
                }
            })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
        }
    }
}

/// Parse `owner/repo` (or nested GitLab path) from a remote URL.
pub fn parse_git_remote(remote: &str) -> Option<(GitHost, String)> {
    if let Some(path) = crate::git::parse_github_remote(remote) {
        return Some((GitHost::GitHub, path));
    }
    parse_gitlab_remote(remote)
}

fn parse_gitlab_remote(remote: &str) -> Option<(GitHost, String)> {
    let candidate = remote
        .strip_prefix("git@gitlab.com:")
        .or_else(|| remote.strip_prefix("https://gitlab.com/"))
        .or_else(|| remote.strip_prefix("ssh://git@gitlab.com/"))?;

    let mut canonical = candidate;
    while let Some(stripped) = canonical.strip_suffix(".git") {
        canonical = stripped;
    }

    is_forge_project_path(canonical).then(|| (GitHost::GitLab, canonical.to_string()))
}

/// Resolve `owner/repo` (or GitLab project path) for forge event payloads.
pub fn detect_forge_repository() -> Option<String> {
    if let Ok(path) = std::env::var("CI_PROJECT_PATH") {
        let trimmed = path.trim().trim_matches('/');
        if is_forge_project_path(trimmed) {
            return Some(trimmed.to_string());
        }
    }

    std::env::var("GITHUB_REPOSITORY")
        .ok()
        .filter(|value| is_forge_project_path(value))
        .or_else(|| {
            std::env::var("GITLAB_PROJECT_PATH")
                .ok()
                .filter(|value| is_forge_project_path(value))
        })
        .or_else(detect_forge_repository_from_origin)
}

fn detect_forge_repository_from_origin() -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let remote = String::from_utf8(output.stdout).ok()?;
    parse_git_remote(remote.trim()).map(|(_, path)| path)
}

/// True for `owner/repo` or nested GitLab paths (`a/b/c/d`).
pub fn is_forge_project_path(value: &str) -> bool {
    let value = value.trim().trim_matches('/');
    if value.is_empty() || value.len() > 200 {
        return false;
    }
    let segments: Vec<&str> = value.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return false;
    }
    segments
        .iter()
        .all(|segment| is_valid_forge_segment(segment))
}

fn is_valid_forge_segment(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 100 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gitlab_remote_accepts_https_and_ssh() {
        assert_eq!(
            parse_gitlab_remote("https://gitlab.com/lornu-ai/infra-code.git"),
            Some((GitHost::GitLab, "lornu-ai/infra-code".to_string()))
        );
        assert_eq!(
            parse_gitlab_remote("git@gitlab.com:lornu-ai/nested/infra-code.git"),
            Some((GitHost::GitLab, "lornu-ai/nested/infra-code".to_string()))
        );
    }

    #[test]
    fn nested_gitlab_path_is_valid() {
        assert!(is_forge_project_path("lornu-ai/nested/infra-code"));
        assert!(!is_forge_project_path("single-segment"));
    }
}
