# ACPX Protocols for AIVCS

Date: 2026-06-19
Status: initial research kickoff

## Working Definition

No public standard named "ACPX Protocols" was found in current public documentation. For AIVCS, treat ACPX as the protocol-exchange layer around the Agent Client Protocol plus adjacent agent interoperability protocols.

For this research track, use "ACPX" as a shorthand for an agent protocol exchange layer around AIVCS:

- ACP: Agent Client Protocol. It provides headless, deterministic routing between coding clients and agents, and helps prevent context rot in long-running engineering tasks through explicit sessions, resumability, prompt turns, and structured updates.
- A2A: Cross-agent task delegation, discovery, messages, artifacts, and long-running task state.
- MCP: Model-to-tool/data/resource integration for agents and IDEs.
- ANP/AGNTCY: cross-domain agent identity, discovery, directory, secure messaging, and observability.

Implementation rule: AIVCS protocol work should be Rust-first. Do not build Python or shell glue into the architecture; use Rust crates, binaries, services, and conformance tests.

## Local AIVCS Context

AIVCS is already positioned as version orchestration in the Brains architecture: branch, snapshot, rollback, semantic merge, and replay. Brains has a hard invariant that autonomous mutation requires an AIVCS snapshot, isolated worktree, validation result, policy decision, rollback plan, and evidence graph.

Relevant local surfaces:

- `brains/crates/brains-aivcs/src/lib.rs` defines `SnapshotPhase` and `AivcsSnapshotRef`.
- `brains/docs/ARCHITECTURE.md` places AIVCS as the version-orchestration substrate.
- `brains/docs/RESOURCE_CATALOG.md` lists AIVCS MCP tools and planned auth/gateway work.
- `oxidizedgraph/src/a2a/types.rs` contains a local A2A JSON-RPC subset with Agent Card, task state, messages, and task methods.
- `bullpen/agents/a2a-protocol.md` and `bullpen/ai-agents/ai-agent-core/src/ai_agent_core/protocols/a2a.py` contain older local A2A-style schemas.
- `bullpen/ai-agents/ai-agent-core/src/ai_agent_core/protocols/mcp.py` contains an MCP hub state-sync adapter.

## External Landscape

| Protocol | Primary role | Fit for AIVCS | Caveat |
| --- | --- | --- | --- |
| ACP | Standard client-agent protocol for code editors and coding agents. Uses JSON-RPC, sessions, prompt turns, session updates, tool-call permission requests, filesystem/terminal capability negotiation, and MCP-friendly types. | Primary headless routing layer for AIVCS-driven engineering sessions: create/load/resume sessions, stream plans and diffs, route tool permissions, and bind every prompt turn to AIVCS snapshots and evidence. | ACP is client-to-agent, not a full cross-agent trust fabric. AIVCS still needs policy gates, provenance, rollback refs, and stable storage for long-running work. |
| MCP | Standard tool, prompt, and resource access between LLM apps and external systems. Uses JSON-RPC 2.0 and host/client/server capability negotiation. | Expose AIVCS as tools/resources: snapshot, restore, fork, trace, semantic merge, replay, PR pipeline, reasoning trace. | MCP auth is optional; high-impact tools still need local policy gates, consent, least privilege, and audit. |
| A2A | Agent-to-agent protocol for discovery, task submission, task lifecycle, messages, artifacts, streaming, and push notifications. | Make AIVCS a "version-orchestrator" agent with skills for snapshotting, replay, rollback, branch/fork, semantic merge, and PR orchestration. | Agent Cards are capability claims; clients should verify signatures and bind decisions to policy/evidence. |
| ANP | Agentic Web protocol stack for decentralized identity, description, discovery, messaging, attachments, federation, and payments. | Longer-range option for cross-organization AIVCS federation and verifiable agent identities. | Broader than immediate AIVCS needs; adopt later only if cross-domain discovery/trust becomes a requirement. |
| AGNTCY | Linux Foundation-aligned Internet-of-Agents stack: OASF records, directory, identity, SLIM messaging, observability/eval. | Candidate registry/identity layer for AIVCS agents and MCP servers, especially in multi-org or multi-cloud environments. | Needs mapping to AIVCS provenance and existing data-fabric identity/policy surfaces. |

## Initial Design Hypothesis

AIVCS should not choose one protocol as its internal model. It should become the durable protocol provenance ledger behind multiple protocol front doors:

1. ACP front door for deterministic client-agent sessions, headless routing, session replay/resume, structured updates, and long-running engineering task continuity.
2. MCP front door for tool and resource access.
3. A2A front door for cross-agent task delegation where peer agents need to exchange work.
4. ANP/AGNTCY identity and directory integration later for cross-domain discovery and trust.

The stable AIVCS contract should be protocol-neutral:

```text
external_protocol_event
  -> policy admission check
  -> AIVCS branch/snapshot/commit/ref
  -> validation/evidence graph
  -> replayable task/run ledger
```

Minimum event metadata to preserve:

- `protocol`: `mcp`, `a2a`, `acp`, `anp`, or `internal`
- `protocol_version`
- `acp_session_id` when routed through ACP
- `acp_prompt_turn_id` or message ID when available
- `trace_id`
- `task_id` or `run_id`
- `agent_id`
- `agent_card_hash` or manifest hash when available
- `mcp_server_uri` or remote endpoint URI when available
- `aivcs_commit_id`
- `aivcs_branch`
- `data_fabric_run_id`
- `evidence_graph_id`
- `policy_decision_id`
- `rollback_ref`

