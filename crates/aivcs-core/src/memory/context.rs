//! Token-budgeted context assembly from memory entries.

use super::index::MemoryEntry;

/// Budget constraints for context window assembly.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    pub max_tokens: usize,
    pub reserved_tokens: usize,
}

impl ContextBudget {
    pub fn new(max_tokens: usize, reserved_tokens: usize) -> Result<Self, String> {
        if reserved_tokens >= max_tokens {
            return Err(format!(
                "reserved_tokens ({reserved_tokens}) must be less than max_tokens ({max_tokens})"
            ));
        }
        Ok(Self {
            max_tokens,
            reserved_tokens,
        })
    }

    /// Available tokens after reserving space.
    pub fn available(&self) -> usize {
        self.max_tokens - self.reserved_tokens
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_tokens: 128_000,
            reserved_tokens: 4_000,
        }
    }
}

/// A single item in the assembled context window.
#[derive(Debug, Clone)]
pub struct ContextItem {
    pub entry_id: String,
    pub text: String,
    pub tokens: usize,
}

/// The assembled context window.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    pub items: Vec<ContextItem>,
    pub total_tokens: usize,
    pub dropped_count: usize,
    pub budget: ContextBudget,
}

/// Assemble a context window from candidate entries, respecting the token budget.
///
/// Candidates are sorted by relevance (descending), then greedily packed
/// until the budget is exhausted. Entries that don't fit are dropped.
pub fn assemble_context(candidates: &[MemoryEntry], budget: &ContextBudget) -> ContextWindow {
    let mut sorted: Vec<&MemoryEntry> = candidates.iter().collect();
    sorted.sort_by(|a, b| {
        let a_score = if a.relevance.is_finite() {
            a.relevance
        } else {
            f64::NEG_INFINITY
        };
        let b_score = if b.relevance.is_finite() {
            b.relevance
        } else {
            f64::NEG_INFINITY
        };
        b_score
            .total_cmp(&a_score)
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });

    let available = budget.available();
    let mut items = Vec::new();
    let mut total_tokens = 0;
    let mut dropped_count = 0;

    for entry in sorted {
        if total_tokens + entry.token_estimate <= available {
            items.push(ContextItem {
                entry_id: entry.id.clone(),
                text: entry.summary.clone(),
                tokens: entry.token_estimate,
            });
            total_tokens += entry.token_estimate;
        } else {
            dropped_count += 1;
        }
    }

    ContextWindow {
        items,
        total_tokens,
        dropped_count,
        budget: budget.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::index::MemoryEntryKind;
    use chrono::Utc;

    fn make(id: &str, tokens: usize, relevance: f64) -> MemoryEntry {
        MemoryEntry {
            id: id.into(),
            kind: MemoryEntryKind::RunTrace,
            summary: format!("summary {id}"),
            content_digest: format!("d_{id}"),
            created_at: Utc::now(),
            tags: Vec::new(),
            token_estimate: tokens,
            relevance,
        }
    }

    #[test]
    fn test_empty_candidates() {
        let budget = ContextBudget::new(1000, 100).unwrap();
        let w = assemble_context(&[], &budget);
        assert!(w.items.is_empty());
        assert_eq!(w.total_tokens, 0);
    }

    #[test]
    fn test_all_fit() {
        let entries = vec![make("a", 100, 0.5), make("b", 200, 0.8)];
        let budget = ContextBudget::new(1000, 100).unwrap();
        let w = assemble_context(&entries, &budget);
        assert_eq!(w.items.len(), 2);
        assert_eq!(w.total_tokens, 300);
        // Higher relevance first
        assert_eq!(w.items[0].entry_id, "b");
    }

    #[test]
    fn test_budget_drops() {
        let entries = vec![make("a", 500, 0.9), make("b", 500, 0.5)];
        let budget = ContextBudget::new(700, 100).unwrap();
        let w = assemble_context(&entries, &budget);
        assert_eq!(w.items.len(), 1);
        assert_eq!(w.dropped_count, 1);
    }

    #[test]
    fn test_budget_validation() {
        assert!(ContextBudget::new(100, 200).is_err());
        assert!(ContextBudget::new(100, 100).is_err());
        assert!(ContextBudget::new(100, 99).is_ok());
    }

    #[test]
    fn test_nan_relevance_does_not_panic_or_win_sorting() {
        let entries = vec![make("good", 100, 0.8), make("nan", 100, f64::NAN)];
        let budget = ContextBudget::new(150, 0).unwrap();
        let w = assemble_context(&entries, &budget);
        assert_eq!(w.items.len(), 1);
        assert_eq!(w.items[0].entry_id, "good");
    }
}
