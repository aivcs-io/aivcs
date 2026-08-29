//! End-to-end tests for memory compaction and retention policies.

use chrono::{Duration, Utc};

use aivcs_core::memory::context::{assemble_context, ContextBudget};
use aivcs_core::memory::index::{IndexQuery, MemoryEntry, MemoryEntryKind, MemoryIndex};
use aivcs_core::memory::retention::{compact_index, CompactionPolicy, CompactionResult};

fn entry(id: &str, age_days: i64, tokens: usize) -> MemoryEntry {
    MemoryEntry {
        id: id.into(),
        kind: MemoryEntryKind::RunTrace,
        summary: format!("summary {id}"),
        content_digest: format!("digest_{id}"),
        created_at: Utc::now() - Duration::days(age_days),
        tags: Vec::new(),
        token_estimate: tokens,
        relevance: 0.5,
    }
}

#[test]
fn test_compact_age_only() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("fresh", 1, 100)).unwrap();
    idx.insert(entry("stale", 60, 100)).unwrap();
    idx.insert(entry("ancient", 120, 100)).unwrap();

    let r = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: Some(30),
            max_entries: None,
            min_token_threshold: None,
        },
    )
    .unwrap();
    assert_eq!(r.removed_count, 2);
    assert_eq!(r.remaining_count, 1);
    assert!(idx.get("fresh").is_ok());
}

#[test]
fn test_compact_count_only() {
    let mut idx = MemoryIndex::new();
    for i in 0..10 {
        idx.insert(entry(&format!("e{i}"), i, 100)).unwrap();
    }
    let r = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: None,
            max_entries: Some(5),
            min_token_threshold: None,
        },
    )
    .unwrap();
    assert_eq!(r.removed_count, 5);
    assert_eq!(r.remaining_count, 5);
}

#[test]
fn test_compact_min_tokens_only() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("tiny", 0, 3)).unwrap();
    idx.insert(entry("medium", 0, 50)).unwrap();
    idx.insert(entry("large", 0, 500)).unwrap();

    let r = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: None,
            max_entries: None,
            min_token_threshold: Some(20),
        },
    )
    .unwrap();
    assert_eq!(r.removed_count, 1);
    assert!(r.removed_ids.contains(&"tiny".to_string()));
}

#[test]
fn test_compact_combined_all_three_policies() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("old", 200, 100)).unwrap();
    idx.insert(entry("tiny", 1, 2)).unwrap();
    for i in 0..5 {
        idx.insert(entry(&format!("keep{i}"), i, 100)).unwrap();
    }
    let r = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: Some(30),
            max_entries: Some(3),
            min_token_threshold: Some(10),
        },
    )
    .unwrap();
    assert_eq!(r.remaining_count, 3);
    assert!(r.removed_ids.contains(&"old".to_string()));
    assert!(r.removed_ids.contains(&"tiny".to_string()));
}

#[test]
fn test_compact_noop_when_everything_fits() {
    let mut idx = MemoryIndex::new();
    idx.insert(entry("e1", 1, 100)).unwrap();
    idx.insert(entry("e2", 2, 200)).unwrap();
    let r = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: Some(365),
            max_entries: Some(100),
            min_token_threshold: Some(10),
        },
    )
    .unwrap();
    assert_eq!(r.removed_count, 0);
    assert_eq!(r.remaining_count, 2);
}

#[test]
fn test_compact_then_context_assembly_pipeline() {
    let mut idx = MemoryIndex::new();

    let mut e1 = entry("recent_good", 1, 200);
    e1.relevance = 0.9;
    e1.tags = vec!["agent:coder".into()];
    let mut e2 = entry("recent_ok", 2, 150);
    e2.relevance = 0.6;
    e2.tags = vec!["agent:coder".into()];
    let mut e3 = entry("ancient", 100, 100);
    e3.relevance = 0.95;
    e3.tags = vec!["agent:coder".into()];
    let mut e4 = entry("trivial", 1, 3);
    e4.relevance = 0.1;

    idx.insert(e1).unwrap();
    idx.insert(e2).unwrap();
    idx.insert(e3).unwrap();
    idx.insert(e4).unwrap();

    let compaction = compact_index(
        &mut idx,
        &CompactionPolicy {
            max_age_days: Some(30),
            max_entries: None,
            min_token_threshold: Some(10),
        },
    )
    .unwrap();
    assert_eq!(compaction.removed_count, 2);

    let results = idx.query(&IndexQuery::all().with_tag("agent:coder"));
    let budget = ContextBudget::new(1000, 200).unwrap();
    let window = assemble_context(&results.entries, &budget);
    assert_eq!(window.items.len(), 2);
    assert_eq!(window.items[0].entry_id, "recent_good");
    assert_eq!(window.total_tokens, 350);
}

#[test]
fn test_compaction_result_serde_roundtrip_e2e() {
    let r = CompactionResult {
        removed_count: 5,
        remaining_count: 15,
        removed_ids: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
    };
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(r, serde_json::from_str::<CompactionResult>(&json).unwrap());
}

#[test]
fn test_compaction_policy_serde_roundtrip() {
    let p = CompactionPolicy {
        max_age_days: Some(60),
        max_entries: Some(500),
        min_token_threshold: Some(25),
    };
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(p, serde_json::from_str::<CompactionPolicy>(&json).unwrap());
}
