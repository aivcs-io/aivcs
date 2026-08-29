# AIVCS SQL core pilot

This directory provisions two isolated candidates for the central VCS SQL
store. It does not change the production `DATABASE_URL`.

## Targets

* `supabase`: set `SUPABASE_DATABASE_URL` to the direct PostgreSQL connection
  string from Supabase. This is the PostgreSQL-dialect compatibility target.
* `cockroach`: use the local CockroachDB container, or set
  `COCKROACH_DATABASE_URL` to a CockroachDB Cloud connection string. This is
  the distributed SQL target.

Both targets use the same schema and the existing `forge-cas` integration
test. Do not use the free cloud tiers as the production source of truth:
Supabase Free can pause inactive projects and has no automatic backups;
Cockroach Basic has bounded free resource usage.

## Local setup

```sh
cd ops/sql-core-pilot
docker compose up -d cockroach
./smoke.sh cockroach
```

The local Cockroach connection is:

```text
postgresql://root@localhost:26257/defaultdb?sslmode=disable
```

## Hosted setup

Export credentials in the shell; do not commit them:

```sh
export SUPABASE_DATABASE_URL='postgresql://...'
export COCKROACH_DATABASE_URL='postgresql://...'
./smoke.sh supabase
./smoke.sh cockroach
```

The hosted URLs must be reachable from the machine running the test. The
script only creates the four `forge_*` tables and writes a namespaced smoke
fixture, then removes that fixture.

## AIVCS integration test

For either target, run the existing ignored integration test:

```sh
TEST_DATABASE_URL="$SUPABASE_DATABASE_URL" \
  cargo test -p forge-cas -- --ignored sqlx_store_roundtrip_against_postgres

TEST_DATABASE_URL="$COCKROACH_DATABASE_URL" \
  cargo test -p forge-cas -- --ignored sqlx_store_roundtrip_against_postgres
```

Do not cut over `DATABASE_URL` until the same test suite has passed against
both targets, including concurrent branch-update and atomic-publish tests.
