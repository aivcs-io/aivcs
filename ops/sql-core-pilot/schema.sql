-- Mirrors forge-cas::SqlxForgeStore::migrate.
-- Keep this schema portable across PostgreSQL and CockroachDB.
CREATE TABLE IF NOT EXISTS forge_blobs (
    digest TEXT PRIMARY KEY,
    bytes BYTEA NOT NULL
);

CREATE TABLE IF NOT EXISTS forge_commits (
    repo TEXT NOT NULL,
    commit_id TEXT NOT NULL,
    tree_digest TEXT NOT NULL,
    manifest TEXT NOT NULL,
    parents TEXT NOT NULL,
    message TEXT NOT NULL,
    author TEXT NOT NULL,
    PRIMARY KEY (repo, commit_id)
);

CREATE TABLE IF NOT EXISTS forge_branches (
    repo TEXT NOT NULL,
    branch TEXT NOT NULL,
    head_commit_id TEXT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (repo, branch)
);

CREATE TABLE IF NOT EXISTS forge_repositories (
    repo TEXT PRIMARY KEY,
    private BOOLEAN NOT NULL DEFAULT TRUE
);
