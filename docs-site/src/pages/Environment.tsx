import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import Callout from "@/components/Callout";

export default function Environment() {
  return (
    <Layout>
      <h1 className="text-3xl font-bold tracking-tight mb-2">Environment</h1>
      <p className="text-zinc-400 mb-8">Nix Flakes integration and Attic binary cache for reproducible agent environments.</p>

      <h2 className="text-xl font-semibold mt-8 mb-4">Overview</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        The <code className="font-mono text-violet-300/90">nix-env-manager</code> crate (Layer 2) provides hermetic environment
        versioning. It generates deterministic hashes from Nix Flake lockfiles and Rust source code,
        ensuring that agent environments can be reproduced exactly.
      </p>

      <Callout>
        Nix and Attic are <strong>optional</strong> dependencies. AIVCS works without them &mdash; environment
        hashing simply returns placeholder values when Nix is not available.
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">Environment Hashing</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Two types of hashes are computed for each commit:
      </p>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Hash Type</th>
            <th className="py-3 pr-4 font-medium">Source</th>
            <th className="py-3 font-medium">Purpose</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">env_hash</td>
            <td className="py-3 pr-4">flake.lock</td>
            <td className="py-3">Captures exact Nix dependency tree</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">logic_hash</td>
            <td className="py-3 pr-4">Rust source files</td>
            <td className="py-3">Captures agent logic/code changes</td>
          </tr>
        </tbody>
      </table>

      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Both hashes feed into the <code className="font-mono text-violet-300/90">CommitId</code> computation
        alongside the state digest, creating fully content-addressed commits.
      </p>

      <CodeBlock title="terminal">{`# Show environment hash for a Nix Flake
aivcs env hash /path/to/flake

# Show logic hash (Rust source code)
aivcs env logic-hash src/`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Attic Binary Cache</h2>
      <p className="text-[13px] text-zinc-400 mb-4 leading-relaxed">
        Attic provides a self-hosted Nix binary cache with deduplication. AIVCS integrates with
        Attic to check whether an environment closure is already cached, avoiding redundant builds.
      </p>

      <CodeBlock title="terminal">{`# Check Attic cache status
aivcs env cache-info

# Check if an environment hash is cached
aivcs env is-cached <hash>

# Show system info (Nix/Attic availability)
aivcs env info`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Configuration</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Variable</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_SERVER</td>
            <td className="py-3">Attic binary cache server URL</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_CACHE</td>
            <td className="py-3">Attic cache name</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">ATTIC_TOKEN</td>
            <td className="py-3">Attic authentication token</td>
          </tr>
        </tbody>
      </table>

      <CodeBlock title=".env">{`ATTIC_SERVER=https://cache.example.com
ATTIC_CACHE=aivcs-envs
ATTIC_TOKEN=your-token-here`}</CodeBlock>
    </Layout>
  );
}
