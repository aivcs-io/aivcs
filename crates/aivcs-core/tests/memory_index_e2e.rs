//! End-to-end tests for memory indexing, querying, and rationale capture.

use chrono::{Duration, Utc};

use aivcs_core::memory::context::{assemble_context, ContextBudget};
use aivcs_core::memory::index::{IndexQuery, MemoryEntry, MemoryEntryKind, MemoryIndex};
use aivcs_core::memory::rationale::{DecisionRationale, RationaleEntry, RationaleOutcome};

fn entry(id: &str, kind: MemoryEntryKind, tags: &[&str], age_h: i64, tokens: usize) -> MemoryEntry {
    MemoryEntry {
        id: id.into(),
        kind,
        summary: format!("summary for {id}"),
        content_digest: format!("digest_{id}"),
        created_at: Utc::now() - Duration::hours(age_h),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        token_estimate: tokens,
        relevance: 0.0,
    }
}

#[test]
fn test_index_insert_query_remove_lifecycle() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry(
        "r1",
        MemoryEntryKind::RunTrace,
        &["agent:planner"],
        1,
        100,
    ))
    .unwrap();
    idx.insert(entry(
        "r2",
        MemoryEntryKind::Rationale,
        &["agent:coder"],
        2,
        200,
    ))
    .unwrap();
    idx.insert(entry(
        "d1",
        MemoryEntryKind::Diff,
        &["agent:planner"],
        3,
        150,
    ))
    .unwrap();
    assert_eq!(idx.len(), 3);

    assert_eq!(
        idx.query(&IndexQuery::all().with_kind(MemoryEntryKind::RunTrace))
            .total_matches,
        1
    );
    assert_eq!(
        idx.query(&IndexQuery::all().with_tag("agent:planner"))
            .total_matches,
        2
    );

    idx.remove("r2").unwrap();
    assert_eq!(idx.len(), 2);
}

#[test]
fn test_index_query_time_filter() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("recent", MemoryEntryKind::RunTrace, &[], 1, 100))
        .unwrap();
    idx.insert(entry("old", MemoryEntryKind::RunTrace, &[], 48, 100))
        .unwrap();

    let cutoff = Utc::now() - Duration::hours(24);
    let result = idx.query(&IndexQuery::all().after(cutoff));
    assert_eq!(result.total_matches, 1);
    assert_eq!(result.entries[0].id, "recent");
}

#[test]
fn test_index_query_combined_kind_and_tag() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry(
        "rt1",
        MemoryEntryKind::RunTrace,
        &["run:abc"],
        0,
        100,
    ))
    .unwrap();
    idx.insert(entry(
        "rt2",
        MemoryEntryKind::RunTrace,
        &["run:def"],
        0,
        100,
    ))
    .unwrap();
    idx.insert(entry(
        "rat1",
        MemoryEntryKind::Rationale,
        &["run:abc"],
        0,
        100,
    ))
    .unwrap();

    let result = idx.query(
        &IndexQuery::all()
            .with_kind(MemoryEntryKind::RunTrace)
            .with_tag("run:abc"),
    );
    assert_eq!(result.total_matches, 1);
    assert_eq!(result.entries[0].id, "rt1");
}

#[test]
fn test_index_serde_roundtrip_preserves_entries() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("e1", MemoryEntryKind::Snapshot, &["s:1"], 5, 50))
        .unwrap();
    idx.insert(entry(
        "e2",
        MemoryEntryKind::ToolResult,
        &["tool:bash"],
        1,
        75,
    ))
    .unwrap();

    let json = serde_json::to_string(&idx).unwrap();
    let restored: MemoryIndex = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.len(), 2);
    assert_eq!(restored.get("e1").unwrap().kind, MemoryEntryKind::Snapshot);
    assert_eq!(
        restored.get("e2").unwrap().kind,
        MemoryEntryKind::ToolResult
    );
}

