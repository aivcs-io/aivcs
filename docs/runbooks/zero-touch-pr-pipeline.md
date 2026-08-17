# Zero-Touch PR Pipeline

> **Status: implemented / current behavior.** This runbook describes how the pipeline works today, not planned work. The `pr pipeline` / `pr branch` / `pr commit` / `pr open` subcommands and `CODE_COMMITTED` emission are all shipped — verify with `aivcs pr --help`.

Autonomous builder agents running in ephemeral Jobs use `aivcs` to branch, commit, open Pull Requests, and emit `CODE_COMMITTED` A2A events — without a local git checkout or human in the loop.

## Flow

```mermaid
sequenceDiagram
    participant Job as Agent Job
    participant AIVCS as aivcs
    participant GH as GitHub API
    participant Lib as Librarian Agent
    participant A2A as A2A JSON-RPC

    Job->>AIVCS: pr pipeline (or branch → commit → open)
    AIVCS->>GH: create branch
    AIVCS->>GH: commit file (Contents API)
    AIVCS->>A2A: CODE_COMMITTED (if URL configured)
    AIVCS->>GH: open PR
    AIVCS->>GH: request review (Librarian)
    GH-->>Lib: review requested
```

## Single-command path

```bash
aivcs pr pipeline \
  --branch "feat/my-change" \
  --base main \
  --path docs/example.md \
  --file ./example.md \
  --message "docs: add example" \
  --title "feat: my change" \
  --body "Closes issue #191" \
  --owner aivcs-io \
  --repo aivcs
```

Use `--skip-branch` when retrying after a partial run where the branch already exists.

## Step-by-step path

1. `aivcs pr branch`
2. `aivcs pr commit` — emits `CODE_COMMITTED` when `AIVCS_A2A_JSONRPC_URL` is set
3. `aivcs pr open` — requests Librarian review by default

## Required environment

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub API token (preferred). |
| `GITHUB_TOKEN_FILE` | Alternative: path to a mounted token file (e.g. `/path/to/token`). Used when `GITHUB_TOKEN` is unset or whitespace-only. |
| `RELIC_LIBRARIAN_USERNAME` | GitHub username of the Librarian Agent. Required when `--librarian` is enabled (default). |
| `AIVCS_A2A_JSONRPC_URL` | JSON-RPC endpoint for A2A events. Absent ⇒ `CODE_COMMITTED` emission is skipped (no-op). |
| `AIVCS_AGENT_ID` | Authoring agent ID in the event payload. Falls back to a pipeline-specific default. |
| `AIVCS_JOB_ID` | Optional run correlation ID. |
| `GITHUB_REPOSITORY` | `owner/repo` for the `CODE_COMMITTED` payload. Set in CI/Jobs when no local git remote exists. |

## Librarian review

Every PR opened via `aivcs pr open` or `aivcs pr pipeline` requests review from the Librarian by default. Pass `--librarian=false` only in local dev or test contexts where the Librarian is not deployed.
