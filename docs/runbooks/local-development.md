# Local Development Runbook

## Clone and Build

```bash
git clone aivcs://stevedores-org/aivcs.git
cd aivcs
cargo build
```

## Running Tests

```bash
# Full test suite (~195 tests)
cargo test --all

# Single crate
cargo test -p oxidized-state

# Single test by name
cargo test test_snapshot_is_atomic

# With output
cargo test -- --nocapture
```

## Linting

```bash
# Clippy (CI enforces zero warnings)
cargo clippy --all -- -D warnings

# Formatting
cargo fmt --all -- --check
```

## Logging

Set `RUST_LOG` for fine-grained control:

```bash
# Default info level
RUST_LOG=info cargo run -- log

# Debug for core crate only
RUST_LOG=aivcs_core=debug cargo run -- snapshot --state state.json

# Trace everything
RUST_LOG=trace cargo run -- merge feature --target main
```

## Dev Workflows

### MCP gateway + mom memory topology

Memory MCP tools ([proposal](../proposals/mcp-memory-surface.md), [ADR 001](../adr/001-memory-mom-deployment-topology.md)) use a dual-mode mom backend:

```bash
# Default local dev — embedded in-process mom (no sidecar)
cargo run -p aivcs-mcp-gateway
# listens :8082; MOM_BACKEND_URL unset → in-memory mom store inside gateway

# Sidecar mode — against a running mom-service
MOM_BACKEND_URL=http://127.0.0.1:8080 cargo run -p aivcs-mcp-gateway

# Start mom separately when testing HTTP mode
# (from lornu-ai/mom checkout)
cargo run -p mom-service
```

Production/staging sets `MOM_BACKEND_URL` to the internal mom service URL. Session memory (CommitId-scoped) always uses oxidized-state inside aivcs regardless of mom mode.

See also [mcp-auth-guide.md](../mcp-auth-guide.md) for gateway auth and [database-configuration.md](database-configuration.md) for SurrealDB endpoints.

### Add a new CLI command

1. Add a variant to `Commands` enum in `crates/aivcs-cli/src/main.rs`
2. Add a match arm in the `main()` dispatch
3. Implement `cmd_<name>()` function
4. Add tests in the `#[cfg(test)] mod tests` block

### Add a new domain type

1. Create a file in `crates/aivcs-core/src/domain/`
2. Export from `crates/aivcs-core/src/domain/mod.rs`
3. Re-export from `crates/aivcs-core/src/lib.rs`
4. Write co-located `#[cfg(test)] mod tests`

### Add a new SurrealDB record type

1. Define the struct in `crates/oxidized-state/src/schema.rs`
2. Add CRUD methods to `SurrealHandle` in `crates/oxidized-state/src/handle.rs`
3. Add schema creation to `create_schema()` if needed
4. Write tests using `SurrealHandle::setup_db()` (in-memory)