#[test]
fn test_rationale_capture_and_index_retrieval() {
    let rationale = DecisionRationale::new("use ort merge", "fewer conflicts historically")
        .with_alternative("recursive merge")
        .with_constraint("must preserve file history")
        .with_confidence(0.92);

    let re = RationaleEntry::new(rationale.clone(), "run-42", 7)
        .with_outcome(RationaleOutcome::Success)
        .with_tag("merge:strategy");

    let mem_entry = MemoryEntry {
        id: format!("rat-{}-{}", re.run_id, re.event_seq),
        kind: MemoryEntryKind::Rationale,
        summary: format!("{}: {}", rationale.decision, rationale.reasoning),
        content_digest: "digest_placeholder".into(),
        created_at: re.decided_at,
        tags: re.tags.clone(),
        token_estimate: re.token_estimate(),
        relevance: rationale.confidence,
    };

    let mut idx = MemoryIndex::new();
    idx.insert(mem_entry).unwrap();

    let result = idx.query(&IndexQuery::all().with_kind(MemoryEntryKind::Rationale));
    assert_eq!(result.total_matches, 1);
    assert!(result.entries[0].summary.contains("ort merge"));
}

#[test]
fn test_rationale_with_different_outcomes() {
    let r = DecisionRationale::new("try fast-forward", "simplest option");
    assert_eq!(
        RationaleEntry::new(r.clone(), "r1", 1)
            .with_outcome(RationaleOutcome::Success)
            .outcome,
        Some(RationaleOutcome::Success)
    );
    assert_eq!(
        RationaleEntry::new(r.clone(), "r2", 1)
            .with_outcome(RationaleOutcome::Failure)
            .outcome,
        Some(RationaleOutcome::Failure)
    );
    assert_eq!(
        RationaleEntry::new(r.clone(), "r3", 1)
            .with_outcome(RationaleOutcome::Partial)
            .outcome,
        Some(RationaleOutcome::Partial)
    );
    assert_eq!(
        RationaleEntry::new(r, "r4", 1)
            .with_outcome(RationaleOutcome::Skipped)
            .outcome,
        Some(RationaleOutcome::Skipped)
    );
}

#[test]
fn test_context_assembly_from_index_results() {
    let mut idx = MemoryIndex::new();
    let mut e1 = entry("high", MemoryEntryKind::RunTrace, &["agent:coder"], 1, 200);
    e1.relevance = 0.95;
    let mut e2 = entry(
        "medium",
        MemoryEntryKind::Rationale,
        &["agent:coder"],
        2,
        150,
    );
    e2.relevance = 0.6;
    let mut e3 = entry("low", MemoryEntryKind::Diff, &["agent:coder"], 3, 300);
    e3.relevance = 0.2;
    idx.insert(e1).unwrap();
    idx.insert(e2).unwrap();
    idx.insert(e3).unwrap();

    let results = idx.query(&IndexQuery::all().with_tag("agent:coder"));
    let budget = ContextBudget::new(1000, 200).unwrap();
    let window = assemble_context(&results.entries, &budget);
    assert_eq!(window.items.len(), 3);
    assert_eq!(window.total_tokens, 650);
    assert_eq!(window.items[0].entry_id, "high");
}

#[test]
fn test_context_assembly_respects_budget_with_index() {
    let mut idx = MemoryIndex::new();
    let mut e1 = entry("big", MemoryEntryKind::RunTrace, &[], 0, 500);
    e1.relevance = 0.9;
    let mut e2 = entry("small", MemoryEntryKind::RunTrace, &[], 0, 100);
    e2.relevance = 0.5;
    idx.insert(e1).unwrap();
    idx.insert(e2).unwrap();

    let results = idx.query(&IndexQuery::all());
    let budget = ContextBudget::new(700, 200).unwrap();
    let window = assemble_context(&results.entries, &budget);
    assert_eq!(window.items.len(), 1);
    assert_eq!(window.items[0].entry_id, "big");
    assert_eq!(window.dropped_count, 1);
}

#[test]
fn test_rationale_entry_serde_roundtrip_e2e() {
    let e = RationaleEntry::new(
        DecisionRationale::new("decision A", "reason B")
            .with_alternative("alt C")
            .with_confidence(0.75),
        "run-99",
        12,
    )
    .with_outcome(RationaleOutcome::Partial)
    .with_tag("category:merge");

    let json = serde_json::to_string_pretty(&e).unwrap();
    let back: RationaleEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(e, back);
}
