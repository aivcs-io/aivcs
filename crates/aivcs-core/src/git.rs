//! Git integration utilities for capturing repository state.

use std::path::Path;
use std::process::Command;

use crate::domain::error::{AivcsError, Result};

/// Capture the HEAD commit SHA from a git repository.
///
/// Runs `git rev-parse HEAD` in the given directory. Returns an error if the
/// directory is not inside a git repository or if git is not available.
pub fn capture_head_sha(repo_dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| AivcsError::GitError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AivcsError::GitError(format!(
            "git rev-parse HEAD failed: {stderr}"
        )));
    }

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        return Err(AivcsError::GitError(
            "git rev-parse HEAD returned empty output".to_string(),
        ));
    }

    Ok(sha)
}

/// Check whether a directory is inside a git work tree.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve `owner/name` for GitHub event payloads.
///
/// Prefers `GITHUB_REPOSITORY`, then parses `git remote get-url origin`.
pub fn detect_github_repository() -> Option<String> {
    std::env::var("GITHUB_REPOSITORY")
        .ok()
        .filter(|value| is_owner_repo(value))
        .or_else(detect_github_repository_from_origin)
}

fn detect_github_repository_from_origin() -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let remote = String::from_utf8(output.stdout).ok()?;
    parse_github_remote(remote.trim())
}

