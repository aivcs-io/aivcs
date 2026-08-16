use aivcs_core::memory_context::{
    read_memory_context_artifact, write_memory_context_artifact, CompactionPolicy,
    ContextAssembler, ContextSegment, DecisionImportance, DecisionRationale, MemoryContextArtifact,
    MemoryEntry, MemoryIndex, MemoryQuery, RationaleLedger,
};
use chrono::{Duration, Utc};
use tempfile::tempdir;

fn sample_entries() -> Vec<MemoryEntry> {
    let now = Utc::now();
    vec![
        MemoryEntry {
            key: "deploy/config".to_string(),
            content: "Deploy uses rolling strategy with canary checks".to_string(),
            commit_id: "commit-a".to_string(),
            created_at: now - Duration::hours(2),
        },
        MemoryEntry {
            key: "test/flaky-retry".to_string(),
            content: "Flaky integration test retried 3 times before passing".to_string(),
            commit_id: "commit-a".to_string(),
            created_at: now - Duration::hours(1),
        },
        MemoryEntry {
            key: "deploy/rollback".to_string(),
            content: "Rollback triggered by health check failure on canary".to_string(),
            commit_id: "commit-b".to_string(),
            created_at: now,
        },
    ]
}

// ---- Memory Index ----

#[test]
fn memory_index_keyword_query_returns_scored_hits() {
    let mut index = MemoryIndex::new();
    index.ingest(sample_entries());
    assert_eq!(index.len(), 3);

    let hits = index.query(&MemoryQuery::keyword("deploy canary", 10));
    assert!(!hits.is_empty());
    // "deploy/config" and "deploy/rollback" both mention deploy
    assert!(hits[0].score > 0.0);
    // Top hit should score higher (both keywords match)
    assert!(hits[0].score >= hits.last().unwrap().score);
}

#[test]
fn memory_index_exact_query_returns_single_match() {
    let mut index = MemoryIndex::new();
    index.ingest(sample_entries());

    let hits = index.query(&MemoryQuery::exact("deploy/config"));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, "deploy/config");
    assert_eq!(hits[0].score, 1.0);
}

#[test]
fn memory_index_scoped_query_filters_by_commit() {
    let mut index = MemoryIndex::new();
    index.ingest(sample_entries());

    let q = MemoryQuery::keyword("deploy", 10).scoped(vec!["commit-b".to_string()]);
    let hits = index.query(&q);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].commit_id, "commit-b");
}

#[test]
fn memory_index_empty_query_returns_nothing() {
    let mut index = MemoryIndex::new();
    index.ingest(sample_entries());

    let hits = index.query(&MemoryQuery::exact("nonexistent/key"));
    assert!(hits.is_empty());
}

// ---- Rationale Ledger ----

#[test]
fn rationale_ledger_records_and_queries_by_run() {
    let mut ledger = RationaleLedger::new();
    let now = Utc::now();

    ledger.record(DecisionRationale {
        decision_id: "d1".to_string(),
        run_id: "run-1".to_string(),
        event_seq: 5,
        action: "chose rolling deploy".to_string(),
        reasoning: "Lower risk than blue-green for this service size".to_string(),
        alternatives_considered: vec!["blue-green".to_string(), "recreate".to_string()],
        importance: DecisionImportance::High,
        outcome: Some("success".to_string()),
        recorded_at: now,
    });

    ledger.record(DecisionRationale {
        decision_id: "d2".to_string(),
        run_id: "run-2".to_string(),
        event_seq: 3,
        action: "skip integration tests".to_string(),
        reasoning: "CI budget exhausted, unit tests passed".to_string(),
        alternatives_considered: vec!["run partial suite".to_string()],
        importance: DecisionImportance::Medium,
        outcome: None,
        recorded_at: now,
    });

    assert_eq!(ledger.len(), 2);

    let run1 = ledger.for_run("run-1");
    assert_eq!(run1.len(), 1);
    assert_eq!(run1[0].action, "chose rolling deploy");

    let deploy_decisions = ledger.for_action("deploy");
    assert_eq!(deploy_decisions.len(), 1);
}

#[test]
fn rationale_ledger_filters_by_importance() {
    let mut ledger = RationaleLedger::new();
    let now = Utc::now();

    for (id, importance) in [
        ("d1", DecisionImportance::Low),
        ("d2", DecisionImportance::Medium),
        ("d3", DecisionImportance::High),
        ("d4", DecisionImportance::Critical),
    ] {
        ledger.record(DecisionRationale {
            decision_id: id.to_string(),
            run_id: "run-1".to_string(),
            event_seq: 1,
            action: format!("action-{}", id),
            reasoning: "reason".to_string(),
            alternatives_considered: vec![],
            importance,
            outcome: None,
            recorded_at: now,
        });
    }

    let high_plus = ledger.important_decisions(DecisionImportance::High);
    assert_eq!(high_plus.len(), 2); // High + Critical
}

