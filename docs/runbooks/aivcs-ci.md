# Runbook: AIVCS CI (`aivcs ci run`)

`aivcs ci run` runs CI stages **and records the execution** as part of the
AIVCS ledger. It is distinct from the repository's GitHub Actions workflow and
from running raw `cargo`/`local-ci` (see
[ci-troubleshooting.md](./ci-troubleshooting.md)).

> Confirm the live surface with `aivcs ci run --help`.

## Usage

```bash
aivcs ci run \
  --workspace . \                 # workspace path (default: current directory)
  --stages fmt,check,clippy,test \ # comma-separated stages (default: fmt,check)
  --no-cache \                    # skip caching
  --fix                           # auto-repair using fix commands
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--workspace` | `.` | Workspace to run against |
| `--stages` | `fmt,check` | Comma-separated stages: `fmt`, `check`, `clippy`, `test` |
| `--no-cache` | off | Skip the stage cache (force a clean run) |
| `--fix` | off | Run fix variants (e.g. `cargo fmt`, `clippy --fix`) to auto-repair |

## `aivcs ci run` vs raw cargo / local-ci

| Use… | When |
|------|------|
| `aivcs ci run` | You want the run **recorded** in the AIVCS ledger, stage caching, and the option to auto-repair (`--fix`) in one command. The default `fmt,check` is a fast pre-commit gate; widen to `fmt,check,clippy,test` for a full local mirror. |
| `local-ci` | You want to reproduce the **GitHub Actions** pipeline exactly before pushing (the canonical pre-push gate). |
| Raw `cargo fmt` / `clippy` / `test` | Ad-hoc, single-stage iteration while editing. |

For diagnosing GitHub Actions failures and the precise cargo invocations CI
uses, see [ci-troubleshooting.md](./ci-troubleshooting.md).

## Typical flows

```bash
# Fast gate while iterating
aivcs ci run

# Full local mirror before opening a PR
aivcs ci run --stages fmt,check,clippy,test

# Let it auto-fix formatting/clippy, then re-run clean
aivcs ci run --stages fmt,clippy --fix
aivcs ci run --stages fmt,check,clippy,test --no-cache
```

## See also

- [CI troubleshooting](./ci-troubleshooting.md)
- [Getting Started — Command map](../getting-started.md#command-map)
