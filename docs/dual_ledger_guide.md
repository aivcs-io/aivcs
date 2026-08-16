# AIVCS Dual Ledger Local Guide

This guide describes how to use the Dual Ledger (Git + AIVCS linkage) features implemented in Phase 0.

## Overview

The Dual Ledger pattern links Git commits/pull requests (which track deterministic source code changes) with AIVCS commits (which track agent cognition, prompt graphs, memory states, and env hashes).

```
  GitHub Pull Request
  ┌──────────────────────────────────────────────┐
  │ title: "feat: update solver logic"           │
  │                                              │
  │ <!-- aivcs-linkage -->                       │
  │ aivcs-commit: aivcs_commit_abc123            │
  └──────────────────────┬───────────────────────┘
                         │
                         ▼
             Links to AIVCS DB State
             - Cognitive Graph Topology
             - Prompts & Memory Vectors
             - Replayable Execution Path
```

---

## 1. Generating PR Notes (`aivcs pr-note`)

To link a pull request to the corresponding AIVCS cognitive commit, run the `pr-note` command:

```bash
aivcs pr-note
```

By default, it will detect the current Git branch and retrieve the head AIVCS commit from the SurrealDB instance. If you want to specify a different branch:

```bash
aivcs pr-note --branch feat/my-experiment
```

### Example Output

```markdown
<!-- aivcs-linkage -->
aivcs-commit: 6a2c3b8f1d...
intent-id: objective_xyz...

### AIVCS Commit Summary
- **Message**: Update RAG prompt weights
- **Author**: agent-solver-3
- **Created**: 2026-06-13 12:00:00 UTC
- **State Hash**: 5f9c...
- **Logic Hash**: 7b2a...
- **Env Hash**: 1c8d...
```

Paste the entire output (including the `<!-- aivcs-linkage -->` tag and metadata fields) into your GitHub PR description.

---

## 2. Event-Driven Dual Ledger (`CODE_COMMITTED` Payload)

When commits are pushed or a pipeline runs, `aivcs` automatically emits a `CODE_COMMITTED` payload to your configured event hub. The payload now includes `aivcs_commit_id`:

```json
{
  "jsonrpc": "2.0",
  "method": "aivcs_code_committed",
  "params": {
    "event": {
      "payload": {
        "repo": "stevedores-org/aivcs",
        "branch": "develop",
        "commit_sha": "git_commit_sha_xyz...",
        "aivcs_commit_id": "aivcs_commit_abc123...",
        "changed_paths": ["intent/current.yaml"]
      }
    }
  }
}
```

---

## 3. Replaying/Verifying Local Runs

To verify the state associated with an AIVCS commit, reviewers can restore the state snapshot for the given `CommitId` and replay the session locally.

### Step 1: Restore the state snapshot from the CommitId
```bash
aivcs restore <CommitId> --output restored_state.json
```

### Step 2: Replay the run artifact
If a recorded run artifact is available on disk, you can verify its deterministic execution digest by running:

```bash
aivcs replay-artifact --run <run-id>
```

This will run the execution path using the captured prompts and graph structure, verifying the output digest matches the golden record.
