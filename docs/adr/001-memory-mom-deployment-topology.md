# ADR 001: Memory MCP — mom deployment topology

| Field | Value |
|-------|-------|
| **Status** | Accepted |
| **Date** | 2026-06-16 |
| **Issue** | [stevedores-org/aivcs#258](aivcs://stevedores-org/aivcs/issues/258) |
| **Proposal** | [mcp-memory-surface.md](../proposals/mcp-memory-surface.md) |
| **Parent** | [stevedores-org/aivcs#220](aivcs://stevedores-org/aivcs/issues/220) Phase 2.2 |

## Context

The unified `memory::*` MCP tool family ([proposal](../proposals/mcp-memory-surface.md)) delegates durable memory to **mom** (`mom-core`, `mom-store-surrealdb`) and CommitId-scoped session memory to **aivcs** (`oxidized-state`, `aivcs-core`).

Before implementing the `aivcs-memory` facade behind `aivcs-mcp-gateway`, we must choose how the gateway reaches mom:

1. **In-process** — link `mom-core` / `mom-store-surrealdb` directly inside the gateway binary.
2. **HTTP sidecar** — call an existing or co-deployed `mom-service` over HTTP (`/v1/memory`, `/v1/context-pack`, …).

Both options were listed as open question §6.1 in the memory surface proposal.

## Decision

**Use a dual-mode `MomBackend` with environment-selected implementation:**

| Profile | Backend | Selection |
|---------|---------|-----------|
| **Production / staging** | HTTP sidecar → `mom-service` | `MOM_BACKEND_URL` set (e.g. `http://mom:8080`) |
| **Local dev (default)** | In-process embedded | `MOM_BACKEND_URL` unset → embedded `MemoryStore` |
| **CI unit tests** | In-process (in-memory SurrealDB) | Test harness constructs embedded backend directly |
| **CI integration (future)** | HTTP sidecar | Optional matrix job with `MOM_BACKEND_URL` |

**Production default is HTTP sidecar.** Local dev default is in-process embedded. The facade API is identical in both modes.

### Facade shape (implementation follow-up)

```text
aivcs-mcp-gateway
    └── aivcs-memory (new crate)
            trait MomBackend { async fn put(...); async fn query(...); ... }
            ├── EmbeddedMomBackend   → mom-core + mom-store-surrealdb (in-process)
            └── HttpMomBackend       → reqwest client to mom-service REST routes
```

Session memory (`oxidized-state`) stays **in-process inside aivcs** in all profiles — only the mom leg is dual-mode.

### Configuration

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `MOM_BACKEND_URL` | No | *(unset)* | Base URL of `mom-service`. Unset → embedded in-process backend. |
| `MOM_BACKEND_TIMEOUT_MS` | No | `30000` | HTTP client timeout for sidecar mode. |
| `SURREALDB_ENDPOINT` | Prod | — | mom-service uses this for durable store (unchanged from mom runbooks). aivcs session memory uses oxidized-state config separately ([#259](aivcs://stevedores-org/aivcs/issues/259)). |

### Gateway ↔ mom auth (sidecar mode)

- **v1:** Internal network trust — gateway runs in same K8s namespace / NixOS slice as mom; no public mom ingress.
- **v1.1 (follow-up):** Service JWT or mTLS between gateway and mom (`Authorization: Bearer <internal>`), minted by `aivcs-auth` with `aud=mom-internal`. Tracked under [#239](aivcs://stevedores-org/aivcs/issues/239) hardening gaps.

Tenant isolation remains enforced by mom's `ScopeKey` on every store call; the gateway forwards scope from MCP token claims ([proposal §3](../proposals/mcp-memory-surface.md)).

## Rationale

### Why not in-process only?

- Couples gateway releases to `mom-core` semver and SurrealDB schema migrations.
- Forces mom embeddings + hybrid search into the gateway process memory footprint.
- Conflicts with the migration plan ([proposal §4](../proposals/mcp-memory-surface.md)): mom HTTP becomes a thin shim — production already assumes a distinct mom service boundary.

### Why not HTTP sidecar only?

- Local dev would require `docker compose` or a second terminal for every gateway session.
- CI unit tests would need network mocks for all gateway memory tests.
- `cargo run -p aivcs-mcp-gateway` should work out of the box for agent tool development.

### Why dual-mode wins

- **Production** gets independent deploy, scale, and blast-radius isolation.
- **Dev/CI** gets single-binary ergonomics and fast in-memory tests.
- **Migration T2** is natural: mom HTTP shim and gateway HTTP client share the same wire format.
- Matches the proposal's sketch: `MomBackend::connect(endpoint | in_process)`.

## Latency and isolation tradeoffs

| Dimension | In-process (embedded) | HTTP sidecar |
|-----------|----------------------|--------------|
| **Typical query latency** | Sub-ms trait call + store | +1–5 ms same-host; +20–50 ms cross-AZ |
| **Dominant cost** | Embedding + hybrid search (10–200 ms) | Same — network hop is negligible vs embed/search |
| **Process isolation** | Shared fate with gateway | mom crashes don't take down gateway |
| **Memory footprint** | Combined (gateway + embedder cache) | Split across pods |
| **Tenant isolation** | Rust type system + `ScopeKey` in-process | HTTP + `ScopeKey` + network policy |
| **Nix packaging** | Single derivation (`aivcs-mcp-gateway` with mom deps) | Two derivations + compose module |
| **Version coupling** | Tight (`mom-core` git rev in `Cargo.toml`) | Loose (HTTP contract + semver) |

**Conclusion:** Latency difference is acceptable for agent memory workloads. Isolation and deploy independence favor HTTP in production; developer velocity favors embedded locally.

## Nix / deploy topology

```text
Production (NixOS / K8s):
  aivcs-mcp-gateway:8082  ──HTTP──►  mom-service:8080  ──►  SurrealDB (mom/main)
        │
        └── oxidized-state (aivcs namespace) ──►  SurrealDB (aivcs)   [#259 TBD]

Local dev:
  cargo run -p aivcs-mcp-gateway     # MOM_BACKEND_URL unset → embedded mom Mem backend
  cargo run -p aivcs-mcp-gateway       # optional: MOM_BACKEND_URL=http://127.0.0.1:8080
  cargo run -p mom-service             # when testing sidecar mode against real HTTP
```

Nix flake changes (implementation follow-up, not this ADR):

- `packages.aivcs-mcp-gateway` — gateway binary; optional `mom-core` dependency for embedded mode.
- `packages.mom-service` — re-export or wrap lornu-ai/mom flake when pinned.
- `services.aivcs-mcp-gateway` — sets `MOM_BACKEND_URL` to internal mom service URL in prod.

## Consequences

### Positive

- Unblocks `memory::*` gateway implementation with a clear backend trait.
- Local `cargo run -p aivcs-mcp-gateway` works without sidecar orchestration.
- Production aligns with mom HTTP shim migration path.

### Negative

- Two code paths to test (embedded + HTTP client).
- Embedded mode adds `mom-core` / `mom-store-surrealdb` as optional/heavy deps on gateway build.
- HTTP contract drift must be caught by integration tests or shared OpenAPI spec (future).

### Follow-up issues

| Issue | Topic |
|-------|-------|
| [#259](aivcs://stevedores-org/aivcs/issues/259) | SurrealDB cluster topology (mom vs aivcs namespaces) |
| [#239](aivcs://stevedores-org/aivcs/issues/239) | Gateway ↔ mom service auth |
| *TBD* | Implement `aivcs-memory` crate + `MomBackend` trait |
| [lornu-ai/mom#84](aivcs://lornu-ai/mom/issues/84) | mom `MomBackend::connect` factory export |

## Alternatives considered

1. **In-process only** — rejected for production isolation and migration fit (see above).
2. **HTTP sidecar only** — rejected for local dev / CI ergonomics.
3. **gRPC instead of HTTP** — rejected; mom already exposes REST; adding gRPC duplicates the shim surface under migration option (b).
4. **Shared SurrealDB, no mom-service** — rejected pending [#259]; even with shared DB, production still benefits from mom as a distinct service for ingest/sources.
