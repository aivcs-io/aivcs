//! Semantic-RAG-Merge: Semantic Merging with RAG and LLM Arbiter
//!
//! This crate provides the semantic version control features for AIVCS,
//! allowing intelligent merging of divergent agent states and memory.
//!
//! ## Layer 3 - VCS Logic
//!
//! Focus: Semantic conflict resolution and memory synthesis.

use anyhow::Result;
use oxidized_state::{CommitId, EdgeType, MemoryRecord, SurrealHandle};
use serde::{Deserialize, Serialize};

/// Difference between two memory vector stores
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStoreDelta {
    /// Memories only in commit A
    pub only_in_a: Vec<MemoryRecord>,
    /// Memories only in commit B
    pub only_in_b: Vec<MemoryRecord>,
    /// Memories that are identical in both A and B
    pub identical: Vec<MemoryRecord>,
    /// Memories that differ between A and B (same key, different content)
    pub conflicts: Vec<MemoryConflict>,
}

/// A conflict between two memory records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConflict {
    /// Memory key
    pub key: String,
    /// Memory from commit A
    pub memory_a: MemoryRecord,
    /// Memory from commit B
    pub memory_b: MemoryRecord,
}

/// Result of automatic conflict resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoResolvedValue {
    /// The resolved value
    pub value: String,
    /// Which branch the resolution favored (if any)
    pub favored_branch: Option<String>,
    /// Reasoning for the resolution
    pub reasoning: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

/// Result of a semantic merge operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    /// The new merge commit ID
    pub merge_commit_id: CommitId,
    /// Number of automatic resolutions
    pub auto_resolved: usize,
    /// Any conflicts that couldn't be auto-resolved
    pub manual_conflicts: Vec<MemoryConflict>,
    /// Summary of the merge
    pub summary: String,
}

/// Diff memory vectors between two commits
///
/// # TDD: test_memory_diff_shows_only_new_vectors
pub async fn diff_memory_vectors(
    handle: &SurrealHandle,
    commit_a: &str,
    commit_b: &str,
) -> Result<VectorStoreDelta> {
    let memories_a = handle.get_memories(commit_a).await?;
    let memories_b = handle.get_memories(commit_b).await?;

    let keys_a: std::collections::HashSet<_> = memories_a.iter().map(|m| &m.key).collect();
    let keys_b: std::collections::HashSet<_> = memories_b.iter().map(|m| &m.key).collect();

    let only_in_a: Vec<_> = memories_a
        .iter()
        .filter(|m| !keys_b.contains(&m.key))
        .cloned()
        .collect();

    let only_in_b: Vec<_> = memories_b
        .iter()
        .filter(|m| !keys_a.contains(&m.key))
        .cloned()
        .collect();

    // Find conflicts (same key, different content) and identical memories
    let mut conflicts = Vec::new();
    let mut identical = Vec::new();
    for mem_a in &memories_a {
        if let Some(mem_b) = memories_b.iter().find(|m| m.key == mem_a.key) {
            if mem_a.content != mem_b.content {
                conflicts.push(MemoryConflict {
                    key: mem_a.key.clone(),
                    memory_a: mem_a.clone(),
                    memory_b: mem_b.clone(),
                });
            } else {
                identical.push(mem_a.clone());
            }
        }
    }

    Ok(VectorStoreDelta {
        only_in_a,
        only_in_b,
        identical,
        conflicts,
    })
}

