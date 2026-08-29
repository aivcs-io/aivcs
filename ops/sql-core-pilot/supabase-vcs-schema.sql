-- AIVCS central VCS schema for Supabase PostgreSQL.
-- Run this file in the Supabase SQL Editor for project:
-- busolkxpliryfwnnbnix
--
-- This intentionally mirrors forge-cas::SqlxForgeStore::migrate. Keep the
-- schema PostgreSQL-native and portable; do not add Supabase-only types or
-- APIs to the VCS persistence contract.

BEGIN;

CREATE TABLE IF NOT EXISTS public.forge_blobs (
    digest TEXT PRIMARY KEY,
    bytes BYTEA NOT NULL
);

CREATE TABLE IF NOT EXISTS public.forge_commits (
    repo        TEXT NOT NULL,
    commit_id   TEXT NOT NULL,
    tree_digest TEXT NOT NULL,
    manifest    TEXT NOT NULL,
    parents     TEXT NOT NULL,
    message     TEXT NOT NULL,
    author      TEXT NOT NULL,
    PRIMARY KEY (repo, commit_id)
);

CREATE TABLE IF NOT EXISTS public.forge_branches (
    repo           TEXT NOT NULL,
    branch         TEXT NOT NULL,
    head_commit_id TEXT NOT NULL,
    updated_at_ms  BIGINT NOT NULL,
    PRIMARY KEY (repo, branch)
);

CREATE TABLE IF NOT EXISTS public.forge_repositories (
    repo    TEXT PRIMARY KEY,
    private BOOLEAN NOT NULL DEFAULT TRUE
);

-- Useful lookup indexes. Primary keys already cover their key order; these
-- indexes target the common repository and branch query shapes explicitly.
CREATE INDEX IF NOT EXISTS forge_commits_repo_idx
    ON public.forge_commits (repo);

CREATE INDEX IF NOT EXISTS forge_branches_repo_idx
    ON public.forge_branches (repo);

-- Keep direct SQLx access working while preventing accidental browser/API
-- access through Supabase's exposed roles. Add narrowly scoped policies later
-- only if a Supabase API client is intentionally introduced.
ALTER TABLE public.forge_blobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.forge_commits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.forge_branches ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.forge_repositories ENABLE ROW LEVEL SECURITY;

COMMIT;

-- Verify the objects created by this script.
SELECT table_name
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_name LIKE 'forge_%'
ORDER BY table_name;
