# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.4] - 2026-08-28

### Added
- Public release bump past v0.4.3 with updated Forge v2 login and device-flow discovery.
- Reverse proxy route resilience and full URL-encoded slash fallback support across CAS endpoints.

## [0.4.1] - 2026-08-16

### Fixed
- Fixed `aivcs clone` / `fetch` proxy routing issue. Added double-segment route fallback support in `forge-cas` to handle reverse proxies (like Cloudflare/ALBs) that decode `%2F` urlencoded repo slashes in HTTP paths.

## [0.3.2] - 2026-06-15

### Added
- crates.io publish workflow (`publish.yml`) and release runbook

## [0.1.0] - 2026-02-18

### Added
- Snapshot core: commit, restore, branch, log commands
- Content-addressed store (CAS) with SHA-256 digests
- SurrealDB backend (in-memory and WebSocket/Cloud)
- Nix Flake environment hashing and Attic binary cache integration
- Semantic merge with memory vector diffing and heuristic conflict resolution
- Parallel branch forking with concurrent Tokio tasks
- Branch pruning based on score thresholds
- Time-travel trace debugging
- Run recording and replay with deterministic digest verification
- Tool-call sequence diffing (LCS-based)
- Diff commands for runs and state
- Release registry with promote/rollback support
- Eval suite with deterministic runner and scorer framework