/// Resolve a state conflict using multi-signal heuristic scoring.
///
/// Scores each branch across: content length, recency, and metadata
/// richness. The branch with the higher total score wins. Confidence
/// is derived from how decisively one branch outscores the other.
///
/// # Future: Replace with LLM-based chain-of-thought arbiter when
/// an LLM endpoint is available, using trace_a/trace_b for context.
///
/// # TDD: test_arbiter_resolves_value_conflict_based_on_CoT
pub async fn resolve_conflict_state(
    _trace_a: &[serde_json::Value],
    _trace_b: &[serde_json::Value],
    conflict: &MemoryConflict,
) -> Result<AutoResolvedValue> {
    let score_a = conflict_score(&conflict.memory_a);
    let score_b = conflict_score(&conflict.memory_b);
    let total = score_a + score_b;

    let (winner, _loser_score, label) = if score_a >= score_b {
        (&conflict.memory_a, score_b, "A")
    } else {
        (&conflict.memory_b, score_a, "B")
    };

    // Confidence: how decisive is the margin?
    // - Equal scores → 0.5 (coin-flip)
    // - One signal dominant → ~0.7
    // - All signals agree → ~0.85
    let confidence = if total == 0.0 {
        0.5
    } else {
        let margin = (score_a - score_b).abs() / total;
        // Map margin [0, 1] → confidence [0.5, 0.85]
        0.5 + margin * 0.35
    };

    let mut reasons = Vec::new();
    let a_len = conflict.memory_a.content.len();
    let b_len = conflict.memory_b.content.len();
    if a_len != b_len {
        reasons.push(format!("content length: A={} B={} chars", a_len, b_len));
    }
    if conflict.memory_a.created_at != conflict.memory_b.created_at {
        reasons.push(format!(
            "recency: A={} B={}",
            conflict.memory_a.created_at.format("%Y-%m-%dT%H:%M:%S"),
            conflict.memory_b.created_at.format("%Y-%m-%dT%H:%M:%S"),
        ));
    }
    let meta_a = metadata_field_count(&conflict.memory_a.metadata);
    let meta_b = metadata_field_count(&conflict.memory_b.metadata);
    if meta_a != meta_b {
        reasons.push(format!("metadata fields: A={meta_a} B={meta_b}"));
    }

    let reasoning = if reasons.is_empty() {
        format!("Chose branch {label}: identical signals, defaulting to A")
    } else {
        format!("Chose branch {label}: {}", reasons.join(", "))
    };

    Ok(AutoResolvedValue {
        value: winner.content.clone(),
        favored_branch: Some(label.to_string()),
        reasoning,
        confidence: (confidence * 100.0).round() / 100.0,
    })
}

/// Score a memory record for conflict resolution.
///
/// Higher score = stronger candidate. Signals:
/// 1. Content length (more detail is generally better)
/// 2. Recency (newer memories reflect latest state)
/// 3. Metadata richness (more context is better)
fn conflict_score(record: &MemoryRecord) -> f32 {
    conflict_score_at(record, chrono::Utc::now())
}

/// Same as `conflict_score` but with the "now" reference injected, so the
/// function is pure and unit-testable.
fn conflict_score_at(record: &MemoryRecord, now: chrono::DateTime<chrono::Utc>) -> f32 {
    let length_score = (record.content.len() as f32).ln().max(0.0);
    // Recency: bounded in [0, 1], decays with age. ~1.0 for "just now",
    // ~0.5 at 30 days, ~0.1 at ~9 months. The previous version divided a
    // raw Unix-seconds timestamp by 1e9, which produced a near-constant
    // ~1.7 for every recently-created memory — i.e. zero discriminative
    // power between any two candidates.
    let age_secs = (now - record.created_at).num_seconds().max(0) as f32;
    let recency_score = 1.0 / (1.0 + age_secs / (30.0 * 86_400.0));
    let meta_score = metadata_field_count(&record.metadata) as f32;
    length_score + recency_score + meta_score
}

/// Count the number of fields in a JSON metadata value.
fn metadata_field_count(meta: &serde_json::Value) -> usize {
    match meta {
        serde_json::Value::Object(map) => map.len(),
        _ => 0,
    }
}

/// Synthesize two memory stores into one
///
/// # TDD: test_merge_synthesizes_two_memories_into_one_new_commit
pub async fn synthesize_memory(
    handle: &SurrealHandle,
    commit_a: &str,
    commit_b: &str,
    new_commit_id: &str,
) -> Result<Vec<MemoryRecord>> {
    let delta = diff_memory_vectors(handle, commit_a, commit_b).await?;

    let mut merged_memories = Vec::new();

    // Include all memories unique to A
    for mut mem in delta.only_in_a {
        mem.commit_id = new_commit_id.to_string();
        mem.id = None;
        merged_memories.push(mem);
    }

    // Include all memories unique to B
    for mut mem in delta.only_in_b {
        mem.commit_id = new_commit_id.to_string();
        mem.id = None;
        merged_memories.push(mem);
    }

    // Include identical memories
    for mut mem in delta.identical {
        mem.commit_id = new_commit_id.to_string();
        mem.id = None;
        merged_memories.push(mem);
    }

    // Resolve conflicts
    for conflict in delta.conflicts {
        let resolved = resolve_conflict_state(&[], &[], &conflict).await?;
        let merged_mem = MemoryRecord::new(new_commit_id, &conflict.key, &resolved.value)
            .with_metadata(serde_json::json!({
                "merged_from": [commit_a, commit_b],
                "resolution": resolved.reasoning,
                "confidence": resolved.confidence,
            }));
        merged_memories.push(merged_mem);
    }

    Ok(merged_memories)
}

