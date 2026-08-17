//! Memory, context, and retrieval for EPIC5.
//!
//! Provides indexed retrieval over snapshots and memories, decision rationale
//! capture, token-budgeted context assembly, and retention/compaction policies.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{AivcsError, Result};
use oxidized_state::storage_traits::ContentDigest;

// ---------------------------------------------------------------------------
// Memory Index — Indexed retrieval over memories and run history
// ---------------------------------------------------------------------------

/// A relevance-scored memory hit from an index query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryHit {
    pub key: String,
    pub content: String,
    pub commit_id: String,
    pub score: f64,
    pub created_at: DateTime<Utc>,
}

/// Match strategy for memory queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStrategy {
    /// Exact key match.
    Exact,
    /// Substring match on key or content.
    Substring,
    /// Keyword overlap scoring.
    Keyword,
}

/// Query parameters for memory index lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub query_text: String,
    pub strategy: MatchStrategy,
    pub max_results: usize,
    /// Optional: restrict to memories from these commit IDs.
    pub scope_commits: Option<Vec<String>>,
}

impl MemoryQuery {
    pub fn keyword(text: impl Into<String>, max_results: usize) -> Self {
        Self {
            query_text: text.into(),
            strategy: MatchStrategy::Keyword,
            max_results,
            scope_commits: None,
        }
    }

    pub fn exact(key: impl Into<String>) -> Self {
        Self {
            query_text: key.into(),
            strategy: MatchStrategy::Exact,
            max_results: 1,
            scope_commits: None,
        }
    }

    pub fn scoped(mut self, commits: Vec<String>) -> Self {
        self.scope_commits = Some(commits);
        self
    }
}

/// In-process memory index backed by a flat list of `MemoryRecord`-like entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryIndex {
    entries: Vec<MemoryEntry>,
}

/// Indexed entry (lightweight projection of MemoryRecord).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub commit_id: String,
    pub created_at: DateTime<Utc>,
}

impl MemoryIndex {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Ingest a batch of memory entries.
    pub fn ingest(&mut self, entries: Vec<MemoryEntry>) {
        self.entries.extend(entries);
    }

