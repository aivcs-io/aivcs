import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import Callout from "@/components/Callout";
import { Card, CardGrid } from "@/components/Card";

export default function Architecture() {
  return (
    <Layout>
      <h1 className="text-3xl font-bold tracking-tight mb-2">Architecture</h1>
      <p className="text-zinc-400 mb-8">Crate layers, dependency flow, and key abstractions.</p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Crate Layers</h2>

      <CodeBlock title="architecture">{`Layer 4  aivcs-cli          Clap CLI, command dispatch
Layer 3  semantic-rag-merge  Memory vector diff + heuristic conflict resolution
Layer 2  nix-env-manager     Nix Flake hashing + Attic binary cache
Layer 1  aivcs-core          Domain logic, CAS, recording, replay, parallel fork
Layer 0  oxidized-state      SurrealDB persistence (snapshots, commits, branches, runs)
         aivcsd              Daemon stub (placeholder)`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Dependency Flow</h2>

      <CodeBlock>{`aivcs-cli ──► aivcs-core ──► oxidized-state
                           ──► nix-env-manager
                           ──► semantic-rag-merge ──► oxidized-state`}</CodeBlock>

      <CardGrid>
        <Card to="/crates/core" title="aivcs-core" description="Layer 1 — Domain logic and orchestration" tag="Layer 1" />
        <Card to="/crates/state" title="oxidized-state" description="Layer 0 — SurrealDB persistence" tag="Layer 0" />
        <Card to="/crates/nix" title="nix-env-manager" description="Layer 2 — Nix/Attic integration" tag="Layer 2" />
        <Card to="/crates/merge" title="semantic-rag-merge" description="Layer 3 — Semantic merge logic" tag="Layer 3" />
      </CardGrid>

      <h2 className="text-xl font-semibold mt-8 mb-4">Key Abstractions</h2>

      <h3 className="text-lg font-medium mt-6 mb-3">CommitId (content-addressed)</h3>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        A composite SHA-256 hash derived from <code className="font-mono text-violet-300/90">logic_hash + state_digest + env_hash</code>.
        Ensures that identical agent state, source code, and environment always produce the same commit ID.
      </p>

      <h3 className="text-lg font-medium mt-6 mb-3">SurrealHandle (Layer 0)</h3>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Wraps a SurrealDB connection (in-memory or WebSocket). Provides CRUD for SnapshotRecord,
        CommitRecord, BranchRecord, GraphEdge, and MemoryRecord.
      </p>

      <h3 className="text-lg font-medium mt-6 mb-3">CasStore / FsCasStore (Layer 1)</h3>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Content-addressed store with SHA-256 digests. FsCasStore writes blobs to disk
        under <code className="font-mono text-violet-300/90">.aivcs/cas/</code>. Used during snapshot to deduplicate state.
      </p>

      <h3 className="text-lg font-medium mt-6 mb-3">ParallelManager (Layer 1)</h3>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Tracks branch scores and step counts during parallel exploration. <code className="font-mono text-violet-300/90">fork_agent_parallel</code> spawns
        N concurrent Tokio tasks that each create a branch + commit + snapshot from a parent commit.
      </p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Data Flow: Snapshot</h2>

      <CodeBlock title="aivcs snapshot --state state.json --branch main">{`CLI: aivcs snapshot --state state.json --branch main
  │
  ├─ Read state.json
  ├─ Detect git HEAD SHA
  ├─ Generate logic hash (Rust src) + env hash (flake.lock)
  ├─ CAS put → Digest
  ├─ CommitId::new(logic, state_digest, env)
  ├─ SurrealHandle::save_snapshot(commit_id, state)
  ├─ SurrealHandle::save_commit(record)
  ├─ SurrealHandle::save_commit_graph_edge(child, parent)
  └─ SurrealHandle::save_branch(updated head)`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Data Flow: Semantic Merge</h2>

      <CodeBlock title="aivcs merge feature --target main">{`CLI: aivcs merge feature --target main
  │
  ├─ Resolve branch heads → source_commit, target_commit
  ├─ semantic_merge(handle, source, target, msg, author)
  │   ├─ Load memories for both commits
  │   ├─ diff_memory_vectors → VectorStoreDelta
  │   ├─ resolve_conflict_state (heuristic: prefer longer content)
  │   ├─ synthesize_memory → merged memories
  │   ├─ Save merged snapshot + commit + graph edges
  │   └─ Return MergeResult
  └─ Update target branch head`}</CodeBlock>

      <Callout>
        All database operations go through <strong>Arc&lt;SurrealHandle&gt;</strong> for safe concurrent
        access across Tokio tasks. Tests use in-memory SurrealDB via <code className="font-mono text-violet-300/90">SurrealHandle::setup_db()</code>.
      </Callout>
    </Layout>
  );
}
