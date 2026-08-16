# Bug Fixes — June 2026

A correctness pass over `aivcs-core` found and fixed four genuine logic bugs.
Each fix ships with a regression test; the full workspace lib suite (424 tests)
passes with no regressions.

## Summary

| # | Location | Severity | Symptom |
|---|----------|----------|---------|
| 1 | `aivcs-core/src/diff/tool_calls.rs` | Medium | Spurious `Reordered` tool-call changes |
| 2 | `aivcs-core/src/publish_gate.rs` | High | Valid releases wrongly blocked by `VersionBump` |
| 3 | `aivcs-core/src/multi_repo/graph.rs` | High | Repo scheduled concurrently with its own dependency |
| 4 | `aivcs-core/src/domain/eval.rs` | High | Empty eval suite vacuously passes the gate |

---

## 1. Phantom "reordered" tool calls

**File:** `crates/aivcs-core/src/diff/tool_calls.rs`

Reorder detection compared the *absolute* LCS indices of aligned tool calls
(`if i_a != i_b`). LCS-aligned pairs are monotonically increasing in both index
sequences, so this check can never identify a true reorder — but it fires a
spurious `Reordered` change for every aligned call that follows an insertion or
removal.

**Trigger:** diffing `A = [search, fetch]` against `B = [translate, search, fetch]`
(only `translate` was added) produced two false `Reordered` entries.

**Fix:** dropped the index check. True reorders are now detected by matching a
tool that disappears from `A` and reappears (same name) in `B`; the remainder
are genuine additions/removals.

## 2. Wrong semver pre-release ordering blocks valid releases

**File:** `crates/aivcs-core/src/publish_gate.rs`

For versions sharing the same `MAJOR.MINOR.PATCH`, pre-release tags were compared
with lexicographic `String::cmp`. That ordered `1.0.0-alpha.10` *below*
`1.0.0-alpha.9` (because `'1' < '9'`), so the `VersionBump` publish rule
**rejected a legitimately higher** release candidate.

**Fix:** added `cmp_prerelease`, which implements semver §11 — identifiers are
split on `.`, purely numeric identifiers compare numerically, numeric ranks
below alphanumeric, and a longer identifier set wins when all preceding ones are
equal.

## 3. Repo scheduled concurrently with its own dependency

**File:** `crates/aivcs-core/src/multi_repo/graph.rs`

`RepoExecutionPlan::parallel_groups()` merged adjacent steps flagged
`parallelizable` without regard to topological level. The `parallelizable` flag
only means "shares a level with a sibling," and adjacent steps in the plan can
belong to different levels.

**Trigger:** roots `A, B` (level 0) followed by dependents `C, D` that both
depend on `A` (level 1) collapsed into a single parallel group — scheduling `C`
and `D` to run concurrently with their dependency `A`.

**Fix:** grouping is now dependency-aware — a step never joins a group that
already contains a repo it depends on.

## 4. Empty eval suite vacuously passes the gate

**File:** `crates/aivcs-core/src/domain/eval.rs`

An evaluation suite with zero test cases reported `pass_rate = 1.0` and
`overall_pass = true`, so a misconfigured or fully-filtered suite green-lit a
release with no evidence (`1.0 >= min_pass_rate` always holds).

**Fix:** an empty suite now yields `pass_rate = 0.0` and fails the gate
(`overall_pass` additionally requires `total_cases > 0`).

---

## Verification

```bash
cargo test --workspace --lib
# 424 passed; 0 failed
```

New regression tests:

- `diff::tool_calls::tests::insertion_does_not_produce_spurious_reorders`
- `diff::tool_calls::tests::swapped_calls_report_a_single_reorder`
- `publish_gate::tests::numeric_prerelease_identifiers_compare_numerically`
- `publish_gate::tests::prerelease_is_less_than_release`
- `publish_gate::tests::version_bump_accepts_higher_numeric_prerelease`
- `multi_repo::graph::tests::test_parallel_groups_never_group_a_step_with_its_dependency`
- `domain::eval::tests::test_empty_suite_does_not_vacuously_pass`
