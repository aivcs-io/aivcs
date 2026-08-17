//! Unified git forge client — GitHub or GitLab, selected by env.

use anyhow::Result;

use crate::git_host::GitHost;
use crate::github::{resolve_github_token, GitHubClient};
use crate::gitlab::{resolve_gitlab_token, GitLabClient};

/// Forge-agnostic change request (PR / MR).
pub enum ForgeClient {
    GitHub(GitHubClient),
    GitLab(GitLabClient),
}

impl ForgeClient {
    pub fn from_env(owner: String, repo: String) -> Result<Self> {
        match GitHost::detect() {
            GitHost::GitHub => {
                let token = resolve_github_token()?;
                Ok(Self::GitHub(GitHubClient::new(token, owner, repo)?))
            }
            GitHost::GitLab => {
                let token = resolve_gitlab_token()?;
                Ok(Self::GitLab(GitLabClient::new(token, owner, repo)?))
            }
        }
    }

    pub fn host(&self) -> GitHost {
        match self {
            Self::GitHub(_) => GitHost::GitHub,
            Self::GitLab(_) => GitHost::GitLab,
        }
    }

    pub async fn create_branch(&self, branch_name: &str, base: &str) -> Result<String> {
        match self {
            Self::GitHub(client) => client.create_branch(branch_name, base).await,
            Self::GitLab(client) => client.create_branch(branch_name, base).await,
        }
    }

    pub async fn commit_file(
        &self,
        branch: &str,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<String> {
        match self {
            Self::GitHub(client) => client.commit_file(branch, path, content, message).await,
            Self::GitLab(client) => client.commit_file(branch, path, content, message).await,
        }
    }

    pub async fn open_change_request(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        request_librarian: bool,
    ) -> Result<u64> {
        match self {
            Self::GitHub(client) => {
                client
                    .open_pr(title, body, head, base, request_librarian)
                    .await
            }
            Self::GitLab(client) => {
                // GitLab reviewer assignment is a follow-up; MR body carries aivcs snapshot.
                let _ = request_librarian;
                client.open_mr(title, body, head, base).await
            }
        }
    }
}