    /// Number of indexed entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Query the index and return scored hits.
    pub fn query(&self, q: &MemoryQuery) -> Vec<MemoryHit> {
        let candidates: Vec<&MemoryEntry> = self
            .entries
            .iter()
            .filter(|e| match &q.scope_commits {
                Some(commits) => commits.contains(&e.commit_id),
                None => true,
            })
            .collect();

        let mut scored: Vec<MemoryHit> = candidates
            .into_iter()
            .filter_map(|e| {
                let score = match q.strategy {
                    MatchStrategy::Exact => {
                        if e.key == q.query_text {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    MatchStrategy::Substring => {
                        let q_lower = q.query_text.to_lowercase();
                        if e.key.to_lowercase().contains(&q_lower) {
                            0.8
                        } else if e.content.to_lowercase().contains(&q_lower) {
                            0.5
                        } else {
                            0.0
                        }
                    }
                    MatchStrategy::Keyword => keyword_score(&q.query_text, &e.key, &e.content),
                };
                if score > 0.0 {
                    Some(MemoryHit {
                        key: e.key.clone(),
                        content: e.content.clone(),
                        commit_id: e.commit_id.clone(),
                        score,
                        created_at: e.created_at,
                    })
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(q.max_results);
        scored
    }
}

impl Default for MemoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple keyword overlap scorer: fraction of query words found in key+content.
fn keyword_score(query: &str, key: &str, content: &str) -> f64 {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let haystack = format!("{} {}", key, content).to_lowercase();
    let matched = words
        .iter()
        .filter(|w| haystack.contains(&w.to_lowercase()))
        .count();
    matched as f64 / words.len() as f64
}

// ---------------------------------------------------------------------------
// Decision Rationale — Capture reasoning for major actions
// ---------------------------------------------------------------------------

/// Severity/importance of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionImportance {
    Low,
    Medium,
    High,
    Critical,
}

/// A captured decision rationale tied to a run event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRationale {
    pub decision_id: String,
    pub run_id: String,
    pub event_seq: u64,
    pub action: String,
    pub reasoning: String,
    pub alternatives_considered: Vec<String>,
    pub importance: DecisionImportance,
    pub outcome: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

/// Ledger for decision rationales within a run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RationaleLedger {
    entries: Vec<DecisionRationale>,
}

impl RationaleLedger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Record a decision.
    pub fn record(&mut self, rationale: DecisionRationale) {
        self.entries.push(rationale);
    }

    /// Query rationales for a specific run.
    pub fn for_run(&self, run_id: &str) -> Vec<&DecisionRationale> {
        self.entries.iter().filter(|r| r.run_id == run_id).collect()
    }

    /// Query rationales by action substring.
    pub fn for_action(&self, action_pattern: &str) -> Vec<&DecisionRationale> {
        let pattern = action_pattern.to_lowercase();
        self.entries
            .iter()
            .filter(|r| r.action.to_lowercase().contains(&pattern))
            .collect()
    }

    /// Get high-importance decisions (for planning context).
    pub fn important_decisions(
        &self,
        min_importance: DecisionImportance,
    ) -> Vec<&DecisionRationale> {
        self.entries
            .iter()
            .filter(|r| r.importance >= min_importance)
            .collect()
    }

    /// Total recorded decisions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries (for serialization / persistence).
    pub fn entries(&self) -> &[DecisionRationale] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// Context Assembly — Token-budgeted context packing
// ---------------------------------------------------------------------------

/// A context segment with estimated token cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSegment {
    pub label: String,
    pub content: String,
    pub priority: u32,
    pub estimated_tokens: usize,
}

impl ContextSegment {
    /// Create a segment with auto-estimated token count (~4 chars per token).
    pub fn new(label: impl Into<String>, content: impl Into<String>, priority: u32) -> Self {
        let content = content.into();
        let estimated_tokens = estimate_tokens(&content);
        Self {
            label: label.into(),
            content,
            priority,
            estimated_tokens,
        }
    }

    /// Create a segment with explicit token count.
    pub fn with_tokens(
        label: impl Into<String>,
        content: impl Into<String>,
        priority: u32,
        tokens: usize,
    ) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            priority,
            estimated_tokens: tokens,
        }
    }
}

/// Assembled context window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssembledContext {
    pub segments: Vec<ContextSegment>,
    pub total_tokens: usize,
    pub budget: usize,
    pub dropped_count: usize,
}

impl AssembledContext {
    /// Render all segments into a single string.
    pub fn render(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("## {}\n{}", s.label, s.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Assembles context segments within a token budget.
#[derive(Debug, Clone)]
pub struct ContextAssembler {
    pub token_budget: usize,
}

impl ContextAssembler {
    pub fn new(token_budget: usize) -> Self {
        Self { token_budget }
    }

    /// Pack segments by priority (highest first) within the token budget.
    pub fn assemble(&self, mut segments: Vec<ContextSegment>) -> AssembledContext {
        // Sort by priority descending (highest priority first).
        segments.sort_by_key(|s| std::cmp::Reverse(s.priority));

        let mut included = Vec::new();
        let mut total_tokens = 0usize;
        let mut dropped_count = 0usize;

        for seg in segments {
            if total_tokens + seg.estimated_tokens <= self.token_budget {
                total_tokens += seg.estimated_tokens;
                included.push(seg);
            } else {
                dropped_count += 1;
            }
        }

        AssembledContext {
            segments: included,
            total_tokens,
            budget: self.token_budget,
            dropped_count,
        }
    }
}

/// Estimate token count from text (~4 chars per token heuristic).
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

// ---------------------------------------------------------------------------
// Compaction Policy — Retention and compaction for memory stores
// ---------------------------------------------------------------------------

/// Strategy for compacting old memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStrategy {
    /// Delete memories older than the threshold.
    DeleteOld,
    /// Keep only the most recent N per key.
    KeepRecentPerKey,
    /// Merge old entries into summary records.
    Summarize,
}

/// Policy for memory retention and compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionPolicy {
    pub max_age_days: Option<u64>,
    pub max_entries_per_key: Option<usize>,
    pub strategy: CompactionStrategy,
}

impl CompactionPolicy {
    pub fn delete_older_than(days: u64) -> Self {
        Self {
            max_age_days: Some(days),
            max_entries_per_key: None,
            strategy: CompactionStrategy::DeleteOld,
        }
    }

