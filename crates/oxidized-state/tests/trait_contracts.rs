//! Trait contract tests for CasStore, RunLedger, and ReleaseRegistry.
//!
//! These tests verify the behavioral contracts of the storage traits
//! using in-memory fakes. Any conforming implementation must pass these.

use chrono::Utc;
use oxidized_state::fakes::{MemoryCasStore, MemoryReleaseRegistry, MemoryRunLedger};
use oxidized_state::storage_traits::*;
use oxidized_state::{StorageError, SurrealRunLedger};

// ===========================================================================
// CasStore contract tests
// ===========================================================================

#[tokio::test]
async fn cas_put_returns_correct_digest() {
    let store = MemoryCasStore::new();
    let data = b"hello world";
    let digest = store.put(data).await.unwrap();

    assert_eq!(digest, ContentDigest::from_bytes(data));
}

#[tokio::test]
async fn cas_get_round_trip() {
    let store = MemoryCasStore::new();
    let data = b"round trip data";
    let digest = store.put(data).await.unwrap();
    let retrieved = store.get(&digest).await.unwrap();

    assert_eq!(retrieved, data);
}

#[tokio::test]
async fn cas_get_not_found() {
    let store = MemoryCasStore::new();
    let bogus = ContentDigest::from_bytes(b"nonexistent data for bogus digest");
    let err = store.get(&bogus).await.unwrap_err();

    assert!(matches!(err, StorageError::NotFound { .. }));
}

#[tokio::test]
async fn cas_deduplicate_same_content() {
    let store = MemoryCasStore::new();
    let data = b"identical bytes";
    let d1 = store.put(data).await.unwrap();
    let d2 = store.put(data).await.unwrap();

    assert_eq!(d1, d2);
}

#[tokio::test]
async fn cas_different_content_different_digest() {
    let store = MemoryCasStore::new();
    let d1 = store.put(b"alpha").await.unwrap();
    let d2 = store.put(b"beta").await.unwrap();

    assert_ne!(d1, d2);
}

#[tokio::test]
async fn cas_contains_after_put() {
    let store = MemoryCasStore::new();
    let digest = store.put(b"check me").await.unwrap();

    assert!(store.contains(&digest).await.unwrap());
}

#[tokio::test]
async fn cas_contains_false_for_missing() {
    let store = MemoryCasStore::new();
    let bogus = ContentDigest::from_bytes(b"bogus missing content");

    assert!(!store.contains(&bogus).await.unwrap());
}

#[tokio::test]
async fn cas_delete_removes_content() {
    let store = MemoryCasStore::new();
    let digest = store.put(b"deletable").await.unwrap();
    store.delete(&digest).await.unwrap();

    assert!(!store.contains(&digest).await.unwrap());
    assert!(store.get(&digest).await.is_err());
}

#[tokio::test]
async fn cas_delete_noop_for_missing() {
    let store = MemoryCasStore::new();
    let bogus = ContentDigest::from_bytes(b"another bogus content for delete test");
    // Should not error
    store.delete(&bogus).await.unwrap();
}

#[tokio::test]
async fn cas_preserves_binary_data() {
    let store = MemoryCasStore::new();
    let data: Vec<u8> = (0u8..=255).collect();
    let digest = store.put(&data).await.unwrap();
    let retrieved = store.get(&digest).await.unwrap();

    assert_eq!(retrieved, data);
}

// ===========================================================================
// RunLedger contract tests
// ===========================================================================

fn sample_metadata() -> RunMetadata {
    RunMetadata {
        git_sha: Some("abc123".to_string()),
        agent_name: "test-agent".to_string(),
        tags: serde_json::json!({"env": "test"}),
        evaluation: Default::default(),
    }
}

fn sample_event(seq: u64, kind: &str) -> RunEvent {
    RunEvent {
        seq,
        kind: kind.to_string(),
        payload: serde_json::json!({"detail": kind}),
        timestamp: Utc::now(),
    }
}

