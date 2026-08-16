# Proposal: Unified MCP `memory::*` Surface (mom + aivcs)

| Field | Value |
|-------|-------|
| **Status** | Draft — design only (no code) |
| **Issue** | [stevedores-org/aivcs#248](aivcs://stevedores-org/aivcs/issues/248) |
| **Parent** | [stevedores-org/aivcs#220](aivcs://stevedores-org/aivcs/issues/220) Phase 2.2 |
| **Related** | [lornu-ai/mom](aivcs://lornu-ai/mom) — event-sourced memory kernel |

## Summary

Today agents can reach **two unrelated memory systems**:

1. **mom** — durable, tenant-scoped, event-sourced `MemoryItem`s with embeddings, hybrid recall, `ContextPack`, and typed `TaskRecord` / `CheckpointRecord` semantics (`mom-core`, `mom-store-surrealdb`, `mom-embeddings`, `mom-sources`).
2. **aivcs** — CommitId-scoped session memory tied to the dual ledger (`oxidized-state::MemoryRecord`, `aivcs-core` memory index / context assembly, `semantic-rag-merge` vector diff).

`aivcs-mcp-gateway` currently exposes only repo diff/merge tools — no memory family. Phase 2.2 of the dual-ledger epic requires a **single canonical MCP surface** so agents see one ID scheme, one retrieval model, and one auth/audit path.

This document proposes the `memory::*` tool family the gateway will expose, which backend each tool delegates to, how IDs are namespaced, and how existing mom HTTP callers migrate.

---

## 1. Tool list

All tools are exposed by `aivcs-mcp-gateway` under the `memory::` prefix. Parameters inherit **scope** from the MCP token (`tenant_id`, `agent_id`, `run_id`, `repo`) and accept an optional **`commit_id`** when the caller wants CommitId-scoped session memory instead of (or in addition to) long-term mom memory.

Risk levels assume the gateway's existing `max_risk` / scope model ([mcp-auth-guide](../mcp-auth-guide.md)).

### `memory::write`

**Purpose:** Persist a memory item — either a durable mom event/summary/fact or a CommitId-bound session vector used for replay and semantic merge.

**Parameters (JSON Schema sketch):**

```json
{
  "type": "object",
  "required": ["kind", "content"],
  "properties": {
    "kind": {
      "enum": ["event", "summary", "fact", "preference", "task", "checkpoint", "session_vector"]
    },
    "content": {
      "oneOf": [
        { "type": "string" },
        { "type": "object" }
      ]
    },
    "tags": { "type": "array", "items": { "type": "string" } },
    "importance": { "type": "number", "minimum": 0, "maximum": 1 },
    "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
    "source": { "type": "string" },
    "commit_id": { "type": "string", "description": "Required when kind=session_vector; optional provenance anchor otherwise" },
    "key": { "type": "string", "description": "Required when kind=session_vector (oxidized-state MemoryRecord key)" },
    "task_id": { "type": "string", "description": "For checkpoint writes" },
    "step": { "type": "integer", "description": "For checkpoint writes" },
    "ttl_ms": { "type": "integer" }
  }
}
```

**Result:**

```json
{
  "memory_id": "mom:uuid-or-hash",
  "commit_id": "optional-aivcs-commit-id",
  "created_at_ms": 1718400000000
}
```

**Returns:** A unified `memory_id` (see §3). For `session_vector`, the ID is CommitId-namespaced; for mom kinds, the underlying mom `MemoryId` is wrapped.

---

### `memory::query`

**Purpose:** Retrieve ranked memories by structured filters and/or natural-language query. Covers list, recall, semantic, and hybrid search behind one tool.

**Parameters:**

```json
{
  "type": "object",
  "properties": {
    "query_text": { "type": "string" },
    "kinds": { "type": "array", "items": { "type": "string" } },
    "tags_any": { "type": "array", "items": { "type": "string" } },
    "since_ms": { "type": "integer" },
    "until_ms": { "type": "integer" },
    "limit": { "type": "integer", "default": 20 },
    "mode": {
      "enum": ["filter", "lexical", "semantic", "hybrid"],
      "default": "hybrid"
    },
    "commit_id": { "type": "string", "description": "When set, also search CommitId-scoped session vectors" },
    "scope_commits": { "type": "array", "items": { "type": "string" } }
  }
}
```

**Result:**

```json
{
  "items": [
    {
      "memory_id": "mom:…",
      "kind": "event",
      "score": 0.87,
      "content_preview": "…",
      "created_at_ms": 1718400000000,
      "citation": { "source": "user", "tags": ["pr-123"] }
    }
  ]
}
```

**Returns:** Ranked hits. Hybrid mode maps to mom's `hybrid_recall`; CommitId-scoped leg uses `aivcs-core` `MemoryQuery` against `oxidized-state` records.

---

### `memory::context_pack`

**Purpose:** Assemble a token-budgeted bundle for prompt injection — mom's `ContextPack` (highlights, summaries, facts, citations) plus optional CommitId-scoped session excerpts.

**Parameters:**

```json
{
  "type": "object",
  "required": ["query_text"],
  "properties": {
    "query_text": { "type": "string" },
    "budget_tokens": { "type": "integer", "default": 3000 },
    "kinds": { "type": "array", "items": { "type": "string" } },
    "commit_id": { "type": "string" },
    "include_session": { "type": "boolean", "default": true }
  }
}
```

**Result:** Mirrors mom `ContextPack`:

```json
{
  "highlights": [],
  "summaries": [],
  "facts": [],
  "citations": [],
  "session_excerpts": [],
  "estimated_tokens": 2400,
  "budget_tokens": 3000
}
```

**Returns:** Structured context suitable for agent system prompts. Implementation composes mom `build_context_pack` output with `aivcs-core` `assemble_context` when `commit_id` is present.

---

### `memory::summarize`

**Purpose:** Produce a new **summary** memory from a set of source memories or a time window — compaction/retention, not raw retrieval.

**Parameters:**

```json
{
  "type": "object",
  "properties": {
    "source_memory_ids": { "type": "array", "items": { "type": "string" } },
    "since_ms": { "type": "integer" },
    "until_ms": { "type": "integer" },
    "kinds": { "type": "array", "items": { "type": "string" } },
    "max_source_items": { "type": "integer", "default": 50 },
    "commit_id": { "type": "string", "description": "When summarizing session vectors for a replay branch" }
  }
}
```

**Result:**

```json
{
  "memory_id": "mom:…",
  "kind": "summary",
  "content": "…",
  "source_count": 12,
  "estimated_tokens": 450
}
```

**Returns:** A newly written summary `MemoryItem` (mom-backed) or a CommitId-bound summary key (session leg). Uses mom for durable summaries; uses `aivcs-core` retention/compaction policies for session leg.

---

### `memory::task_record`

**Purpose:** Typed task lifecycle and durable checkpoints — wraps mom `TaskRecord` / `CheckpointRecord` without exposing raw JSON conventions to agents.

**Parameters (discriminated by `action`):**

```json
{
  "type": "object",
  "required": ["action"],
  "properties": {
    "action": {
      "enum": ["create", "update_status", "checkpoint", "resume", "get"]
    },
    "task_id": { "type": "string" },
    "description": { "type": "string" },
    "status": { "enum": ["pending", "in_progress", "blocked", "completed", "failed"] },
    "depends_on": { "type": "array", "items": { "type": "string" } },
    "step": { "type": "integer" },
    "scratchpad": { "type": "object" }
  }
}
```

**Result (varies by action):**

```json
{
  "task": {
    "memory_id": "mom:…",
    "status": "in_progress",
    "description": "Implement memory MCP facade"
  },
  "checkpoint": {
    "memory_id": "mom:…",
    "task_id": "…",
    "step": 3,
    "scratchpad": {}
  }
}
```

**Returns:** Typed task/checkpoint views. Maps to mom HTTP `/v1/task/checkpoint` and `/v1/task/resume` semantics internally.

---

## 2. Backend split

| Tool | Primary backend | Secondary / overlay | Rationale |
|------|-----------------|---------------------|-----------|
| `memory::write` | **mom-core** + **mom-store-surrealdb** for `event`, `summary`, `fact`, `preference`, `task`, `checkpoint` | **oxidized-state** `MemoryRecord` + optional embedding via **mom-embeddings** for `session_vector` | Durable, tenant-scoped event log belongs in mom; CommitId-bound replay state belongs in aivcs dual ledger |
| `memory::query` | **mom-store-surrealdb** (`query`, `vector_recall`, `hybrid_recall`) | **aivcs-core** `MemoryIndex` / `MemoryQuery` over **oxidized-state** when `commit_id` or `scope_commits` set | Unified ranking facade; mom owns hybrid retrieval at scale |
| `memory::context_pack` | **mom-core** `build_context_pack` | **aivcs-core** `assemble_context` / `memory_context` for session excerpts | Token budgeting already implemented in both; compose rather than reimplement |
| `memory::summarize` | **mom-core** (writes `MemoryKind::Summary`) | **aivcs-core** `retention::compact_index` for session leg | Summaries are long-lived mom items; session compaction stays CommitId-local |
| `memory::task_record` | **mom-core** `task` module + **mom-store-surrealdb** | None initially | Task/checkpoint conventions already codified in mom; no aivcs equivalent |

**Shared infrastructure (future crate, not in this proposal):**

```text
aivcs-mcp-gateway
    └── memory facade (new: aivcs-memory or aivcs-core::mcp_memory)
            ├── MomBackend trait ([ADR 001](../adr/001-memory-mom-deployment-topology.md))
            │     ├── EmbeddedMomBackend   → mom-core + mom-store-surrealdb (local dev / tests)
            │     └── HttpMomBackend       → mom-service REST (production default)
            └── SessionMemoryBackend → oxidized-state via SurrealHandle
```

**Explicit non-goals for v1 facade:**

- No second SurrealDB schema in aivcs for mom-shaped items — mom-store remains source of truth for `MemoryItem`.
- No re-embedding of session vectors inside oxidized-state until semantic merge requires it (today `MemoryRecord.embedding` is optional).

---

## 3. Identity model

### Unified external ID

Agents always receive a **single string** `memory_id` with a typed prefix:

| Prefix | Meaning | Example |
|--------|---------|---------|
| `mom:` | Durable mom `MemoryId` | `mom:7f3a9c2e-…` |
| `aivcs:` | CommitId-scoped session key | `aivcs:commit_abc123/session/rationale/step-4` |

**Rules:**

1. **mom IDs are never re-issued.** Existing `MemoryId` strings from mom HTTP/MCP callers remain valid; the gateway wraps them as `mom:<id>` on output and strips the prefix on input.
2. **CommitId scope is carried explicitly** on session writes (`commit_id` + `key`) and encoded in `aivcs:` IDs. Session IDs are deterministic: `aivcs:{commit_id}/{namespace}/{key}`.
3. **ScopeKey mapping:** MCP token fields map to mom `ScopeKey`:

   | MCP claim | mom ScopeKey field |
   |-----------|-------------------|
   | `tenant_id` (from auth) | `tenant_id` |
   | `repo` | `workspace_id` |
   | `project` (optional claim) | `project_id` |
   | `agent_id` | `agent_id` |
   | `run_id` | `run_id` |

4. **Cross-linking:** When a mom `MemoryItem` is written during an aivcs run, the gateway SHOULD set `meta.aivcs_commit_id` (mom-side metadata only — no schema change in oxidized-state required for v1).
5. **Dual-ledger replay:** `aivcs replay <CommitId>` loads session vectors via `aivcs:` IDs; mom memories referenced in `meta` or citations are fetched lazily through `memory::query`.

---

## 4. Migration path

**Recommendation: (b) — mom's HTTP surface becomes a thin shim over aivcs, with a parallel transition window.**

| Phase | Duration (target) | Behavior |
|-------|-------------------|----------|
| **T0 — today** | — | mom HTTP (`/v1/memory`, `/v1/context-pack`, …) and aivcs gateway (no memory tools) operate independently |
| **T1 — gateway memory tools** | Q3 2026 | `aivcs-mcp-gateway` exposes `memory::*`; new agent integrations use gateway only |
| **T2 — parallel exposure** | 90 days | mom HTTP routes delegate to the same facade (HTTP → in-process or sidecar call). Existing mom callers unchanged at the wire format |
| **T3 — deprecate direct mom MCP** | T2 + 90 days | If mom gains a standalone MCP server, mark deprecated; document gateway as sole MCP entrypoint |
| **T4 — hard cutover** | T3 + 180 days | mom HTTP retained for non-MCP integrations (batch ingest, ops); agent-facing MCP only via aivcs |

**Why not (a) parallel forever?** Two agent-facing surfaces perpetuates split ID schemes and divergent audit trails.

**Why not (c) immediate hard cutover?** mom already has production HTTP consumers (`/v1/ingest/*`, hybrid search). A shim phase avoids breaking ingest pipelines and SurrealDB tenant isolation tests.

**Compatibility guarantees during T1–T2:**

- mom `MemoryId` values stable
- mom HTTP request/response shapes unchanged (shim translates internally)
- aivcs CommitId session memory readable via both `memory::query(commit_id=…)` and `aivcs replay`

---

## 5. Cross-repo impact (lornu-ai/mom)

mom changes required **before** aivcs can consume it as a backend (implementation tracked in follow-up issues):

| Area | Change | Owner |
|------|--------|-------|
| **Stable kernel API** | Publish semver guarantees for `mom-core` (`MemoryStore`, `MemoryItem`, `Query`, `ContextPack`, `TaskRecord`) | mom |
| **Library-first embedding** | Ensure `mom-service` is a thin Axum shell over `mom-core` + store (already mostly true) | mom |
| **In-process vs HTTP** | Expose a factory (`MomBackend::connect(endpoint \| in_process)`) so aivcs can link `mom-core` directly in monorepo/workspace builds | mom + aivcs |
| **Metadata hook** | Accept optional `meta.aivcs_commit_id` on write; index for query filter (no breaking change) | mom |
| **Drop standalone MCP ambitions** | If planned, defer to aivcs-mcp-gateway; mom remains kernel + HTTP shim | mom maintainers |
| **Embeddings** | Keep model selection inside `mom-embeddings`; aivcs facade calls `Embedder` trait, does not configure models | mom |
| **Sources / ingest** | `mom-sources` + `/v1/ingest/*` stay mom-local; not exposed as MCP tools in v1 (batch/ops concern) | mom |

**aivcs-side (post-proposal, separate issues):**

- New facade module behind `aivcs-mcp-gateway` tool registry ([#239 gap-5](aivcs://stevedores-org/aivcs/issues/239))
- `Cargo.toml` workspace dependency on `mom-core` (git rev pin) or HTTP client to mom-service
- Gateway audit events typed as `mcp.memory.*` ([#239 gap-4](aivcs://stevedores-org/aivcs/issues/239))

---

## 6. Open questions

These are intentionally unresolved — each should become a follow-up issue **after this proposal merges**:

1. ~~**Deployment topology:**~~ **Resolved — [ADR 001](../adr/001-memory-mom-deployment-topology.md):** dual-mode `MomBackend`; **HTTP sidecar (`mom-service`) in production**, **in-process embedded in local dev/CI unit tests**, selected via `MOM_BACKEND_URL`.

2. **Single vs dual SurrealDB:** mom uses namespace `mom/main`; oxidized-state uses its own namespace. Consolidate to one SurrealDB cluster with two namespaces, or enforce separate instances per tenant?

3. **Semantic merge boundary:** When `semantic-rag-merge` diffs memory vectors at CommitId merge time, does it read mom summaries, session vectors, or both? Need a normative precedence rule.

4. **Authorization matrix:** Which `memory::*` tools are `read` vs `write` vs `destructive` risk? Is `memory::summarize` a write (creates summary) requiring elevated scope?

5. **Embedding model governance:** mom-embeddings model ID is store-local today. Should aivcs CommitId metadata record which embedding model was active for reproducibility checks?

---

## Acceptance mapping (issue #248)

| Criterion | Addressed in |
|-----------|--------------|
| Tool list with schemas | §1 |
| Backend split | §2 |
| Identity model | §3 |
| Migration path | §4 (recommends **option b**) |
| Cross-repo impact | §5 |
| Open questions | §6 (follow-up issues filed post-merge) |

---

## References

- [AIVCS architecture](../architecture.md) — Layer 0 `MemoryRecord`, semantic merge flow
- [Dual ledger guide](../dual_ledger_guide.md) — CommitId linkage
- [MCP auth guide](../mcp-auth-guide.md) — gateway token claims and tool risk
- [mom-core `MemoryStore`](aivcs://lornu-ai/mom/blob/main/crates/mom-core/src/lib.rs) — kernel trait
- [mom HTTP routes](aivcs://lornu-ai/mom/blob/main/crates/mom-service/src/main.rs) — `/v1/memory`, `/v1/context-pack`, `/v1/task/*`
