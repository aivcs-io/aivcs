import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import Callout from "@/components/Callout";

export default function Branching() {
  return (
    <Layout>
      <h1 className="text-3xl font-bold tracking-tight mb-2">Branching & Merging</h1>
      <p className="text-zinc-400 mb-8">Parallel exploration, semantic merge, and fork strategies.</p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Branching Model</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        AIVCS branches work like Git branches &mdash; named pointers to commit hashes stored
        in SurrealDB. Each branch tracks a HEAD commit, and new snapshots advance the HEAD.
      </p>

      <CodeBlock title="terminal">{`# Create a branch
aivcs branch create experiment-1

# List branches
aivcs branch list

# Delete a branch
aivcs branch delete experiment-1`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Parallel Forking</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        The <code className="font-mono text-violet-300/90">fork</code> command spawns N concurrent Tokio tasks,
        each creating a branch + commit + snapshot from a parent. This enables parallel exploration
        of different agent strategies from the same starting point.
      </p>

      <CodeBlock title="terminal">{`# Fork 5 branches from main
aivcs fork main --count 5 --prefix experiment

# Creates: experiment-0, experiment-1, ..., experiment-4
# Each branch starts from main's current HEAD`}</CodeBlock>

      <Callout>
        Forking is fully concurrent &mdash; all branches are created in parallel using Tokio tasks.
        The <strong>ParallelManager</strong> tracks branch scores and step counts for later
        comparison and pruning.
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">Semantic Merge</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Unlike Git&rsquo;s text-based merge, AIVCS performs <strong>semantic merging</strong> on agent
        memory vectors. The merge pipeline:
      </p>

      <ol className="list-decimal list-inside text-[13px] text-zinc-400 space-y-2 mb-6 ml-2">
        <li>Load memories for both source and target commits</li>
        <li>Compute <code className="font-mono text-violet-300/90">VectorStoreDelta</code> via memory vector diffing</li>
        <li>Resolve conflicts with a heuristic arbiter (prefers longer content)</li>
        <li>Synthesize merged memories into a new snapshot</li>
        <li>Create merge commit with two parent edges</li>
      </ol>

      <CodeBlock title="terminal">{`# Merge experiment branch into main
aivcs merge experiment-1 --target main`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Time-Travel Debugging</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        The <code className="font-mono text-violet-300/90">trace</code> command walks the commit graph backwards,
        showing the reasoning trail of an agent branch. Combined with <code className="font-mono text-violet-300/90">diff</code>,
        this enables full time-travel debugging of agent decisions.
      </p>

      <CodeBlock title="terminal">{`# Show reasoning trace for a branch
aivcs trace experiment-0

# Increase depth for longer histories
aivcs trace experiment-0 --depth 50

# Diff two commits
aivcs diff <commit-a> <commit-b>`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Run Recording & Replay</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Every agent execution is recorded as a sequence of RunEvent entries (tool calls, state
        transitions, decisions). These can be replayed deterministically or diffed across runs.
      </p>

      <CodeBlock title="terminal">{`# Replay a recorded run
aivcs replay <run-id>

# Diff tool-call sequences between two runs
aivcs diff-runs <run-a> <run-b>`}</CodeBlock>
    </Layout>
  );
}