fn sample_summary(total_events: u64, success: bool) -> RunSummary {
    RunSummary {
        total_events,
        final_state_digest: None,
        duration_ms: 100,
        success,
    }
}

#[tokio::test]
async fn ledger_create_run_returns_unique_ids() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");

    let id1 = ledger.create_run(&spec, sample_metadata()).await.unwrap();
    let id2 = ledger.create_run(&spec, sample_metadata()).await.unwrap();

    assert_ne!(id1, id2);
}

#[tokio::test]
async fn ledger_get_run_returns_created_run() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

    let record = ledger.get_run(&run_id).await.unwrap();
    assert_eq!(record.run_id, run_id);
    assert_eq!(record.spec_digest, spec);
    assert_eq!(record.status, RunStatus::Running);
    assert!(record.summary.is_none());
}

#[tokio::test]
async fn ledger_get_run_not_found() {
    let ledger = MemoryRunLedger::new();
    let bogus = RunId("nonexistent".to_string());
    let err = ledger.get_run(&bogus).await.unwrap_err();

    assert!(matches!(err, StorageError::RunNotFound { .. }));
}

#[tokio::test]
async fn ledger_append_and_get_events_ordered() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

    // Append out of order
    ledger
        .append_event(&run_id, sample_event(2, "node_exited"))
        .await
        .unwrap();
    ledger
        .append_event(&run_id, sample_event(1, "node_entered"))
        .await
        .unwrap();
    ledger
        .append_event(&run_id, sample_event(3, "graph_completed"))
        .await
        .unwrap();

    let events = ledger.get_events(&run_id).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
}

#[tokio::test]
async fn ledger_complete_run_sets_status() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

    ledger
        .complete_run(&run_id, sample_summary(0, true))
        .await
        .unwrap();

    let record = ledger.get_run(&run_id).await.unwrap();
    assert_eq!(record.status, RunStatus::Completed);
    assert!(record.summary.is_some());
    assert!(record.completed_at.is_some());
}

#[tokio::test]
async fn ledger_fail_run_sets_status() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

    ledger
        .fail_run(&run_id, sample_summary(0, false))
        .await
        .unwrap();

    let record = ledger.get_run(&run_id).await.unwrap();
    assert_eq!(record.status, RunStatus::Failed);
}

#[tokio::test]
async fn ledger_cannot_append_to_completed_run() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();
    ledger
        .complete_run(&run_id, sample_summary(0, true))
        .await
        .unwrap();

    let err = ledger
        .append_event(&run_id, sample_event(1, "late"))
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidRunState { .. }));
}

#[tokio::test]
async fn ledger_cannot_complete_twice() {
    let ledger = MemoryRunLedger::new();
    let spec = ContentDigest::from_bytes(b"spec");
    let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();
    ledger
        .complete_run(&run_id, sample_summary(0, true))
        .await
        .unwrap();

    let err = ledger
        .complete_run(&run_id, sample_summary(0, true))
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidRunState { .. }));
}

#[tokio::test]
async fn ledger_list_runs_all() {
    let ledger = MemoryRunLedger::new();
    let spec_a = ContentDigest::from_bytes(b"spec-a");
    let spec_b = ContentDigest::from_bytes(b"spec-b");

    ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
    ledger.create_run(&spec_b, sample_metadata()).await.unwrap();

    let all = ledger.list_runs(None).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn ledger_list_runs_filtered_by_spec() {
    let ledger = MemoryRunLedger::new();
    let spec_a = ContentDigest::from_bytes(b"spec-a");
    let spec_b = ContentDigest::from_bytes(b"spec-b");

    ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
    ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
    ledger.create_run(&spec_b, sample_metadata()).await.unwrap();

    let filtered = ledger.list_runs(Some(&spec_a)).await.unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(filtered.iter().all(|r| r.spec_digest == spec_a));
}

// ===========================================================================
// ReleaseRegistry contract tests
// ===========================================================================

fn sample_release_meta(label: &str) -> ReleaseMetadata {
    ReleaseMetadata {
        version_label: Some(label.to_string()),
        promoted_by: "ci".to_string(),
        notes: None,
    }
}

#[tokio::test]
async fn registry_promote_creates_release() {
    let reg = MemoryReleaseRegistry::new();
    let digest = ContentDigest::from_bytes(b"v1-spec");

    let release = reg
        .promote("my-agent", &digest, sample_release_meta("v1.0.0"))
        .await
        .unwrap();

    assert_eq!(release.name, "my-agent");
    assert_eq!(release.spec_digest, digest);
}

#[tokio::test]
async fn registry_current_returns_latest() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");

    reg.promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    reg.promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();

    let current = reg.current("agent").await.unwrap().unwrap();
    assert_eq!(current.spec_digest, d2);
}

