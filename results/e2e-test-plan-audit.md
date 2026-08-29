# End-to-End Test Plan Audit (Issue #9)

**Date:** 2026-02-18
**Branch:** `develop`
**Total tests:** 254 passed, 0 failed, 2 ignored (doc-tests)

---

## AIVCS-001: Agent Versioning MVP

| Story | Acceptance Criteria | Test(s) | Status |
|-------|-------------------|---------|--------|
| **S1: CAS store + retrieve** | put/get bytes+json, dedupe works | `cas::fs::tests::blob_roundtrip`, `dedupe_invariant`, `random_bytes_roundtrip`, `empty_blob`, `large_blob` + trait_contracts: `cas_*` (10 tests) | PASS |
| **S2: AgentSpec hashing** | Stable digest regardless of field order, digest changes on field mutation | `agent_spec_digest_stable`, `agent_spec_digest_changes_on_mutation`, `agent_spec_field_order_invariant`, `agent_spec_digest_golden_value`, `canonical_json_*` (10+ tests) | PASS |
| **S3: Run capture adapter** | 2-node graph run -> ledger has ordered events | `happy_path_two_node_lifecycle`, `failure_path_node_failed`, `handler_creates_and_completes_run`, `custom_event_mapping`, `map_event_covers_all_variants` | PASS |
| **S4: Replay from recorded artifacts** | Replay reproduces stored event log + identical final_state_digest | `replay_golden_digest_equality` (unit + CLI integration), `replay_event_order`, `replay_empty_run`, `replay_missing_run_rejection` | PASS |

## AIVCS-002: Semantic Diff Engine

| Story | Acceptance Criteria | Test(s) | Status |
|-------|-------------------|---------|--------|
| **S1: Tool-call diff** | Detect added/removed/reordered calls, param diffs | `tool_call_diff::*` (11 tests): `added_call_detected`, `removed_call_detected`, `reordered_calls_detected`, `param_changed_detected`, `nested_json_param_produces_deep_deltas`, `symmetry_added_becomes_removed` | PASS |
| **S2: Path diff from event logs** | Extract node traversal, highlight divergence point | `node_path_diff::*` (8 tests): `diverges_at_first_step`, `diverges_mid_path`, `path_a_is_prefix_of_b`, etc. | PASS |
| **S3: State diff (scoped keys)** | JSON pointer diff on specified keys | `state_diff::*` (12 tests): `single_pointer_value_changed`, `nested_pointer_value_changed`, `multiple_pointers_mixed_changes`, `diff_run_states_end_to_end` | PASS |

## AIVCS-003: Eval-Gated PR Workflow

| Story | Acceptance Criteria | Test(s) | Status |
|-------|-------------------|---------|--------|
| **S1: EvalSuite schema + runner** | Dataset test cases, scores per test + aggregated | `eval::tests::*` (14 tests): `eval_suite_fluent_api`, `deterministic_eval_runner_golden_output`, `deterministic_eval_runner_stable_score`, `eval_suite_digest_stable` | PASS |
| **S2: Merge gate rules** | Threshold gates (pass rate, regression), fail fast | `merge_gate::*` (15 tests): `below_min_pass_rate_fails`, `regression_exceeding_limit_fails`, `fail_fast_stops_at_first_violation`, `required_tag_some_fail` | PASS |
| **S3: GitHub Actions template** | Reusable workflow, attaches artifacts | `eval_workflow::*` (8 tests): `workflow_file_exists_and_is_nonempty`, `workflow_is_callable_via_workflow_call`, `workflow_uploads_artifacts`, `workflow_has_gate_enforcement_step` | PASS |

## AIVCS-004: Agent Registry + Rollback Deploys

| Story | Acceptance Criteria | Test(s) | Status |
|-------|-------------------|---------|--------|
| **S1: Registry storage + API** | Promote sets pointer, rollback moves to prior, history queryable | `promote_promote_rollback_keeps_append_only_history`, `surreal_registry_*` (4 tests), `registry_*` trait_contracts (10 tests) | PASS |
| **S2: Release compatibility checks** | Block release if tools missing or schema mismatch | `compat::*` (12 tests): `no_tools_change_fails_different_digest`, `require_tools_digest_fails_empty`, `spec_digest_valid_fails_*` + `promote_validator::*` (8 tests) | PASS |
| **S3: Deploy by digest** | Load AgentSpec by digest, execute run, produce expected output | `deploy_by_digest_matches_replay_golden`, `deploy_run_records_spec_digest_and_completes` | PASS |

## M5 Hardening

| Area | Test(s) | Status |
|------|---------|--------|
| Observability | `observability::metric_counters_reflect_increments`, `flush_emits_aggregated_metric_event` | PASS |
| Publish gating | `publish_gate::*` (15 tests) | PASS |
| Version consistency | `version_consistency::*` (2 tests) | PASS |

## CI Hard Gates (from Issue #9)

| Gate | Status |
|------|--------|
| `cargo fmt --check` | Verified in CI |
| `cargo clippy -D warnings` | Verified in CI |
| `cargo test --workspace` | **254 tests, 0 failures** |
| Golden-run: snapshot -> capture -> replay -> identical digest | `replay_golden_digest_equality` PASS |

---

## Summary

All 4 epics (16 stories) fully covered. 254 tests, 0 failures, 2 ignored doc-tests (expected). Every acceptance criterion from the TDD plan has at least one test passing.
