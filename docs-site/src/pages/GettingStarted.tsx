import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import Callout from "@/components/Callout";

export default function GettingStarted() {
  return (
    <Layout>
      <h1 className="text-3xl font-bold tracking-tight mb-2">Getting Started</h1>
      <p className="text-zinc-400 mb-8">Prerequisites, installation, and your first AIVCS session.</p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Prerequisites</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Requirement</th>
            <th className="py-3 pr-4 font-medium">Version</th>
            <th className="py-3 font-medium">Notes</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Rust</td>
            <td className="py-3 pr-4">stable (1.75+)</td>
            <td className="py-3">Install via rustup</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Git</td>
            <td className="py-3 pr-4">2.x</td>
            <td className="py-3">For SHA linking and repo detection</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Nix</td>
            <td className="py-3 pr-4">2.18+</td>
            <td className="py-3">Optional &mdash; only for env commands</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Attic</td>
            <td className="py-3 pr-4">latest</td>
            <td className="py-3">Optional &mdash; only for binary cache</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">Installation</h2>

      <CodeBlock title="terminal">{`git clone aivcs://stevedores-org/aivcs.git
cd aivcs
cargo build --release
./target/release/aivcs --version`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">First-Run Walkthrough</h2>

      <h3 className="text-lg font-medium mt-6 mb-3">1. Initialise a repository</h3>
      <CodeBlock>{`aivcs init`}</CodeBlock>
      <p className="text-[13px] text-zinc-400 mb-4">
        Creates an initial commit and a <code className="font-mono text-violet-300/90">main</code> branch in the backing SurrealDB store.
      </p>

      <h3 className="text-lg font-medium mt-6 mb-3">2. Create a state snapshot</h3>
      <CodeBlock>{`echo '{"step": 1, "memory": "learned X"}' > state.json
aivcs snapshot --state state.json --message "First snapshot"`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">3. View history</h3>
      <CodeBlock>{`aivcs log`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">4. Branch and explore</h3>
      <CodeBlock>{`aivcs branch create experiment-1
aivcs snapshot --state state.json --message "Experiment" --branch experiment-1`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">5. Merge back</h3>
      <CodeBlock>{`aivcs merge experiment-1 --target main`}</CodeBlock>

      <h3 className="text-lg font-medium mt-6 mb-3">6. Restore a previous state</h3>
      <CodeBlock>{`aivcs restore <commit-id> --output restored.json`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Environment Variables</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Variable</th>
            <th className="py-3 pr-4 font-medium">Default</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SURREALDB_ENDPOINT</td>
            <td className="py-3 pr-4 text-zinc-500 italic">in-memory</td>
            <td className="py-3">WebSocket URL for SurrealDB Cloud</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SURREALDB_USERNAME</td>
            <td className="py-3 pr-4 text-zinc-500">&mdash;</td>
            <td className="py-3">Database user</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SURREALDB_PASSWORD</td>
            <td className="py-3 pr-4 text-zinc-500">&mdash;</td>
            <td className="py-3">Database password</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SURREALDB_NAMESPACE</td>
            <td className="py-3 pr-4">aivcs</td>
            <td className="py-3">SurrealDB namespace</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">SURREALDB_DATABASE</td>
            <td className="py-3 pr-4">main</td>
            <td className="py-3">SurrealDB database name</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_SERVER</td>
            <td className="py-3 pr-4 text-zinc-500">&mdash;</td>
            <td className="py-3">Attic binary cache server URL</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_CACHE</td>
            <td className="py-3 pr-4 text-zinc-500">&mdash;</td>
            <td className="py-3">Attic cache name</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_TOKEN</td>
            <td className="py-3 pr-4 text-zinc-500">&mdash;</td>
            <td className="py-3">Attic authentication token</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">RUST_LOG</td>
            <td className="py-3 pr-4">info</td>
            <td className="py-3">Tracing filter (e.g. debug, aivcs_core=trace)</td>
          </tr>
        </tbody>
      </table>

      <Callout>
        By default AIVCS uses an in-memory SurrealDB instance &mdash; no external database needed
        for local development. Set <code className="font-mono text-violet-300/90">SURREALDB_ENDPOINT</code> to
        connect to SurrealDB Cloud for production use.
      </Callout>
    </Layout>
  );
}