#[tokio::test]
async fn registry_current_returns_none_for_unknown() {
    let reg = MemoryReleaseRegistry::new();
    let current = reg.current("unknown").await.unwrap();
    assert!(current.is_none());
}

#[tokio::test]
async fn registry_history_newest_first() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");
    let d3 = ContentDigest::from_bytes(b"v3");

    reg.promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    reg.promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();
    reg.promote("agent", &d3, sample_release_meta("v3"))
        .await
        .unwrap();

    let history = reg.history("agent").await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].spec_digest, d3); // newest first
    assert_eq!(history[2].spec_digest, d1); // oldest last
}

#[tokio::test]
async fn registry_history_empty_for_unknown() {
    let reg = MemoryReleaseRegistry::new();
    let history = reg.history("unknown").await.unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn registry_rollback_restores_previous() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");

    reg.promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    reg.promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();

    let rolled_back = reg.rollback("agent").await.unwrap();
    assert_eq!(rolled_back.spec_digest, d1);

    // Current should now be v1
    let current = reg.current("agent").await.unwrap().unwrap();
    assert_eq!(current.spec_digest, d1);
}

#[tokio::test]
async fn registry_rollback_fails_with_single_release() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"v1");
    reg.promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();

    let err = reg.rollback("agent").await.unwrap_err();
    assert!(matches!(err, StorageError::NoPreviousRelease { .. }));
}

#[tokio::test]
async fn registry_rollback_fails_for_unknown() {
    let reg = MemoryReleaseRegistry::new();
    let err = reg.rollback("nonexistent").await.unwrap_err();
    assert!(matches!(err, StorageError::ReleaseNotFound { .. }));
}

#[tokio::test]
async fn registry_history_append_only() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");
    let d3 = ContentDigest::from_bytes(b"v3");

    reg.promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    reg.promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();
    reg.promote("agent", &d3, sample_release_meta("v3"))
        .await
        .unwrap();

    // Rollback re-appends the previous release (append-only audit trail)
    let rolled_back = reg.rollback("agent").await.unwrap();
    assert_eq!(rolled_back.spec_digest, d2);

    // History preserves full audit trail: 4 entries (v2 re-appended), newest first
    let history = reg.history("agent").await.unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].spec_digest, d2); // rollback entry (current)
    assert_eq!(history[1].spec_digest, d3); // original v3 promotion
    assert_eq!(history[2].spec_digest, d2); // original v2 promotion
    assert_eq!(history[3].spec_digest, d1); // original v1 promotion
}

