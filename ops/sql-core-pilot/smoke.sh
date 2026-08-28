#!/usr/bin/env bash
set -euo pipefail

target="${1:-}"
case "$target" in
  supabase) dsn="${SUPABASE_DATABASE_URL:-}" ;;
  cockroach) dsn="${COCKROACH_DATABASE_URL:-postgresql://root@localhost:26257/defaultdb?sslmode=disable}" ;;
  *) echo "usage: $0 supabase|cockroach" >&2; exit 2 ;;
esac

if [[ -z "$dsn" ]]; then
  echo "missing ${target^^}_DATABASE_URL" >&2
  exit 2
fi

command -v psql >/dev/null || { echo "psql is required" >&2; exit 1; }

psql "$dsn" -v ON_ERROR_STOP=1 -f "$(dirname "$0")/schema.sql"
psql "$dsn" -v ON_ERROR_STOP=1 <<'SQL'
CREATE TEMP TABLE aivcs_sql_core_smoke (
    digest TEXT PRIMARY KEY,
    payload BYTEA NOT NULL
);
INSERT INTO aivcs_sql_core_smoke VALUES ('pilot-smoke', decode('6169766373', 'hex'));
INSERT INTO aivcs_sql_core_smoke VALUES ('pilot-smoke-2', decode('706f737467726573', 'hex'))
    ON CONFLICT (digest) DO NOTHING;
SELECT 'ok' AS result, count(*) AS rows FROM aivcs_sql_core_smoke;
SQL

echo "${target}: SQL connectivity and portable DDL verified"