    pub fn keep_recent(per_key: usize) -> Self {
        Self {
            max_age_days: None,
            max_entries_per_key: Some(per_key),
            strategy: CompactionStrategy::KeepRecentPerKey,
        }
    }

    /// Apply compaction to a list of memory entries, returning retained entries
    /// and the count of compacted entries.
    pub fn compact(&self, entries: &[MemoryEntry]) -> CompactionResult {
        let now = Utc::now();
        let mut retained: Vec<MemoryEntry> = Vec::new();
        let mut compacted = 0usize;

        // Phase 1: age-based filtering
        let after_age: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|e| {
                if let Some(max_days) = self.max_age_days {
                    let cutoff = now - Duration::days(max_days as i64);
                    if e.created_at < cutoff {
                        return false;
                    }
                }
                true
            })
            .collect();

        let age_compacted = entries.len() - after_age.len();
        compacted += age_compacted;

        // Phase 2: per-key limit
        match self.max_entries_per_key {
            Some(max_per_key) => {
                use std::collections::HashMap;
                let mut by_key: HashMap<&str, Vec<&MemoryEntry>> = HashMap::new();
                for e in &after_age {
                    by_key.entry(e.key.as_str()).or_default().push(e);
                }
                for (_key, mut group) in by_key {
                    // Sort newest first.
                    group.sort_by_key(|e| std::cmp::Reverse(e.created_at));
                    for (i, e) in group.into_iter().enumerate() {
                        if i < max_per_key {
                            retained.push(e.clone());
                        } else {
                            compacted += 1;
                        }
                    }
                }
            }
            None => {
                retained = after_age.into_iter().cloned().collect();
            }
        }

        // Restore chronological order.
        retained.sort_by_key(|e| e.created_at);

        CompactionResult {
            retained,
            compacted_count: compacted,
        }
    }
}

/// Result of a compaction operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    pub retained: Vec<MemoryEntry>,
    pub compacted_count: usize,
}

// ---------------------------------------------------------------------------
// Memory Context Artifact — Auditable persistence
// ---------------------------------------------------------------------------

/// Auditable artifact for memory context state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryContextArtifact {
    pub run_id: String,
    pub index_size: usize,
    pub rationale_count: usize,
    pub context_tokens_used: usize,
    pub context_budget: usize,
    pub compaction_applied: bool,
    pub created_at: DateTime<Utc>,
}

/// Persist a memory context artifact with digest verification.
pub fn write_memory_context_artifact(
    artifact: &MemoryContextArtifact,
    dir: &Path,
) -> Result<PathBuf> {
    let run_dir = dir.join(&artifact.run_id);
    std::fs::create_dir_all(&run_dir)?;

    let path = run_dir.join("memory_context.json");
    let digest_path = run_dir.join("memory_context.digest");
    let json = serde_json::to_vec_pretty(artifact)?;
    let digest = ContentDigest::from_bytes(&json).as_str().to_string();

    std::fs::write(&path, &json)?;
    std::fs::write(&digest_path, digest.as_bytes())?;

    Ok(path)
}

/// Read and verify a memory context artifact.
pub fn read_memory_context_artifact(run_id: &str, dir: &Path) -> Result<MemoryContextArtifact> {
    let run_dir = dir.join(run_id);
    let path = run_dir.join("memory_context.json");
    let digest_path = run_dir.join("memory_context.digest");

    let json = std::fs::read(&path)?;
    let digest = std::fs::read_to_string(&digest_path)?;
    let actual = ContentDigest::from_bytes(&json).as_str().to_string();
    if digest.trim() != actual {
        return Err(AivcsError::DigestMismatch {
            expected: digest.trim().to_string(),
            actual,
        });
    }
    let artifact: MemoryContextArtifact = serde_json::from_slice(&json)?;
    Ok(artifact)
}