#[tokio::test]
async fn registry_promotes_independent_agents() {
    let reg = MemoryReleaseRegistry::new();
    let d1 = ContentDigest::from_bytes(b"spec-a");
    let d2 = ContentDigest::from_bytes(b"spec-b");

    reg.promote("agent-a", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    reg.promote("agent-b", &d2, sample_release_meta("v1"))
        .await
        .unwrap();

    let a = reg.current("agent-a").await.unwrap().unwrap();
    let b = reg.current("agent-b").await.unwrap().unwrap();
    assert_eq!(a.spec_digest, d1);
    assert_eq!(b.spec_digest, d2);
}

// ===========================================================================
// SurrealRunLedger contract tests (mirrors MemoryRunLedger tests above)
// ===========================================================================

mod surreal_ledger_tests {
    use super::*;

    async fn ledger() -> impl RunLedger {
        SurrealRunLedger::in_memory()
            .await
            .expect("in_memory() failed")
    }

    #[tokio::test]
    async fn create_run_returns_unique_ids() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");

        let id1 = ledger.create_run(&spec, sample_metadata()).await.unwrap();
        let id2 = ledger.create_run(&spec, sample_metadata()).await.unwrap();

        assert_ne!(id1, id2);
    }

    #[tokio::test]
    async fn get_run_returns_created_run() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

        let record = ledger.get_run(&run_id).await.unwrap();
        assert_eq!(record.run_id, run_id);
        assert_eq!(record.spec_digest, spec);
        assert_eq!(record.status, RunStatus::Running);
        assert!(record.summary.is_none());
    }

    #[tokio::test]
    async fn get_run_not_found() {
        let ledger = ledger().await;
        let bogus = RunId("nonexistent".to_string());
        let err = ledger.get_run(&bogus).await.unwrap_err();

        assert!(matches!(err, StorageError::RunNotFound { .. }));
    }

    #[tokio::test]
    async fn append_and_get_events_ordered() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

        ledger
            .append_event(&run_id, sample_event(2, "node_exited"))
            .await
            .unwrap();
        ledger
            .append_event(&run_id, sample_event(1, "node_entered"))
            .await
            .unwrap();
        ledger
            .append_event(&run_id, sample_event(3, "graph_completed"))
            .await
            .unwrap();

        let events = ledger.get_events(&run_id).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(events[2].seq, 3);
    }

    #[tokio::test]
    async fn complete_run_sets_status() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

        ledger
            .complete_run(&run_id, sample_summary(0, true))
            .await
            .unwrap();

        let record = ledger.get_run(&run_id).await.unwrap();
        assert_eq!(record.status, RunStatus::Completed);
        assert!(record.summary.is_some());
        assert!(record.completed_at.is_some());
    }

    #[tokio::test]
    async fn fail_run_sets_status() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();

        ledger
            .fail_run(&run_id, sample_summary(0, false))
            .await
            .unwrap();

        let record = ledger.get_run(&run_id).await.unwrap();
        assert_eq!(record.status, RunStatus::Failed);
    }

    #[tokio::test]
    async fn cannot_append_to_completed_run() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();
        ledger
            .complete_run(&run_id, sample_summary(0, true))
            .await
            .unwrap();

        let err = ledger
            .append_event(&run_id, sample_event(1, "late"))
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidRunState { .. }));
    }

    #[tokio::test]
    async fn cannot_complete_twice() {
        let ledger = ledger().await;
        let spec = ContentDigest::from_bytes(b"spec");
        let run_id = ledger.create_run(&spec, sample_metadata()).await.unwrap();
        ledger
            .complete_run(&run_id, sample_summary(0, true))
            .await
            .unwrap();

        let err = ledger
            .complete_run(&run_id, sample_summary(0, true))
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidRunState { .. }));
    }

    #[tokio::test]
    async fn list_runs_all() {
        let ledger = ledger().await;
        let spec_a = ContentDigest::from_bytes(b"spec-a");
        let spec_b = ContentDigest::from_bytes(b"spec-b");

        ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
        ledger.create_run(&spec_b, sample_metadata()).await.unwrap();

        let all = ledger.list_runs(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn list_runs_filtered_by_spec() {
        let ledger = ledger().await;
        let spec_a = ContentDigest::from_bytes(b"spec-a");
        let spec_b = ContentDigest::from_bytes(b"spec-b");

        ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
        ledger.create_run(&spec_a, sample_metadata()).await.unwrap();
        ledger.create_run(&spec_b, sample_metadata()).await.unwrap();

        let filtered = ledger.list_runs(Some(&spec_a)).await.unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|r| r.spec_digest == spec_a));
    }
}