// ---- Context Assembly ----

#[test]
fn context_assembler_respects_token_budget() {
    let assembler = ContextAssembler::new(100);
    let segments = vec![
        ContextSegment::with_tokens("critical", "important context", 10, 40),
        ContextSegment::with_tokens("medium", "useful context", 5, 40),
        ContextSegment::with_tokens("low", "nice to have", 1, 40),
    ];

    let ctx = assembler.assemble(segments);
    // Budget is 100, segments cost 40 each, so only 2 fit
    assert_eq!(ctx.segments.len(), 2);
    assert_eq!(ctx.total_tokens, 80);
    assert_eq!(ctx.dropped_count, 1);
    assert_eq!(ctx.budget, 100);
    // Highest priority should be included
    assert_eq!(ctx.segments[0].label, "critical");
    assert_eq!(ctx.segments[1].label, "medium");
}

#[test]
fn context_assembler_includes_all_when_within_budget() {
    let assembler = ContextAssembler::new(1000);
    let segments = vec![
        ContextSegment::with_tokens("a", "content a", 3, 100),
        ContextSegment::with_tokens("b", "content b", 1, 100),
    ];

    let ctx = assembler.assemble(segments);
    assert_eq!(ctx.segments.len(), 2);
    assert_eq!(ctx.dropped_count, 0);
    assert_eq!(ctx.total_tokens, 200);
}

#[test]
fn context_assembler_render_produces_markdown() {
    let assembler = ContextAssembler::new(1000);
    let segments = vec![
        ContextSegment::with_tokens("History", "prior run failed", 2, 10),
        ContextSegment::with_tokens("Memory", "deploy config: rolling", 1, 10),
    ];

    let ctx = assembler.assemble(segments);
    let rendered = ctx.render();
    assert!(rendered.contains("## History"));
    assert!(rendered.contains("prior run failed"));
    assert!(rendered.contains("## Memory"));
}

// ---- Compaction Policy ----

#[test]
fn compaction_deletes_old_entries() {
    let now = Utc::now();
    let entries = vec![
        MemoryEntry {
            key: "old".to_string(),
            content: "ancient".to_string(),
            commit_id: "c1".to_string(),
            created_at: now - Duration::days(100),
        },
        MemoryEntry {
            key: "recent".to_string(),
            content: "fresh".to_string(),
            commit_id: "c2".to_string(),
            created_at: now - Duration::days(1),
        },
    ];

    let policy = CompactionPolicy::delete_older_than(30);
    let result = policy.compact(&entries);
    assert_eq!(result.retained.len(), 1);
    assert_eq!(result.retained[0].key, "recent");
    assert_eq!(result.compacted_count, 1);
}

#[test]
fn compaction_keeps_recent_per_key() {
    let now = Utc::now();
    let entries = vec![
        MemoryEntry {
            key: "deploy".to_string(),
            content: "v1".to_string(),
            commit_id: "c1".to_string(),
            created_at: now - Duration::hours(3),
        },
        MemoryEntry {
            key: "deploy".to_string(),
            content: "v2".to_string(),
            commit_id: "c2".to_string(),
            created_at: now - Duration::hours(2),
        },
        MemoryEntry {
            key: "deploy".to_string(),
            content: "v3".to_string(),
            commit_id: "c3".to_string(),
            created_at: now - Duration::hours(1),
        },
        MemoryEntry {
            key: "test".to_string(),
            content: "only one".to_string(),
            commit_id: "c1".to_string(),
            created_at: now,
        },
    ];

    let policy = CompactionPolicy::keep_recent(2);
    let result = policy.compact(&entries);
    // deploy: keep v2+v3 (newest 2), test: keep only one
    assert_eq!(result.retained.len(), 3);
    assert_eq!(result.compacted_count, 1);
    // Verify oldest deploy entry was dropped
    assert!(!result.retained.iter().any(|e| e.content == "v1"));
}

// ---- Artifact Persistence ----

#[test]
fn memory_context_artifact_round_trip() {
    let artifact = MemoryContextArtifact {
        run_id: "run-42".to_string(),
        index_size: 150,
        rationale_count: 5,
        context_tokens_used: 800,
        context_budget: 1000,
        compaction_applied: true,
        created_at: Utc::now(),
    };

    let dir = tempdir().expect("tempdir");
    let path = write_memory_context_artifact(&artifact, dir.path()).expect("write");
    assert!(path.exists());

    let loaded = read_memory_context_artifact("run-42", dir.path()).expect("read");
    assert_eq!(loaded.run_id, "run-42");
    assert_eq!(loaded.index_size, 150);
    assert_eq!(loaded.rationale_count, 5);
    assert!(loaded.compaction_applied);
}
