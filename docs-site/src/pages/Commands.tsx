import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";

export default function Commands() {
  return (
    <Layout>
      <h1 className="text-3xl font-bold tracking-tight mb-2">Commands</h1>
      <p className="text-zinc-400 mb-8">Complete reference for all AIVCS CLI commands.</p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Core Commands</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Command</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">init</td>
            <td className="py-3">Initialize a new AIVCS repository</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">snapshot</td>
            <td className="py-3">Create a versioned checkpoint of agent state</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">restore</td>
            <td className="py-3">Restore agent to a previous state</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">branch</td>
            <td className="py-3">Manage branches (list, create, delete)</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">log</td>
            <td className="py-3">Show commit history</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">merge</td>
            <td className="py-3">Merge two branches with semantic resolution</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">diff</td>
            <td className="py-3">Show differences between commits or branches</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">replay</td>
            <td className="py-3">Replay a recorded run artifact by run ID</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">diff-runs</td>
            <td className="py-3">Diff the tool-call sequences of two runs</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Environment Commands</h2>

      <CodeBlock title="terminal">{`# Show environment hash for a Nix Flake
aivcs env hash /path/to/flake

# Show logic hash (Rust source code)
aivcs env logic-hash src/

# Check Attic cache status
aivcs env cache-info

# Check if environment is cached
aivcs env is-cached <hash>

# Show system info (Nix/Attic availability)
aivcs env info`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Parallel Simulation Commands</h2>

      <CodeBlock title="terminal">{`# Fork 5 parallel branches from main for exploration
aivcs fork main --count 5 --prefix experiment

# Fork 3 branches from a specific commit
aivcs fork abc123 -c 3 -p strategy

# Show reasoning trace (time-travel debugging)
aivcs trace main

# Show trace with more depth
aivcs trace experiment-0 --depth 50`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Common Workflows</h2>

      <h3 className="text-lg font-medium mt-6 mb-3">Snapshot and restore</h3>
      <CodeBlock>{`aivcs init
echo '{"step": 1}' > state.json
aivcs snapshot --state state.json --message "Step 1"
aivcs log
aivcs restore <commit-id> --output restored.json`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">Branch, explore, and merge</h3>
      <CodeBlock>{`aivcs branch create experiment
aivcs snapshot --state state.json --message "Try approach A" --branch experiment
aivcs merge experiment --target main`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">Parallel exploration</h3>
      <CodeBlock>{`aivcs fork main --count 5 --prefix strategy
# Each strategy-N branch explores independently
aivcs trace strategy-0
aivcs merge strategy-2 --target main`}</CodeBlock>
    </Layout>
  );
}