## Proposed AIVCS Protocol Surfaces

### ACP Agent

Implement an AIVCS-backed ACP agent in Rust using `agent-client-protocol`:

- `initialize`: advertise AIVCS capabilities and supported session operations.
- `session/new`: create a deterministic AIVCS run/session with an initial snapshot.
- `session/load` and `session/resume`: restore long-running engineering tasks from AIVCS refs without losing task state.
- `session/prompt`: route the prompt turn through Brains/context-pack, AIVCS snapshot phases, validation, and evidence capture.
- `session/update`: stream plans, diffs, validation state, tool calls, and AIVCS refs back to the client.
- `session/request_permission`: bind mutation permissions to data-fabric policy decisions and AIVCS rollback refs.

ACP is the preferred interface for headless deterministic routing between coding clients and AIVCS-backed agents. A2A should be used only when a separate peer agent needs a delegated task boundary.

### MCP Server

Expose AIVCS operations as typed MCP tools and resources:

- `aivcs.snapshot`
- `aivcs.restore`
- `aivcs.fork`
- `aivcs.trace`
- `aivcs.semantic_merge`
- `aivcs.replay`
- `aivcs.pr_pipeline`
- `aivcs.get_reasoning_trace`

Resources:

- `aivcs://commits/{commit_id}`
- `aivcs://branches/{branch}`
- `aivcs://runs/{run_id}`
- `aivcs://traces/{trace_id}`
- `aivcs://evidence/{evidence_graph_id}`

### A2A Agent

Advertise an AIVCS Agent Card:

- Name: `aivcs-version-orchestrator`
- Skills: `snapshot_task`, `fork_branch`, `semantic_merge`, `replay_task`, `rollback`, `open_pr_pipeline`
- Input modes: `application/json`, `text/plain`
- Output modes: `application/json`, `text/markdown`
- Capabilities: streaming task updates, push notifications, extended Agent Card

Map AIVCS phases to A2A task states:

| AIVCS phase | A2A state |
| --- | --- |
| `task_accepted` | submitted |
| `context_pack_compiled` | working |
| `plan_accepted` | working |
| `hypothesis_branches_created` | working |
| `patch_generated` | working |
| `tests_verified` | working/completed |
| `pr_opened` | completed |
| `reflection_written` | completed |

## Risks and Gaps

1. "ACPX" naming is ambiguous. Keep the research track explicitly tied to Agent Client Protocol plus MCP/A2A/ANP rather than inventing a new protocol.
2. ACP handles client-agent routing; it does not replace AIVCS state, data-fabric policy, or MCP tools.
3. Existing local A2A types are narrower than current protocol expectations. `oxidizedgraph/src/a2a/types.rs` currently models `MessagePart` as text-only, while modern agent protocols expect typed parts/artifacts.
4. Protocol trust is not enough. AIVCS must bind every session and prompt turn to policy, signatures where available, provenance, snapshots, and rollback.
5. MCP tool access is high-risk for mutation operations. AIVCS MCP tools need admission checks, scoped credentials, and clear separation between read-only and write/mutation tools.
6. ACP session history is not sufficient by itself. AIVCS should persist critical task state, artifacts, evidence, and replay metadata independently.

## Research Backlog

1. Verify the current ACP v1 schema and Rust crate APIs against `agentclientprotocol.com` and `agent-client-protocol` docs.
2. Draft `docs/protocol-mapping.md` for AIVCS: ACP sessions/prompt turns/updates, MCP tools/resources, A2A skills/tasks/artifacts, and the event envelope.
3. Prototype a Rust AIVCS ACP agent that supports `initialize`, `session/new`, `session/load`, `session/resume`, and `session/prompt`.
4. Inspect upstream `stevedores-org/aivcs` auth/gateway work and map it to ACP permission requests plus MCP OAuth/resource-server requirements.
5. Add conformance tests for ACP prompt turns becoming replayable AIVCS commits with policy decisions and rollback refs.
6. Decide whether AGNTCY OASF/Directory or ANP DID-based identity is needed for cross-org discovery in phase 1 or should remain a later integration.

## Immediate Recommendation

Use ACP-first for headless deterministic client-agent routing, MCP for tool/resource access, A2A for delegated peer-agent task boundaries, and ANP/AGNTCY as later identity/discovery options. AIVCS's differentiator should be durable, replayable, policy-bound provenance for all of these protocol interactions. Implement the AIVCS side in Rust.

## Sources

- Agent Client Protocol introduction: https://agentclientprotocol.com/get-started/introduction
- Agent Client Protocol architecture: https://agentclientprotocol.com/get-started/architecture
- Agent Client Protocol v1 overview: https://agentclientprotocol.com/protocol/v1/overview
- Agent Client Protocol session setup: https://agentclientprotocol.com/protocol/v1/session-setup
- Agent Client Protocol Rust library: https://agentclientprotocol.com/libraries/rust
- A2A specification: https://a2a-protocol.org/latest/specification/
- Google A2A announcement: https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/
- Linux Foundation A2A launch: https://www.linuxfoundation.org/press/linux-foundation-launches-the-agent2agent-protocol-project-to-enable-secure-intelligent-communication-between-ai-agents
- MCP specification: https://modelcontextprotocol.io/specification/2025-11-25
- MCP authorization: https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
- ANP protocol stack: https://agent-network-protocol.com/
- AGNTCY documentation: https://docs.agntcy.org/
- Survey: https://arxiv.org/abs/2505.02279