/// Perform a semantic merge of two branches
pub async fn semantic_merge(
    handle: &SurrealHandle,
    commit_a: &str,
    commit_b: &str,
    message: &str,
    author: &str,
) -> Result<MergeResult> {
    // Create the merge commit ID
    let state_data = format!("merge:{}:{}", commit_a, commit_b);
    let merge_commit_id = CommitId::from_state(state_data.as_bytes());

    // Synthesize memories
    let merged_memories =
        synthesize_memory(handle, commit_a, commit_b, &merge_commit_id.hash).await?;

    // Save merged memories
    for mem in &merged_memories {
        handle.save_memory(mem).await?;
    }

    // Save merge snapshot so load_snapshot(merge_commit_id) works
    let merge_state = serde_json::json!({
        "merged_from": [commit_a, commit_b],
        "memory_count": merged_memories.len(),
    });
    handle.save_snapshot(&merge_commit_id, merge_state).await?;

    // Create merge commit record
    let commit = oxidized_state::CommitRecord::new(
        merge_commit_id.clone(),
        vec![commit_a.to_string(), commit_b.to_string()],
        message,
        author,
    );
    handle.save_commit(&commit).await?;

    // Save graph edges for both parents (typed as merge edges)
    handle
        .save_commit_graph_edge_typed(&merge_commit_id.hash, commit_a, EdgeType::Merge)
        .await?;
    handle
        .save_commit_graph_edge_typed(&merge_commit_id.hash, commit_b, EdgeType::Merge)
        .await?;

    // Get delta for summary
    let delta = diff_memory_vectors(handle, commit_a, commit_b).await?;

    Ok(MergeResult {
        merge_commit_id,
        auto_resolved: delta.conflicts.len(),
        manual_conflicts: vec![], // All resolved automatically for now
        summary: format!(
            "Merged {} memories from A, {} from B, resolved {} conflicts",
            delta.only_in_a.len(),
            delta.only_in_b.len(),
            delta.conflicts.len()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_diff_shows_only_new_vectors() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        // Create memories for commit A
        let mem_a1 = MemoryRecord::new("commit-a", "shared-key", "shared content");
        let mem_a2 = MemoryRecord::new("commit-a", "only-a-key", "only in A");
        handle.save_memory(&mem_a1).await.unwrap();
        handle.save_memory(&mem_a2).await.unwrap();

        // Create memories for commit B
        let mem_b1 = MemoryRecord::new("commit-b", "shared-key", "shared content");
        let mem_b2 = MemoryRecord::new("commit-b", "only-b-key", "only in B");
        handle.save_memory(&mem_b1).await.unwrap();
        handle.save_memory(&mem_b2).await.unwrap();

        let delta = diff_memory_vectors(&handle, "commit-a", "commit-b")
            .await
            .unwrap();

        assert_eq!(delta.only_in_a.len(), 1);
        assert_eq!(delta.only_in_a[0].key, "only-a-key");

        assert_eq!(delta.only_in_b.len(), 1);
        assert_eq!(delta.only_in_b[0].key, "only-b-key");

        assert_eq!(delta.conflicts.len(), 0); // Same content = no conflict
    }

    #[tokio::test]
    async fn test_memory_diff_detects_conflicts() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let mem_a = MemoryRecord::new("commit-a", "conflict-key", "content version A");
        let mem_b = MemoryRecord::new("commit-b", "conflict-key", "content version B");
        handle.save_memory(&mem_a).await.unwrap();
        handle.save_memory(&mem_b).await.unwrap();

        let delta = diff_memory_vectors(&handle, "commit-a", "commit-b")
            .await
            .unwrap();

        assert_eq!(delta.conflicts.len(), 1);
        assert_eq!(delta.conflicts[0].key, "conflict-key");
    }

    #[test]
    fn test_recency_discriminates_between_memories() {
        // Two memories with identical content/metadata but different ages.
        // Without the fix, recency_score divides Unix seconds by 1e9, so
        // any two recent memories score ~equally on recency. With the fix,
        // a newer memory must score strictly higher than an older one.
        let now = chrono::Utc::now();
        let old_record = MemoryRecord {
            id: None,
            commit_id: "c".to_string(),
            key: "k".to_string(),
            content: "same".to_string(),
            embedding: None,
            metadata: serde_json::Value::Null,
            created_at: now - chrono::Duration::days(60),
        };
        let new_record = MemoryRecord {
            id: None,
            commit_id: "c".to_string(),
            key: "k".to_string(),
            content: "same".to_string(),
            embedding: None,
            metadata: serde_json::Value::Null,
            created_at: now,
        };

        let old_score = conflict_score_at(&old_record, now);
        let new_score = conflict_score_at(&new_record, now);

        assert!(
            new_score > old_score,
            "newer memory must score higher; got new={new_score} old={old_score}",
        );
        // And the gap should be meaningful (≥0.3, since recency is bounded
        // in [0, 1] and 60 days ago gives ~0.33 while now gives ~1.0).
        assert!(
            (new_score - old_score) > 0.3,
            "recency gap too small: new={new_score} old={old_score}",
        );
    }

    #[tokio::test]
    async fn test_arbiter_resolves_value_conflict_based_on_cot() {
        let conflict = MemoryConflict {
            key: "test-key".to_string(),
            memory_a: MemoryRecord::new("a", "test-key", "short"),
            memory_b: MemoryRecord::new("b", "test-key", "longer content here"),
        };

        let resolved = resolve_conflict_state(&[], &[], &conflict).await.unwrap();

        assert!(resolved.confidence > 0.0);
        assert!(resolved.favored_branch.is_some());
        assert!(!resolved.reasoning.is_empty());
    }

    #[tokio::test]
    async fn test_merge_synthesizes_two_memories_into_one_new_commit() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        // Create commit IDs
        let commit_id_a = oxidized_state::CommitId::from_state(b"branch-a");
        let commit_id_b = oxidized_state::CommitId::from_state(b"branch-b");

        // Create commits with divergent memories
        let commit_a = oxidized_state::CommitRecord::new(
            commit_id_a.clone(),
            vec![],
            "Branch A commit",
            "agent-a",
        );
        handle.save_commit(&commit_a).await.unwrap();

        let commit_b = oxidized_state::CommitRecord::new(
            commit_id_b.clone(),
            vec![],
            "Branch B commit",
            "agent-b",
        );
        handle.save_commit(&commit_b).await.unwrap();

        // Add unique memories to each branch
        let mem_a_only =
            MemoryRecord::new(&commit_id_a.hash, "learned-from-a", "Strategy A knowledge");
        let mem_b_only =
            MemoryRecord::new(&commit_id_b.hash, "learned-from-b", "Strategy B knowledge");
        let mem_conflict_a = MemoryRecord::new(&commit_id_a.hash, "shared-key", "short");
        let mem_conflict_b = MemoryRecord::new(
            &commit_id_b.hash,
            "shared-key",
            "longer and more detailed content",
        );

        handle.save_memory(&mem_a_only).await.unwrap();
        handle.save_memory(&mem_b_only).await.unwrap();
        handle.save_memory(&mem_conflict_a).await.unwrap();
        handle.save_memory(&mem_conflict_b).await.unwrap();

        // Perform semantic merge
        let result = semantic_merge(
            &handle,
            &commit_id_a.hash,
            &commit_id_b.hash,
            "Merge A and B",
            "agent-git",
        )
        .await
        .unwrap();

        // Verify merge commit was created
        assert!(!result.merge_commit_id.hash.is_empty());

        // Verify all memories were synthesized
        let merged_memories = handle
            .get_memories(&result.merge_commit_id.hash)
            .await
            .unwrap();

        // Should have 3 memories: 2 unique + 1 resolved conflict
        assert_eq!(merged_memories.len(), 3, "Expected 3 merged memories");

        // Verify unique memories were preserved
        let keys: Vec<_> = merged_memories.iter().map(|m| m.key.as_str()).collect();
        assert!(
            keys.contains(&"learned-from-a"),
            "Missing memory from branch A"
        );
        assert!(
            keys.contains(&"learned-from-b"),
            "Missing memory from branch B"
        );
        assert!(keys.contains(&"shared-key"), "Missing resolved conflict");

        // Verify conflict was resolved (longer content should win with heuristic)
        let resolved = merged_memories
            .iter()
            .find(|m| m.key == "shared-key")
            .unwrap();
        assert!(
            resolved.content.contains("longer") || resolved.content.contains("detailed"),
            "Conflict resolution should favor more detailed content"
        );

        // Verify summary is informative
        assert!(
            result.summary.contains("2") || result.summary.contains("memories"),
            "Summary should mention merged memories"
        );
    }
}
