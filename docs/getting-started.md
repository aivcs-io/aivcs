# Getting Started with AIVCS

## Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| Rust toolchain | stable (1.75+) | Install via [rustup](https://rustup.rs/) |
| Git | 2.x | For SHA linking and repo detection |
| Nix *(optional)* | 2.18+ | Only needed for `aivcs env` commands |
| Attic *(optional)* | latest | Only needed for binary cache features |

For a reproducible **NixOS-WSL** environment (Windows + WSL2), see [docs/runbooks/nixos-wsl.md](runbooks/nixos-wsl.md).

## Installation
 
### Option A: Homebrew (macOS & Linux)
```bash
brew install aivcs-io/tap/aivcs
```

### Option B: Prebuilt Binaries (GitHub Releases)
```bash
# macOS (arm64)
curl -sL https://github.com/aivcs-io/aivcs/releases/download/v0.4.4/aivcs-darwin-arm64 -o ~/.local/bin/aivcs
chmod +x ~/.local/bin/aivcs

# Linux (x86_64)
curl -sL https://github.com/aivcs-io/aivcs/releases/download/v0.4.4/aivcs-linux-x86_64 -o ~/.local/bin/aivcs
chmod +x ~/.local/bin/aivcs
```

### Option C: Build from Source
```bash
# Clone
aivcs clone aivcs://aivcs/aivcs ./aivcs
cd aivcs

# Build release binary
cargo build --release

# The binary is at:
./target/release/aivcs --version
```

## First-Run Walkthrough

### 1. Initialise a repository

```bash
aivcs init
```

This creates an initial commit and a `main` branch in the backing SurrealDB store.

### 2. Create a state snapshot

```bash
echo '{"step": 1, "memory": "learned X"}' > state.json
aivcs snapshot --state state.json --message "First snapshot"
```

### 3. View history

```bash
aivcs log
```

### 4. Branch and explore

```bash
aivcs branch create experiment-1
aivcs snapshot --state state.json --message "Experiment" --branch experiment-1
```

### 5. Merge back

```bash
aivcs merge experiment-1 --target main
```

### 6. Restore a previous state

```bash
aivcs restore <commit-id> --output restored.json
```

## Command map

The first-run walkthrough covers the core versioning loop. The CLI exposes several
more command families — confirm the live surface with `aivcs --help` and
`aivcs <command> --help`.

| Area | Commands | Where to look |
|------|----------|---------------|
| Versioning | `init`, `snapshot`, `restore`, `log`, `branch`, `merge` | this walkthrough |
| Run inspection | `replay-artifact --run <run-id>`, `diff spec`, `diff run`, `diff-runs --run-a/--run-b` | `aivcs <cmd> --help` |
| Releases | `release promote` / `current` / `history` / `rollback` | [release-workflow runbook](./runbooks/release-workflow.md) |
| CI | `ci run --stages fmt,check,clippy,test [--no-cache] [--fix]` | [aivcs-ci runbook](./runbooks/aivcs-ci.md) |
| Reports | `report cross-org --objective <id> --output <file>` | `aivcs report cross-org --help` |
| GitHub PRs | `pr open` / `branch` / `commit` / `pipeline`, `pr-note` | [zero-touch PR pipeline](./runbooks/zero-touch-pr-pipeline.md) |

> The run-inspection command is `replay-artifact` (there is no `replay` alias).

### What next? (common commands)

```bash
# Validate a workspace locally before pushing
aivcs ci run --stages fmt,check,clippy,test

# Promote a validated agent spec, then inspect the release pointer
aivcs release promote my-agent --git-sha <sha> \
  --graph-digest <h> --prompts-digest <h> --tools-digest <h> --config-digest <h>
aivcs release current my-agent

# Autonomous branch → commit → PR in one shot (base defaults to develop)
aivcs pr pipeline --branch feature/x --path docs/x.md --file ./x.md \
  --message "docs: x" --title "feat: x" --body "…" --owner aivcs-io --repo aivcs
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `SURREALDB_ENDPOINT` | *(in-memory)* | WebSocket URL for SurrealDB Cloud |
| `SURREALDB_USERNAME` | — | Database user |
| `SURREALDB_PASSWORD` | — | Database password |
| `SURREALDB_NAMESPACE` | `aivcs` | SurrealDB namespace |
| `SURREALDB_DATABASE` | `main` | SurrealDB database name |
| `ATTIC_SERVER` | — | Attic binary cache server URL |
| `ATTIC_CACHE` | — | Attic cache name |
| `ATTIC_TOKEN` | — | Attic authentication token |
| `RUST_LOG` | `info` | Tracing filter (e.g. `debug`, `aivcs_core=trace`) |

## Next Steps

- [Architecture overview](./architecture.md)
- [Local development runbook](./runbooks/local-development.md)
- [Database configuration runbook](./runbooks/database-configuration.md)
- [Release workflow runbook](./runbooks/release-workflow.md)
- [AIVCS CI (`aivcs ci run`) runbook](./runbooks/aivcs-ci.md)
- [Zero-touch PR pipeline runbook](./runbooks/zero-touch-pr-pipeline.md)
- [CI troubleshooting runbook](./runbooks/ci-troubleshooting.md)
