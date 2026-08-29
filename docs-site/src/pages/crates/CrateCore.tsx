import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import StatusBadge from "@/components/StatusBadge";
import Callout from "@/components/Callout";

export default function CrateCore() {
  return (
    <Layout>
      <div className="flex items-center gap-3 mb-2">
        <h1 className="text-3xl font-bold tracking-tight">aivcs-core</h1>
        <StatusBadge status="done" />
      </div>
      <p className="text-zinc-400 mb-2">Core library for AIVCS domain logic and orchestration.</p>
      <div className="flex gap-3 text-[12px] text-zinc-500 mb-8">
        <a href="aivcs://stevedores-org/aivcs/tree/main/crates/aivcs-core" className="hover:text-violet-400 transition">GitHub</a>
        <span className="text-zinc-700">&middot;</span>
        <span>Layer 1</span>
      </div>

      <Callout>
        This is the main orchestration layer. It depends on all other crates and provides the
        domain types, CAS store, recording engine, and parallel fork manager.
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">Key Modules</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Module</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">cas/</td>
            <td className="py-3">Content-addressed store with SHA-256 digests (FsCasStore)</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">git/</td>
            <td className="py-3">Git HEAD capture for linking commits to source revisions</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">domain/</td>
            <td className="py-3">Business types: AgentSpec, Run, Release, Event</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">parallel/</td>
            <td className="py-3">Concurrent branch forking with ParallelManager</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">recording/</td>
            <td className="py-3">Execution ledger (RunLedger trait, GraphRunRecorder)</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">diff/</td>
            <td className="py-3">Tool-call sequence diffing between runs</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Dependencies</h2>

      <CodeBlock title="Cargo.toml">{`[dependencies]
oxidized-state.workspace = true
oxidizedgraph.workspace = true
nix-env-manager.workspace = true
semantic-rag-merge.workspace = true
tokio.workspace = true
async-trait.workspace = true
serde.workspace = true
sha2.workspace = true`}</CodeBlock>

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
            <td className="py-3 pr-4 font-mono text-violet-400">CommitId</td>
            <td className="py-3">Content-addressed commit hash (logic + state + env)</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">CasStore / FsCasStore</td>
            <td className="py-3">Content-addressed blob storage with SHA-256 keys</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">GraphRunRecorder</td>
            <td className="py-3">Maps domain events to RunEvent entries for persistence</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ParallelManager</td>
            <td className="py-3">Manages concurrent branch forking and scoring</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ReleaseRegistry</td>
            <td className="py-3">Per-agent release history (promote, rollback, current)</td>
          </tr>
        </tbody>
      </table>
    </Layout>
  );
}
