import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import StatusBadge from "@/components/StatusBadge";
import Callout from "@/components/Callout";

export default function CrateState() {
  return (
    <Layout>
      <div className="flex items-center gap-3 mb-2">
        <h1 className="text-3xl font-bold tracking-tight">oxidized-state</h1>
        <StatusBadge status="done" />
      </div>
      <p className="text-zinc-400 mb-2">SurrealDB backend for AIVCS state persistence.</p>
      <div className="flex gap-3 text-[12px] text-zinc-500 mb-8">
        <a href="aivcs://stevedores-org/aivcs/tree/main/crates/oxidized-state" className="hover:text-violet-400 transition">GitHub</a>
        <span className="text-zinc-700">&middot;</span>
        <span>Layer 0</span>
      </div>

      <Callout>
        This is the persistence foundation. All other crates depend on oxidized-state for
        reading and writing commits, snapshots, branches, and graph edges.
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">SurrealHandle</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        The central abstraction &mdash; wraps a SurrealDB connection (in-memory or WebSocket) behind
        an <code className="font-mono text-violet-300/90">Arc&lt;SurrealHandle&gt;</code> for safe concurrent access across Tokio tasks.
      </p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Record Types</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Record</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SnapshotRecord</td>
            <td className="py-3">JSON state blobs keyed by commit hash</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">CommitRecord</td>
            <td className="py-3">Commit metadata (parents, message, author, timestamp)</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">BranchRecord</td>
            <td className="py-3">Named pointers to commit hashes</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">GraphEdge</td>
            <td className="py-3">Parent-child edges for history traversal</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">MemoryRecord</td>
            <td className="py-3">Key-value memory vectors for semantic merge</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">RunRecord</td>
            <td className="py-3">Execution run metadata and event sequences</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Connection Modes</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Mode</th>
            <th className="py-3 pr-4 font-medium">Config</th>
            <th className="py-3 font-medium">Use Case</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">In-memory</td>
            <td className="py-3 pr-4 text-zinc-500 italic">default</td>
            <td className="py-3">Local development and tests</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">WebSocket</td>
            <td className="py-3 pr-4">SURREALDB_ENDPOINT</td>
            <td className="py-3">SurrealDB Cloud (production)</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Error Handling</h2>

      <CodeBlock title="StateError">{`pub enum StateError {
    ConnectionFailed(String),
    RecordNotFound(String),
    SerializationError(String),
    QueryFailed(String),
}`}</CodeBlock>
    </Layout>
  );
}
