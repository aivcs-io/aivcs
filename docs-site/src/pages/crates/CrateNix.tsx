import Layout from "@/components/Layout";
import CodeBlock from "@/components/CodeBlock";
import StatusBadge from "@/components/StatusBadge";
import Callout from "@/components/Callout";

export default function CrateNix() {
  return (
    <Layout>
      <div className="flex items-center gap-3 mb-2">
        <h1 className="text-3xl font-bold tracking-tight">nix-env-manager</h1>
        <StatusBadge status="done" />
      </div>
      <p className="text-zinc-400 mb-2">Nix Flakes and Attic cache integration for AIVCS.</p>
      <div className="flex gap-3 text-[12px] text-zinc-500 mb-8">
        <a href="aivcs://stevedores-org/aivcs/tree/main/crates/nix-env-manager" className="hover:text-violet-400 transition">GitHub</a>
        <span className="text-zinc-700">&middot;</span>
        <span>Layer 2</span>
      </div>

      <Callout>
        Nix and Attic are optional. When unavailable, hashing functions return deterministic
        placeholder values so the rest of AIVCS continues to work.
      </Callout>

      <h2 className="text-xl font-semibold mt-8 mb-4">Capabilities</h2>

      <table className="w-full text-[13px] mb-8">
        <thead>
          <tr className="border-b border-zinc-800 text-left text-zinc-500">
            <th className="py-3 pr-4 font-medium">Feature</th>
            <th className="py-3 font-medium">Description</th>
          </tr>
        </thead>
        <tbody className="text-zinc-300">
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Flake hashing</td>
            <td className="py-3">SHA-256 hash of flake.lock for reproducible dependency trees</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Logic hashing</td>
            <td className="py-3">SHA-256 hash of Rust source files to detect code changes</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">Cache lookup</td>
            <td className="py-3">Check if an environment closure exists in Attic cache</td>
          </tr>
          <tr className="border-b border-zinc-800/60">
            <td className="py-3 pr-4 font-mono text-violet-400">System info</td>
            <td className="py-3">Detect Nix/Attic availability on the host system</td>
          </tr>
        </tbody>
      </table>

      <h2 className="text-xl font-semibold mt-8 mb-4">How Hashes Feed Into CommitId</h2>

      <CodeBlock>{`CommitId = SHA-256(logic_hash + state_digest + env_hash)

  logic_hash  ← nix-env-manager: hash of Rust source files
  state_digest ← aivcs-core CAS: hash of agent state JSON
  env_hash    ← nix-env-manager: hash of flake.lock`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Dependencies</h2>

      <CodeBlock title="Cargo.toml">{`[dependencies]
tokio.workspace = true
async-trait.workspace = true
serde.workspace = true
sha2.workspace = true
hex.workspace = true
reqwest.workspace = true   # HTTP client for Attic API`}</CodeBlock>

      <h2 className="text-xl font-semibold mt-8 mb-4">Error Handling</h2>

      <CodeBlock title="NixError">{`pub enum NixError {
    NixNotInstalled,
    FlakeLockNotFound(String),
    HashComputationFailed(String),
    AtticUnavailable(String),
    CacheLookupFailed(String),
}`}</CodeBlock>
    </Layout>
  );
}
