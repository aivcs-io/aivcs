# Database Configuration Runbook

AIVCS uses SurrealDB as its backing store. Three deployment modes are supported.

## In-Memory (Default)

No configuration needed. When `SURREALDB_ENDPOINT` is unset, the library automatically starts an embedded in-memory SurrealDB instance. Data is lost when the process exits.

Best for: local development, testing, CI.

## Local SurrealDB

Run a local SurrealDB server:

```bash
# Install SurrealDB
curl -sSf https://install.surrealdb.com | sh

# Start local server (file-backed for persistence)
surreal start file:aivcs.db --user root --pass root
```

Configure AIVCS to connect:

```bash
export SURREALDB_ENDPOINT=ws://127.0.0.1:8000
export SURREALDB_USERNAME=root
export SURREALDB_PASSWORD=root
export SURREALDB_NAMESPACE=aivcs
export SURREALDB_DATABASE=main
```

Best for: local development with persistent data.

## SurrealDB Cloud

1. Create an account at [SurrealDB Cloud](https://surrealdb.com/cloud)
2. Provision an instance
3. Create a database user at **Authentication > Database Users** with Editor or Owner role

```bash
export SURREALDB_ENDPOINT=wss://YOUR_INSTANCE.aws-use1.surrealdb.cloud
export SURREALDB_USERNAME=your_username
export SURREALDB_PASSWORD=your_password
export SURREALDB_NAMESPACE=aivcs
export SURREALDB_DATABASE=main
```

Best for: production, shared team environments.

## Connection Behaviour

`SurrealHandle::setup_from_env()` checks environment variables in this order:

1. If `SURREALDB_ENDPOINT` is set → connect via WebSocket
2. Otherwise → start embedded in-memory instance

The schema (`create_schema()`) runs automatically on every connection, creating tables and indexes idempotently.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| "Failed to connect to AIVCS database" | Endpoint unreachable | Check `SURREALDB_ENDPOINT` and network |
| "Authentication failed" | Wrong credentials | Verify `SURREALDB_USERNAME` / `SURREALDB_PASSWORD` |
| Data missing after restart | Using in-memory mode | Set `SURREALDB_ENDPOINT` for persistence |
| "Table not found" errors | Schema not created | Ensure `create_schema()` runs (automatic on connect) |
