//! Compaction policies for pruning stale or low-value memory entries.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::error::MemoryResult;
use super::index::MemoryIndex;

/// Policy controlling which entries are eligible for compaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionPolicy {
    /// Remove entries older than this many days.
    pub max_age_days: Option<u64>,
    /// Keep at most this many entries (oldest removed first).
    pub max_entries: Option<usize>,
    /// Remove entries with fewer tokens than this threshold.
    pub min_token_threshold: Option<usize>,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            max_age_days: Some(90),
            max_entries: Some(1000),
            min_token_threshold: None,
        }
    }
}

/// Result of a compaction pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionResult {
    pub removed_count: usize,
    pub remaining_count: usize,
    pub removed_ids: Vec<String>,
}

/// Apply compaction policies to a memory index, removing entries in order:
/// 1. Below min token threshold
/// 2. Older than max age
/// 3. Excess entries beyond max count (oldest first)
pub fn compact_index(
    index: &mut MemoryIndex,
    policy: &CompactionPolicy,
) -> MemoryResult<CompactionResult> {
    let mut removed_ids = Vec::new();

    // Phase 1: Remove entries below min token threshold
    if let Some(min_tokens) = policy.min_token_threshold {
        let to_remove: Vec<String> = index
            .entries_mut()
            .iter()
            .filter(|(_, e)| e.token_estimate < min_tokens)
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_remove {
            index.entries_mut().remove(&id);
            removed_ids.push(id);
        }
    }

    // Phase 2: Remove entries older than max_age_days
    if let Some(max_age_days) = policy.max_age_days {
        let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
        let to_remove: Vec<String> = index
            .entries_mut()
            .iter()
            .filter(|(_, e)| e.created_at < cutoff)
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_remove {
            index.entries_mut().remove(&id);
            removed_ids.push(id);
        }
    }

    // Phase 3: Trim to max_entries (remove oldest first)
    if let Some(max_entries) = policy.max_entries {
        if index.len() > max_entries {
            let mut entries_by_age: Vec<(String, chrono::DateTime<Utc>)> = index
                .entries_mut()
                .iter()
                .map(|(id, e)| (id.clone(), e.created_at))
                .collect();
            // Sort oldest first, then id for deterministic tie-breaking.
            entries_by_age
                .sort_by(|(id_a, ts_a), (id_b, ts_b)| ts_a.cmp(ts_b).then_with(|| id_a.cmp(id_b)));

            let to_remove_count = index.len() - max_entries;
            for (id, _) in entries_by_age.into_iter().take(to_remove_count) {
                index.entries_mut().remove(&id);
                removed_ids.push(id);
            }
        }
    }

    Ok(CompactionResult {
        removed_count: removed_ids.len(),
        remaining_count: index.len(),
        removed_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::index::{MemoryEntry, MemoryEntryKind, MemoryIndex};
    use chrono::Duration;

    fn entry(id: &str, age_days: i64, tokens: usize) -> MemoryEntry {
        MemoryEntry {
            id: id.into(),
            kind: MemoryEntryKind::RunTrace,
            summary: format!("s {id}"),
            content_digest: format!("d_{id}"),
            created_at: Utc::now() - Duration::days(age_days),
            tags: Vec::new(),
            token_estimate: tokens,
            relevance: 0.5,
        }
    }

    #[test]
    fn test_noop_policy() {
        let mut idx = MemoryIndex::new();
        idx.insert(entry("a", 1, 100)).unwrap();
        let r = compact_index(
            &mut idx,
            &CompactionPolicy {
                max_age_days: None,
                max_entries: None,
                min_token_threshold: None,
            },
        )
        .unwrap();
        assert_eq!(r.removed_count, 0);
        assert_eq!(r.remaining_count, 1);
    }

    #[test]
    fn test_age_removal() {
        let mut idx = MemoryIndex::new();
        idx.insert(entry("new", 1, 100)).unwrap();
        idx.insert(entry("old", 100, 100)).unwrap();
        let r = compact_index(
            &mut idx,
            &CompactionPolicy {
                max_age_days: Some(30),
                max_entries: None,
                min_token_threshold: None,
            },
        )
        .unwrap();
        assert_eq!(r.removed_count, 1);
        assert!(r.removed_ids.contains(&"old".to_string()));
    }

    #[test]
    fn test_count_trimming() {
        let mut idx = MemoryIndex::new();
        for i in 0..5 {
            idx.insert(entry(&format!("e{i}"), i, 100)).unwrap();
        }
        let r = compact_index(
            &mut idx,
            &CompactionPolicy {
                max_age_days: None,
                max_entries: Some(3),
                min_token_threshold: None,
            },
        )
        .unwrap();
        assert_eq!(r.removed_count, 2);
        assert_eq!(r.remaining_count, 3);
    }

    #[test]
    fn test_count_trimming_deterministic_with_equal_timestamps() {
        let now = Utc::now();
        let mut idx = MemoryIndex::new();
        for id in &["b", "a", "c"] {
            idx.insert(MemoryEntry {
                id: (*id).to_string(),
                kind: MemoryEntryKind::RunTrace,
                summary: format!("s {id}"),
                content_digest: format!("d_{id}"),
                created_at: now,
                tags: Vec::new(),
                token_estimate: 100,
                relevance: 0.5,
            })
            .unwrap();
        }

        let r = compact_index(
            &mut idx,
            &CompactionPolicy {
                max_age_days: None,
                max_entries: Some(1),
                min_token_threshold: None,
            },
        )
        .unwrap();

        assert_eq!(r.removed_ids, vec!["a".to_string(), "b".to_string()]);
        assert!(idx.get("c").is_ok());
    }
}
