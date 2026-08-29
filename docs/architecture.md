# AIVCS Architecture

## Crate Layers

```
Layer 4  aivcs-cli          Clap CLI, command dispatch
Layer 3  semantic-rag-merge  Memory vector diff + heuristic conflict resolution
Layer 2  nix-env-manager     Nix Flake hashing + Attic binary cache
Layer 1  aivcs-core          Domain logic, CAS, recording, replay, parallel fork
Layer 0  oxidized-state      SurrealDB persistence (snapshots, commits, branches, runs)
         aivcsd              Daemon stub (placeholder)
```

### Dependency Flow

```
aivcs-cli ──► aivcs-core ──► oxidized-state
                           ──► nix-env-manager
                           ──► semantic-rag-merge ──► oxidized-state
```

## Key Abstractions

### CommitId (content-addressed)

A composite SHA-256 hash derived from `logic_hash + state_digest + env_hash`. Ensures that identical agent state, source code, and environment always produce the same commit ID.

### SurrealHandle (Layer 0)

Wraps a SurrealDB connection (in-memory or WebSocket). Provides CRUD for:

- `SnapshotRecord` — JSON state blobs keyed by commit hash
- `CommitRecord` — commit metadata (parents, message, author, timestamp)
- `BranchRecord` — named pointers to commit hashes
- `GraphEdge` — parent → child edges for history traversal
- `MemoryRecord` — key-value memory vectors for semantic merge

### CasStore / FsCasStore (Layer 1)

Content-addressed store with SHA-256 digests. `FsCasStore` writes blobs to disk under `.aivcs/cas/`. Used during `snapshot` to deduplicate state.

### RunLedger (Layer 0)

Trait for recording execution runs. Implementations:

- `MemoryRunLedger` — in-memory (tests)
- `SurrealRunLedger` — persisted to SurrealDB

Each run consists of `RunEvent` records (seq, kind, payload, timestamp) that can be replayed deterministically.

### GraphRunRecorder (Layer 1)

Domain-level event recorder that maps `aivcs_core::Event` variants to `RunEvent` entries and persists them through the `RunLedger`.

### ReleaseRegistry (Layer 0)

Tracks per-agent release history. Supports `promote` (append new release), `current` (latest pointer), `rollback` (restore previous), and `history` (ordered list).

### ParallelManager (Layer 1)

Tracks branch scores and step counts during parallel exploration. `fork_agent_parallel` spawns N concurrent Tokio tasks that each create a branch + commit + snapshot from a parent commit.

## Data Flow: Snapshot Command

```
CLI: aivcs snapshot --state state.json --branch main
  │
  ├─ Read state.json
  ├─ Detect git HEAD SHA
  ├─ Generate logic hash (Rust src) + env hash (flake.lock)
  ├─ CAS put → Digest
  ├─ CommitId::new(logic, state_digest, env)
  ├─ SurrealHandle::save_snapshot(commit_id, state)
  ├─ SurrealHandle::save_commit(record)
  ├─ SurrealHandle::save_commit_graph_edge(child, parent)
  └─ SurrealHandle::save_branch(updated head)
```

## Data Flow: Semantic Merge

```
CLI: aivcs merge feature --target main
  │
  ├─ Resolve branch heads → source_commit, target_commit
  ├─ semantic_merge(handle, source, target, msg, author)
  │   ├─ Load memories for both commits
  │   ├─ diff_memory_vectors → VectorStoreDelta
  │   ├─ resolve_conflict_state (heuristic: prefer longer content)
  │   ├─ synthesize_memory → merged memories
  │   ├─ Save merged snapshot + commit + graph edges
  │   └─ Return MergeResult
  └─ Update target branch head
```
