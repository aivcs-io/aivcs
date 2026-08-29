import Layout from "@/components/Layout";
import { Card, CardGrid } from "@/components/Card";
import CodeBlock from "@/components/CodeBlock";
import Callout from "@/components/Callout";

export default function Home() {
  return (
    <Layout>
      <div className="mb-10">
        <h1 className="text-4xl font-bold tracking-tight mb-3">
          <span className="bg-gradient-to-r from-violet-400 to-fuchsia-400 bg-clip-text text-transparent">
            AIVCS
          </span>
        </h1>
        <p className="text-lg text-zinc-400 leading-relaxed max-w-2xl">
          AI Agent Version Control System &mdash; Git-like state commits, branching, and semantic
          merging for AI agent workflows. Built in Rust with SurrealDB.
        </p>
      </div>

      <div className="flex gap-3 mb-10">
        <a
          href="/aivcs/getting-started"
          className="px-4 py-2 bg-violet-600 hover:bg-violet-500 text-white text-sm font-medium rounded-lg transition"
        >
          Get Started
        </a>
        <a
          href="aivcs://stevedores-org/aivcs"
          className="px-4 py-2 bg-zinc-800 hover:bg-zinc-700 text-zinc-200 text-sm font-medium rounded-lg border border-zinc-700 transition"
        >
          GitHub
        </a>
      </div>

      <Callout icon="&#x2693;">
        AIVCS implements <strong>AgentGit 2.0</strong> &mdash; bringing Git-like version control
        primitives to AI agent execution. Snapshot state, fork parallel strategies, merge with
        semantic conflict resolution, and time-travel debug through agent reasoning.
      </Callout>

      <h2 className="text-xl font-semibold mt-10 mb-4">Features</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Feature</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">State Commits</td>
            <td className="py-3">Content-addressed snapshots with SHA-256 deduplication</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Branching</td>
            <td className="py-3">Parallel exploration paths for different agent strategies</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Semantic Merge</td>
            <td className="py-3">LLM-assisted conflict resolution for agent memories</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Time-Travel</td>
            <td className="py-3">Trace agent reasoning through commit history</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Environment Lock</td>
            <td className="py-3">Nix Flake hashing + Attic binary cache integration</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Parallel Fork</td>
            <td className="py-3">Concurrent branch forking with Tokio for exploration</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-10 mb-4">Quick Start</h2>

      <CodeBlock title="terminal">{`# Build
cargo build --release

# Initialize repository
aivcs init

# Create a state snapshot
echo '{"step": 1, "memory": "learned X"}' > state.json
aivcs snapshot --state state.json --message "Initial state"

# View history
aivcs log

# Branch and merge
aivcs branch create experiment-1
aivcs merge experiment-1 --target main`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-10 mb-4">Workspace Crates</h2>

      <CardGrid>
        <Card
          to="/crates/core"
          title="aivcs-core"
          description="Domain logic, CAS, recording, replay, and parallel fork orchestration."
          tag="Layer 1"
        />
        <Card
          to="/crates/state"
          title="oxidized-state"
          description="SurrealDB persistence for snapshots, commits, branches, and runs."
          tag="Layer 0"
        />
        <Card
          to="/crates/nix"
          title="nix-env-manager"
          description="Nix Flakes hashing and Attic binary cache integration."
          tag="Layer 2"
        />
        <Card
          to="/crates/merge"
          title="semantic-rag-merge"
          description="Memory vector diffing and semantic merge with heuristic conflict resolution."
          tag="Layer 3"
        />
      </CardGrid>
    </Layout>
  );
}
