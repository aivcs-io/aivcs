import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import StatusBadge from "@/components/StatusBadge";
import Callout from "@/components/Callout";

export default function CrateMerge() {
  return (
    <Layout>
      <div className="flex items-center gap-3 mb-2">
        <h1 className="text-3xl font-bold tracking-tight">semantic-rag-merge</h1>
        <StatusBadge status="done" />
      </div>
      <p className="text-zinc-400 mb-2">Semantic merging with RAG and LLM Arbiter for AIVCS.</p>
      <div className="flex gap-3 text-[12px] text-zinc-500 mb-8">
        <a href="aivcs://stevedores-org/aivcs/tree/main/crates/semantic-rag-merge" className="hover:text-violet-400 transition">GitHub</a>
        <span className="text-zinc-700">&middot;</span>
        <span>Layer 3</span>
      </div>

      <Callout>
        This crate provides the merge intelligence. Rather than text-based diffing, it operates
        on agent memory vectors and uses heuristic conflict resolution (currently: prefer longer content).
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">Merge Pipeline</h2>

      <ol className="list-decimal list-inside text-[13px] text-zinc-400 space-y-2 mb-6 ml-2">
        <li>Load <code className="font-mono text-violet-300/90">MemoryRecord</code> vectors for source and target commits</li>
        <li>Compute <code className="font-mono text-violet-300/90">VectorStoreDelta</code> &mdash; additions, deletions, and conflicts</li>
        <li>Run conflict arbiter (heuristic: prefer longer/richer content)</li>
        <li>Synthesize merged memories into a new unified set</li>
        <li>Persist merged snapshot, commit, and graph edges</li>
      </ol>

      <CodeBlock title="merge flow">{`semantic_merge(handle, source, target, msg, author)
  ├─ Load memories for both commits
  ├─ diff_memory_vectors → VectorStoreDelta
  ├─ resolve_conflict_state (heuristic: prefer longer content)
  ├─ synthesize_memory → merged memories
  ├─ Save merged snapshot + commit + graph edges
  └─ Return MergeResult`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Key Types</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Type</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">VectorStoreDelta</td>
            <td className="py-3">Diff result: additions, deletions, and conflicting memory keys</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ConflictState</td>
            <td className="py-3">Represents an unresolved conflict between two memory values</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">MergeResult</td>
            <td className="py-3">Output of a merge: new commit ID + merged memory summary</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Dependencies</h2>

      <CodeBlock title="Cargo.toml">{`[dependencies]
oxidized-state.workspace = true
tokio.workspace = true
async-trait.workspace = true
serde.workspace = true
uuid.workspace = true`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Conflict Resolution Strategy</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        The current heuristic arbiter prefers <strong>longer content</strong> when two memory entries
        conflict. This is a simple but effective strategy for agent memories where more detailed
        content typically represents more learned information. Future versions may integrate
        LLM-based arbitration for richer semantic understanding.
      </p>
    </Layout>
  );
}
