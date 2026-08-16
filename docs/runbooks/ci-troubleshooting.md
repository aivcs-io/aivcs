# CI Troubleshooting Runbook

## CI Overview

CI runs on every push to `develop`/`main` and on all pull requests. Two jobs:

| Job | Tool | Blocking? |
|---|---|---|
| `local-ci` | `stevedores-org/local-ci` (Go) | Yes |
| `nix-report` | `nix flake check` | No (report-only) |

### local-ci

`local-ci --json` discovers and runs checks defined in the repository. For AIVCS this includes:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

### nix-report

Runs `nix flake check` and uploads the log as a build artifact. Failures here do **not** block merge.

## Common Failures

### Clippy warnings

```
error: ... implied by `-D warnings`
```

**Fix:** Run `cargo clippy --all -- -D warnings` locally, fix all warnings, then push.

### Format check failure

```
Diff in src/foo.rs
```

**Fix:** Run `cargo fmt --all` locally, commit the formatted files.

### Test failure

**Fix:** Run `cargo test --all` locally. For flaky tests involving SurrealDB, ensure no global state leaks between tests (each test should call `SurrealHandle::setup_db()` for a fresh in-memory instance).

### local-ci not found

```
local-ci: command not found
```

CI installs it via `go install github.com/stevedores-org/local-ci@latest`. If building locally:

```bash
go install github.com/stevedores-org/local-ci@latest
local-ci --json
```

### Nix flake check failure (non-blocking)

Check the uploaded `nix-flake-check-log` artifact in the GitHub Actions run. Common issues:

- Missing flake input → update `flake.lock` with `nix flake update`
- Build failure → check Rust compilation errors in the Nix derivation

## Reproduce CI Locally

```bash
# Exact CI sequence
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Or use local-ci directly
go install github.com/stevedores-org/local-ci@latest
local-ci --json
```
