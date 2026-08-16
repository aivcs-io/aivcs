//! In-memory index for memory entries with tag, kind, and time filtering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::error::{MemoryError, MemoryResult};

/// The kind of memory entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryKind {
    RunTrace,
    Rationale,
    Diff,
    Snapshot,
    ToolResult,
}

impl std::fmt::Display for MemoryEntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RunTrace => write!(f, "run_trace"),
            Self::Rationale => write!(f, "rationale"),
            Self::Diff => write!(f, "diff"),
            Self::Snapshot => write!(f, "snapshot"),
            Self::ToolResult => write!(f, "tool_result"),
        }
    }
}

/// A single entry in the memory index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub kind: MemoryEntryKind,
    pub summary: String,
    pub content_digest: String,
    pub created_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub token_estimate: usize,
    pub relevance: f64,
}

/// Query parameters for searching the memory index.
#[derive(Debug, Clone, Default)]
pub struct IndexQuery {
    pub kind: Option<MemoryEntryKind>,
    pub tag: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

impl IndexQuery {
    /// Query that matches all entries.
    pub fn all() -> Self {
        Self::default()
    }

    pub fn with_kind(mut self, kind: MemoryEntryKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = Some(tag.to_string());
        self
    }

    pub fn after(mut self, after: DateTime<Utc>) -> Self {
        self.after = Some(after);
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Result of an index query.
#[derive(Debug, Clone)]
pub struct IndexResult {
    pub entries: Vec<MemoryEntry>,
    pub total_matches: usize,
}

/// In-memory index of memory entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIndex {
    entries: HashMap<String, MemoryEntry>,
}

impl MemoryIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert an entry. Returns error if id already exists.
    pub fn insert(&mut self, entry: MemoryEntry) -> MemoryResult<()> {
        if self.entries.contains_key(&entry.id) {
            return Err(MemoryError::DuplicateEntry { id: entry.id });
        }
        self.entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    /// Get an entry by id.
    pub fn get(&self, id: &str) -> MemoryResult<&MemoryEntry> {
        self.entries
            .get(id)
            .ok_or_else(|| MemoryError::EntryNotFound { id: id.into() })
    }

    /// Remove an entry by id.
    pub fn remove(&mut self, id: &str) -> MemoryResult<MemoryEntry> {
        self.entries
            .remove(id)
            .ok_or_else(|| MemoryError::EntryNotFound { id: id.into() })
    }

    /// Mutable access to all entries (used by compaction).
    pub fn entries_mut(&mut self) -> &mut HashMap<String, MemoryEntry> {
        &mut self.entries
    }

    /// Query the index with filters. Results sorted newest-first.
    pub fn query(&self, q: &IndexQuery) -> IndexResult {
        let mut matches: Vec<MemoryEntry> = self
            .entries
            .values()
            .filter(|e| {
                if let Some(ref kind) = q.kind {
                    if &e.kind != kind {
                        return false;
                    }
                }
                if let Some(ref tag) = q.tag {
                    if !e.tags.contains(tag) {
                        return false;
                    }
                }
                if let Some(after) = q.after {
                    if e.created_at < after {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Sort newest first
        matches.sort_by_key(|m| std::cmp::Reverse(m.created_at));

        let total_matches = matches.len();

        if let Some(limit) = q.limit {
            matches.truncate(limit);
        }

        IndexResult {
            entries: matches,
            total_matches,
        }
    }
}

impl Default for MemoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_entry(id: &str, kind: MemoryEntryKind) -> MemoryEntry {
        MemoryEntry {
            id: id.into(),
            kind,
            summary: format!("summary {id}"),
            content_digest: format!("digest_{id}"),
            created_at: Utc::now(),
            tags: Vec::new(),
            token_estimate: 100,
            relevance: 0.5,
        }
    }

    #[test]
    fn test_insert_and_get() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("a", MemoryEntryKind::RunTrace))
            .unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.get("a").unwrap().kind, MemoryEntryKind::RunTrace);
    }

    #[test]
    fn test_remove() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("a", MemoryEntryKind::RunTrace))
            .unwrap();
        idx.remove("a").unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn test_get_not_found() {
        let idx = MemoryIndex::new();
        assert!(idx.get("nope").is_err());
    }

    #[test]
    fn test_query_all() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("a", MemoryEntryKind::RunTrace))
            .unwrap();
        idx.insert(make_entry("b", MemoryEntryKind::Diff)).unwrap();
        let r = idx.query(&IndexQuery::all());
        assert_eq!(r.total_matches, 2);
    }

    #[test]
    fn test_query_by_kind() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("a", MemoryEntryKind::RunTrace))
            .unwrap();
        idx.insert(make_entry("b", MemoryEntryKind::Diff)).unwrap();
        let r = idx.query(&IndexQuery::all().with_kind(MemoryEntryKind::Diff));
        assert_eq!(r.total_matches, 1);
        assert_eq!(r.entries[0].id, "b");
    }

    #[test]
    fn test_query_with_limit() {
        let mut idx = MemoryIndex::new();
        for i in 0..10 {
            let mut e = make_entry(&format!("e{i}"), MemoryEntryKind::RunTrace);
            e.created_at = Utc::now() - Duration::hours(i);
            idx.insert(e).unwrap();
        }
        let r = idx.query(&IndexQuery::all().with_limit(3));
        assert_eq!(r.total_matches, 10);
        assert_eq!(r.entries.len(), 3);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("x", MemoryEntryKind::Snapshot))
            .unwrap();
        let json = serde_json::to_string(&idx).unwrap();
        let back: MemoryIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn test_insert_duplicate_id_rejected() {
        let mut idx = MemoryIndex::new();
        idx.insert(make_entry("dup", MemoryEntryKind::RunTrace))
            .unwrap();
        let err = idx
            .insert(make_entry("dup", MemoryEntryKind::Diff))
            .expect_err("duplicate id should fail");
        assert!(matches!(err, MemoryError::DuplicateEntry { .. }));
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.get("dup").unwrap().kind, MemoryEntryKind::RunTrace);
    }
}