/// Parse `owner/name` from common GitHub remote URL formats.
///
/// Supported shapes:
/// - `git@github.com:owner/name` (SCP-style SSH — the default git output)
/// - `aivcs://owner/name`
/// - `ssh://git@github.com/owner/name` (RFC-style SSH that some tooling emits)
///
/// Trailing `.git` suffixes are stripped repeatedly so `owner/name`,
/// `owner/name.git`, and pathological `owner/name.git.git` all collapse to
/// the same canonical identity, preventing alias-based cache poisoning.
pub fn parse_github_remote(remote: &str) -> Option<String> {
    let candidate = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("aivcs://"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;

    // Strip the `.git` suffix from the candidate (not the whole URL) so any
    // accidental double-suffix collapses idempotently. GitHub itself rejects
    // repo names ending in `.git`, so loop-strip is safe.
    let mut canonical = candidate;
    while let Some(stripped) = canonical.strip_suffix(".git") {
        canonical = stripped;
    }

    is_owner_repo(canonical).then(|| canonical.to_string())
}

/// True iff `value` is exactly two valid GitHub name segments joined by `/`.
///
/// Public so CLI `--owner`/`--repo` flag parsers and A2A `repo_override`
/// validators can apply the same rule and not silently bypass the gate.
pub fn is_owner_repo(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    !repo.contains('/') && is_valid_github_name(owner) && is_valid_github_name(repo)
}

/// True iff `value` is a syntactically valid GitHub user/org or repo name.
///
/// Rules enforced (a strict subset of what GitHub itself accepts):
/// - non-empty, max 100 bytes (covers both 39-char username and 100-char
///   repo limits with a single bound — DoS-adjacent cap)
/// - first AND last byte ASCII alphanumeric (rejects whole-segment `.`/`..`
///   path-traversal vectors, leading `-` argv-flag injection, and trailing
///   `.` Windows-aliasing)
/// - interior bytes ASCII alnum, `.`, `_`, or `-` (rejects whitespace,
///   newlines, shell metacharacters)
///
/// Public so callers downstream of `parse_github_remote` (CLI value-parsers,
/// `repo_override` validators) can enforce the same invariant by construction.
pub fn is_valid_github_name(value: &str) -> bool {
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

/// Detect the current local git branch name.
///
/// Runs `git rev-parse --abbrev-ref HEAD` in the given directory.
pub fn detect_current_branch(repo_dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| AivcsError::GitError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AivcsError::GitError(format!(
            "git rev-parse --abbrev-ref HEAD failed: {stderr}"
        )));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err(AivcsError::GitError(
            "git rev-parse --abbrev-ref HEAD returned empty output".to_string(),
        ));
    }
    // `git rev-parse --abbrev-ref HEAD` returns the literal string "HEAD" when
    // the working tree is in detached-HEAD state (common in CI checkouts of
    // tag/PR refs). Returning that to callers caused `aivcs pr-note` to look
    // up a branch literally named "HEAD" in the DB and emit a confusing
    // "Branch 'HEAD' not found" error. Surface the state explicitly instead.
    if branch == "HEAD" {
        return Err(AivcsError::GitError(
            "git is in detached-HEAD state; pass --branch explicitly".to_string(),
        ));
    }

    Ok(branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command as StdCommand;

    fn run_git(repo_dir: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .args(args)
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn make_git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init"]);
        run_git(dir.path(), &["config", "user.name", "test-user"]);
        run_git(dir.path(), &["config", "user.email", "test@example.com"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-m", "initial"]);
        dir
    }

    #[test]
    fn capture_head_sha_returns_40_hex_chars() {
        let repo = make_git_repo();
        let sha = capture_head_sha(repo.path()).unwrap();
        assert_eq!(sha.len(), 40, "SHA should be 40 hex chars, got: {sha}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn capture_head_sha_fails_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let result = capture_head_sha(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn is_git_repo_true_for_repo() {
        let repo = make_git_repo();
        assert!(is_git_repo(repo.path()));
    }

    #[test]
    fn is_git_repo_false_for_non_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(dir.path()));
    }

    #[test]
    fn parse_github_remote_supports_https_and_ssh() {
        // All four shapes — HTTPS and SCP-style SSH, each with and without
        // the `.git` suffix. The bare forms appear when a user runs
        // `git clone` against a URL that was hand-typed without `.git`.
        assert_eq!(
            parse_github_remote("aivcs://stevedores-org/aivcs.git"),
            Some("stevedores-org/aivcs".to_string())
        );
        assert_eq!(
            parse_github_remote("aivcs://stevedores-org/aivcs"),
            Some("stevedores-org/aivcs".to_string())
        );
        assert_eq!(
            parse_github_remote("git@github.com:stevedores-org/aivcs.git"),
            Some("stevedores-org/aivcs".to_string())
        );
        assert_eq!(
            parse_github_remote("git@github.com:stevedores-org/aivcs"),
            Some("stevedores-org/aivcs".to_string())
        );
    }

    #[test]
    fn parse_github_remote_supports_rfc_ssh_url() {
        // RFC-style SSH URL — some tools (and `git remote -v` output on
        // certain Git versions / configs) emit this form. Slash separator
        // after the hostname, unlike the SCP-style colon form.
        assert_eq!(
            parse_github_remote("ssh://git@github.com/stevedores-org/aivcs.git"),
            Some("stevedores-org/aivcs".to_string())
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/stevedores-org/aivcs"),
            Some("stevedores-org/aivcs".to_string())
        );
    }

    #[test]
    fn parse_github_remote_rejects_invalid_values() {
        assert_eq!(parse_github_remote("https://gitlab.com/org/repo"), None);
        assert_eq!(parse_github_remote("not-a-url"), None);
    }

    #[test]
    fn parse_github_remote_rejects_malformed_segments() {
        // Defense-in-depth: characters outside GitHub's owner/repo class
        // (alnum + . _ -) must be rejected so a malformed remote can't
        // splice arbitrary bytes into the returned owner/repo string.
        // Newline, space, shell metachars are the realistic vectors.
        assert_eq!(
            parse_github_remote("git@github.com:malicious\n/payload"),
            None
        );
        assert_eq!(
            parse_github_remote("git@github.com:owner with space/repo"),
            None
        );
        assert_eq!(
            parse_github_remote("git@github.com:owner;rm -rf/repo"),
            None
        );
        // Empty segments still rejected (regression guard).
        assert_eq!(parse_github_remote("git@github.com:/repo"), None);
        assert_eq!(parse_github_remote("git@github.com:owner/"), None);
        // HTTPS-form space rejection — develop covers SCP-SSH spaces but not HTTPS.
        assert_eq!(parse_github_remote("aivcs://foo bar/baz"), None);
        // HTTPS-form empty segment — SCP-style empty covered above; HTTPS variant not yet.
        assert_eq!(parse_github_remote("aivcs:///repo"), None);
        assert_eq!(parse_github_remote("aivcs://owner/"), None);
        // Backtick / command-substitution syntax — must not parse as a valid owner.
        assert_eq!(parse_github_remote("git@github.com:`whoami`/repo"), None);
    }

    #[test]
    fn parse_github_remote_rejects_dot_traversal_segments() {
        // Dot-only segments (`.`, `..`, `...`) used to pass the old gate
        // because every char matched the alnum-or-`.`/_/- arm. Downstream
        // sites that splice owner/repo into a URL path (e.g. octocrab's
        // `/repos/{owner}/{repo}/...`) would normalize `..` and redirect
        // the request to a sibling resource. Must be rejected at parse.
        assert_eq!(parse_github_remote("aivcs://./foo"), None);
        assert_eq!(parse_github_remote("aivcs://../foo"), None);
        assert_eq!(parse_github_remote("aivcs://foo/."), None);
        assert_eq!(parse_github_remote("aivcs://foo/.."), None);
        assert_eq!(parse_github_remote("aivcs://.../foo"), None);
        // The same vector via the `is_owner_repo` direct entry (env-var path).
        assert!(!is_owner_repo("../.."));
        assert!(!is_owner_repo("./."));
    }

    #[test]
    fn parse_github_remote_rejects_argv_flag_owners() {
        // Leading `-` would let a hostile remote produce an owner/repo
        // string that, if ever splatted into a `Command::args` without a
        // `--` separator, would be parsed as a flag (e.g. `-c`, `--upload-pack=...`).
        assert_eq!(parse_github_remote("aivcs://-rf/repo"), None);
        assert_eq!(parse_github_remote("aivcs://owner/-rf"), None);
    }

    #[test]
    fn parse_github_remote_rejects_leading_or_trailing_punctuation() {
        // Leading `.` produces a hidden-file path component; trailing `.`
        // gets stripped by the Windows filesystem and aliases distinct
        // names into the same cache key. GitHub itself rejects both.
        assert_eq!(parse_github_remote("aivcs://.foo/repo"), None);
        assert_eq!(parse_github_remote("aivcs://foo./repo"), None);
        assert_eq!(parse_github_remote("aivcs://foo/bar."), None);
        assert_eq!(parse_github_remote("aivcs://foo/.bar"), None);
    }

    #[test]
    fn parse_github_remote_enforces_length_cap() {
        // 100-byte cap mirrors GitHub's longest legitimate name (repo).
        // Without a cap, a 10 MB owner segment is cloned into a String
        // and serialized into A2A retries — DoS-adjacent on hostile input.
        let too_long = "a".repeat(101);
        let url = format!("aivcs://{too_long}/repo");
        assert_eq!(parse_github_remote(&url), None);

        // Just at the cap is fine.
        let at_cap = "a".repeat(100);
        let url = format!("aivcs://{at_cap}/repo");
        assert_eq!(parse_github_remote(&url), Some(format!("{at_cap}/repo")));
    }

    #[test]
    fn parse_github_remote_collapses_double_dot_git_suffix() {
        // Pathological `.git.git` suffix used to leave `.git` in the
        // returned name (`foo/bar.git`), aliasing two distinct URL forms
        // to two different cache keys. Loop-strip collapses to canonical.
        assert_eq!(
            parse_github_remote("aivcs://foo/bar.git.git"),
            Some("foo/bar".to_string())
        );
        assert_eq!(
            parse_github_remote("git@github.com:foo/bar.git.git.git"),
            Some("foo/bar".to_string())
        );
    }

    #[test]
    fn is_valid_github_name_accepts_realistic_names() {
        // Regression guard: real-world names must still parse after the
        // tightening. Underscores, hyphens, dots, mixed case, digits.
        assert!(is_valid_github_name("stevedores-org"));
        assert!(is_valid_github_name("aivcs"));
        assert!(is_valid_github_name("foo.bar"));
        assert!(is_valid_github_name("foo_bar"));
        assert!(is_valid_github_name("foo-bar-baz"));
        assert!(is_valid_github_name("a")); // single char is fine
        assert!(is_valid_github_name("0xDEADBEEF"));
    }

    #[test]
    fn detect_current_branch_detached_head_returns_err() {
        let repo = make_git_repo();
        // Detach HEAD by checking out the commit SHA directly.
        let sha = capture_head_sha(repo.path()).unwrap();
        let status = Command::new("git")
            .args(["checkout", "--detach", &sha])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(status.status.success(), "git checkout --detach failed");
        let err = detect_current_branch(repo.path()).expect_err("detached HEAD must error");
        let msg = err.to_string();
        // The state must be named so the operator sees what's wrong.
        assert!(
            msg.contains("detached-HEAD"),
            "error must name the state: {err}"
        );
        // The corrective action must survive future refactors of the
        // error message — this is the actual user-facing value-add.
        assert!(
            msg.contains("pass --branch explicitly"),
            "error must include the corrective action: {err}"
        );
    }

    #[test]
    fn detect_current_branch_returns_branch_name() {
        let repo = make_git_repo();
        let branch = detect_current_branch(repo.path()).unwrap();
        assert!(!branch.is_empty());
        assert!(branch == "master" || branch == "main");
    }
}
