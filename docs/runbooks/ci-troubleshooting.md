# CI Troubleshooting Runbook

## CI Overview

CI runs on every push to `develop`/`main` and on all pull requests.

| Job | Tool | Blocking? |
|---|---|---|
| `propel` | Propel Engine (`propel.toml`) | Yes |

### Propel checks

Propel discovers and runs checks defined in `propel.toml` hermetically via Nix. For AIVCS this includes:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `gitleaks detect` (secrets scan)

## Common Failures

### Clippy warnings

```
error: ... implied by `-D warnings`
```

**Fix:** Run `cargo clippy --workspace --all-targets -- -D warnings` locally, fix all warnings, then push.

### Format check failure

```
Diff in src/foo.rs
```

**Fix:** Run `cargo fmt --all` locally, commit the formatted files.

### Test failure

**Fix:** Run `cargo test --all` locally. For flaky tests involving SurrealDB, ensure no global state leaks between tests (each test should call `SurrealHandle::setup_db()` for a fresh in-memory instance).

### Secrets scan failure

**Fix:** Ensure no private keys, passwords, or tokens are committed. Remove sensitive content, update the history if necessary, and re-run.

## Reproduce CI Locally

```bash
# Exact CI sequence using Nix devShell
nix develop -c cargo fmt --all -- --check
nix develop -c cargo clippy --workspace --all-targets -- -D warnings
nix develop -c cargo test --workspace
```
