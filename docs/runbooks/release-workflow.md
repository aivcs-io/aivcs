# Runbook: Release workflow (`aivcs release`)

The release registry is an **append-only** record of which agent spec is the
"current" release for a given agent, with full history and one-step rollback.
The concepts are described in [architecture.md](../architecture.md)
(`ReleaseRegistry`); this runbook is the operational guide.

> Confirm the live surface with `aivcs release --help` and
> `aivcs release <subcommand> --help`. Command names below are taken from the
> CLI definition.

## Subcommands

| Subcommand | Purpose |
|------------|---------|
| `promote`  | Promote a validated agent spec as the latest release |
| `current`  | Show the current release pointer for an agent |
| `history`  | Show release history for an agent (newest first) |
| `rollback` | Roll back the agent to the previous release (append-only) |

## Promote a release

A release pins the four content digests that define an agent spec (graph,
prompts, tools, config) to a git commit SHA. The agent name is a **positional**
argument (not a flag).

```bash
aivcs release promote my-agent \
  --git-sha <git-sha> \
  --graph-digest   <sha256-hex> \
  --prompts-digest <sha256-hex> \
  --tools-digest   <sha256-hex> \
  --config-digest  <sha256-hex> \
  --version v1.2.3 \           # optional version label
  --notes "First stable spec"  # optional release notes
  # --promoted-by defaults to "aivcs-cli"
```

`--version`, `--notes`, and `--promoted-by` are optional. The four `*-digest`
values and `--git-sha` identify exactly which spec was promoted.

## Inspect releases

```bash
aivcs release current my-agent   # the active release pointer
aivcs release history my-agent   # full history, newest first
```

## Roll back

`rollback` appends a new entry that re-points `current` to the previous
release — history is never rewritten, so a rollback is itself auditable and
can be rolled forward again by promoting again.

```bash
aivcs release rollback my-agent
aivcs release current  my-agent  # verify the pointer moved
```

## When to use

- **promote**: after a spec passes validation/CI and you want it to be the
  canonical version agents resolve to.
- **rollback**: to revert to the prior known-good spec without rebuilding it.
- **current / history**: auditing which spec is live and how it got there.

## See also

- [Getting Started — Command map](../getting-started.md#command-map)
- [Architecture — ReleaseRegistry](../architecture.md)
