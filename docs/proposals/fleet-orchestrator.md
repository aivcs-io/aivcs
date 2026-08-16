# Proposal: `aivcs-orchestrator` — tool-agnostic headless agent fleet runner

| Field | Value |
|-------|-------|
| **Status** | Draft — design only (no code) |
| **Type** | Feature Request (FR) |
| **Tracking** | In-repo intent FR — GitHub Issues are being sunset; this doc is the source of truth |
| **Parent** | `intent/current.yaml` → OBJ-2026-SDF-002 (dual-ledger epic) |
| **Related** | `harbormaster` (local Rust prototype of this mechanism), [`aivcs-cli`](../../crates/aivcs-cli), [`oxidized-state`](../../crates/oxidized-state), [`aivcs-mcp-gateway`](../../crates/aivcs-mcp-gateway), [`nix-env-manager`](../../crates/nix-env-manager) |

## Summary

AIVCS today version-controls **agent cognition** — snapshots, branches, semantic
merge, time-travel replay tied to `CommitId` and the dual ledger. What it does
**not** own is the step that *produces* that cognition: actually launching the
agents. That orchestration currently happens out-of-band (manual `claude -p …`,
ad-hoc scripts), so the runs that aivcs is meant to record are neither isolated,
reproducible, nor automatically committed.

This FR proposes a new crate, **`aivcs-orchestrator`**, that makes AIVCS a
first-class **fleet runner for headless AI coding CLIs** (`claude`, `codex`,
`cursor-agent`, …). It launches each agent into an isolated git worktree on a
dedicated **aivcs branch**, runs it autonomously, normalizes its streaming output
into typed events, and **auto-snapshots agent state into the dual ledger** at run
boundaries — so every autonomous run is reproducible and replayable by `CommitId`
out of the box.

The mechanism is prototyped in the standalone `harbormaster` tool (Rust:
config-driven tool registry, git-worktree isolation, stream normalization, fleet
batches, TUI monitor). This FR folds that mechanism **into aivcs as a native
capability** rather than maintaining it as a separate repo.

## Motivation

- **Close the loop.** AIVCS commits cognition but does not start the agents that
  generate it. An in-tree orchestrator means *every* agent run is born inside the
  ledger — no manual `aivcs snapshot` bracketing.
- **Tool-agnostic.** Teams run claude / codex / cursor-agent interchangeably.
  Adding a tool should be a **config change, not a code change** (registry model
  proven in harbormaster's `tools.toml`).
- **Isolation by construction.** One worktree + one aivcs branch per run means
  parallel agents never collide, and each maps cleanly to one `CommitId` and one
  Intent-ID.
- **Fleets.** "Many agents, many tools, one command" — batch an entire
  intent/epic across a pool with bounded concurrency.

## Design

### New crate: `crates/aivcs-orchestrator`

| Concern | Approach |
|---------|----------|
| Tool registry | TOML (`config/tools.toml`): per-tool `binary`, `exec_args`, `resume_args`, `autonomy_args`, `model_args`, `output_format`, `instruction_file`. Template tokens `{prompt}` `{model}` `{session}`. |
| Isolation | One git worktree on an **aivcs branch** per run (reuse the branching path in `oxidized-state` / `aivcs-core`, not raw `git worktree` alone). |
| Event model | Normalize each tool's stdout (`claude-stream-json` \| `codex-json` \| `cursor-stream-json` \| `text`) into a typed `RunEvent` stream (`Say`, `Tool`, `Diff`, `Done`, `Error`). |
| Ledger integration | Auto `snapshot` at run start (env/flake hash via `nix-env-manager`) and at run end (final cognition state); emit `CODE_COMMITTED` A2A event after durable commit; stamp the run's `CommitId` + `Intent-ID`. |
| Fleet | `[[task]]` list with `concurrency`; each task → isolated worktree + autonomous run. |
| Monitor | `ls` / `watch` (TUI) / `logs <CommitId>`; later surfaced through `aivcs-mcp-gateway` as an `orchestrator::*` tool family. |

### CLI surface (via `aivcs-cli`)

```
aivcs run   --tool claude --intent OBJ-2026-SDF-002 "<task prompt>"   # one agent, one branch, auto-snapshotted
aivcs fleet ./fleet.toml                                              # batch, bounded concurrency
aivcs runs  ls | watch | logs <CommitId>                              # monitor / replay
```

### How it differs from harbormaster

Harbormaster isolates with **raw git worktrees** and persists session metadata to
a local store. The aivcs-native version replaces that store with the **dual
ledger**: a run *is* a `CommitId`, isolation rides aivcs branches, and env
reproducibility uses `nix-env-manager`. Harbormaster's `tools.toml`, stream
normalizers, fleet runner, and monitor port over largely as-is.

## Acceptance criteria

- `aivcs run --tool claude --intent <ID> "<task>"` creates an aivcs branch + git
  worktree, runs the agent autonomously within it, and produces a replayable
  `CommitId`.
- Adding a new tool (e.g. `agy`) requires **only** a `config/tools.toml` edit.
- `aivcs fleet <file>` runs N tasks across tools with bounded concurrency, one
  branch/`CommitId` each.
- Each run emits a `CODE_COMMITTED` A2A event after its durable commit and stamps
  the originating `Intent-ID`.
- `aivcs runs logs <CommitId>` returns the normalized event stream for any run.
- `cargo test -p aivcs-orchestrator` passes; no secrets in repo.

## Out of scope

- GUI beyond the terminal monitor.
- Live model API wiring beyond launching the existing headless CLIs.
- Changes to the semantic-merge arbiter (`semantic-rag-merge`).

## Open questions

1. Branch namespace — reuse `feat/<intent>` or a dedicated `run/<id>` space?
2. Should `aivcs-orchestrator` shell out to the `aivcs` binary for snapshots, or
   depend on `aivcs-core` directly? (Prefer the crate dependency.)
3. Fleet scheduling — in-process pool now, or defer to an external queue later?
