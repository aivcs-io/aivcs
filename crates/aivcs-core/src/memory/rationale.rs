//! Decision rationale capture for agent reasoning traces.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Outcome of a decision after execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RationaleOutcome {
    Success,
    Failure,
    Partial,
    Skipped,
}

/// A captured decision rationale with reasoning and alternatives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRationale {
    pub decision: String,
    pub reasoning: String,
    pub alternatives_considered: Vec<String>,
    pub constraints: Vec<String>,
    pub confidence: f64,
}

impl DecisionRationale {
    pub fn new(decision: &str, reasoning: &str) -> Self {
        Self {
            decision: decision.into(),
            reasoning: reasoning.into(),
            alternatives_considered: Vec::new(),
            constraints: Vec::new(),
            confidence: 0.0,
        }
    }

    pub fn with_alternative(mut self, alt: &str) -> Self {
        self.alternatives_considered.push(alt.into());
        self
    }

    pub fn with_constraint(mut self, constraint: &str) -> Self {
        self.constraints.push(constraint.into());
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }
}

/// A rationale entry linked to a specific run and event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RationaleEntry {
    pub rationale: DecisionRationale,
    pub run_id: String,
    pub event_seq: u64,
    pub decided_at: DateTime<Utc>,
    pub outcome: Option<RationaleOutcome>,
    pub tags: Vec<String>,
}

impl RationaleEntry {
    pub fn new(rationale: DecisionRationale, run_id: &str, event_seq: u64) -> Self {
        Self {
            rationale,
            run_id: run_id.into(),
            event_seq,
            decided_at: Utc::now(),
            outcome: None,
            tags: Vec::new(),
        }
    }

    pub fn with_outcome(mut self, outcome: RationaleOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Rough token estimate: chars / 4.
    pub fn token_estimate(&self) -> usize {
        let chars = self.rationale.decision.len()
            + self.rationale.reasoning.len()
            + self
                .rationale
                .alternatives_considered
                .iter()
                .map(|s| s.len())
                .sum::<usize>()
            + self
                .rationale
                .constraints
                .iter()
                .map(|s| s.len())
                .sum::<usize>();
        (chars / 4).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rationale_builder() {
        let r = DecisionRationale::new("do X", "because Y")
            .with_alternative("do Z")
            .with_constraint("must be fast")
            .with_confidence(0.85);

        assert_eq!(r.decision, "do X");
        assert_eq!(r.reasoning, "because Y");
        assert_eq!(r.alternatives_considered, vec!["do Z"]);
        assert_eq!(r.constraints, vec!["must be fast"]);
        assert!((r.confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_confidence_clamped() {
        let r = DecisionRationale::new("a", "b").with_confidence(2.0);
        assert!((r.confidence - 1.0).abs() < f64::EPSILON);

        let r = DecisionRationale::new("a", "b").with_confidence(-1.0);
        assert!(r.confidence.abs() < f64::EPSILON);
    }

    #[test]
    fn test_rationale_entry_token_estimate() {
        let r = DecisionRationale::new("decision", "reasoning");
        let e = RationaleEntry::new(r, "run1", 1);
        assert!(e.token_estimate() > 0);
    }

    #[test]
    fn test_serde_roundtrip() {
        let e = RationaleEntry::new(
            DecisionRationale::new("d", "r")
                .with_alternative("a")
                .with_confidence(0.5),
            "run",
            1,
        )
        .with_outcome(RationaleOutcome::Success)
        .with_tag("test");

        let json = serde_json::to_string(&e).unwrap();
        let back: RationaleEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
