//! Merge gate rules engine.
//!
//! Evaluates [`CaseResult`] vectors against [`GateRuleSet`] configurations to
//! produce a [`GateVerdict`] — the pass/fail decision that blocks or allows a
//! merge. Supports threshold checks, regression limits, fail-fast, and
//! tag-based required-pass rules.

use serde::{Deserialize, Serialize};

use crate::domain::eval::EvalThresholds;

// ---------------------------------------------------------------------------
// Eval result types (input to the gate)
// ---------------------------------------------------------------------------

/// Result of evaluating a single test case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseResult {
    /// Identifier for the test case.
    pub case_id: String,
    /// Score in 0.0–1.0.
    pub score: f32,
    /// Whether this case passed.
    pub passed: bool,
    /// Tags inherited from the `EvalTestCase`.
    pub tags: Vec<String>,
}

/// Aggregated report from an eval run — the input to the gate engine.
///
/// # Invariants
///
/// `pass_rate` must be derived from `case_results` by the eval runner (number
/// of `passed == true` cases divided by total cases) and kept consistent.
/// Gate rules that operate on overall pass rate (`MinPassRate`, `MaxRegression`)
/// use `pass_rate`; per-case rules (`RequireTag`) use `case_results`.
/// Inconsistent values are a bug in the producer and can cause surprising verdicts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalReport {
    /// Per-case results used by per-case and tag-based rules.
    pub case_results: Vec<CaseResult>,
    /// Overall pass rate (0.0–1.0) used by aggregate rules. Must be derived
    /// from `case_results` and remain consistent with it.
    pub pass_rate: f32,
    /// Optional baseline pass rate for regression detection.
    pub baseline_pass_rate: Option<f32>,
}

// ---------------------------------------------------------------------------
// Gate rules
// ---------------------------------------------------------------------------

/// A single gate rule that can block a merge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GateRule {
    /// Pass rate must meet or exceed `EvalThresholds::min_pass_rate`.
    MinPassRate,
    /// Regression (baseline − current) must not exceed `EvalThresholds::max_regression`.
    MaxRegression,
    /// All cases with the given tag must pass.
    RequireTag { tag: String },
}

/// A set of gate rules plus the thresholds they reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRuleSet {
    pub thresholds: EvalThresholds,
    pub rules: Vec<GateRule>,
}

impl GateRuleSet {
    /// Create a rule set with default thresholds and the standard rules
    /// (`MinPassRate` + `MaxRegression`).
    pub fn standard() -> Self {
        Self {
            thresholds: EvalThresholds::default(),
            rules: vec![GateRule::MinPassRate, GateRule::MaxRegression],
        }
    }

    /// Add a rule.
    pub fn with_rule(mut self, rule: GateRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Override thresholds.
    pub fn with_thresholds(mut self, thresholds: EvalThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// A single rule violation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Violation {
    /// Which rule was violated.
    pub rule: GateRule,
    /// Human-readable explanation.
    pub reason: String,
}

/// The outcome of evaluating a gate rule set against an eval report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateVerdict {
    /// Violations found (empty when passed).
    pub violations: Vec<Violation>,
}

impl GateVerdict {
    fn pass() -> Self {
        Self {
            violations: Vec::new(),
        }
    }

    fn fail(violations: Vec<Violation>) -> Self {
        Self { violations }
    }

    /// Whether the gate passed (i.e., there are no violations).
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Evaluate an [`EvalReport`] against a [`GateRuleSet`], returning a [`GateVerdict`].
///
/// When `thresholds.fail_fast` is true, evaluation stops at the first violation.
pub fn evaluate_gate(rule_set: &GateRuleSet, report: &EvalReport) -> GateVerdict {
    let mut violations = Vec::new();
    let fail_fast = rule_set.thresholds.fail_fast;

    for rule in &rule_set.rules {
        if let Some(v) = check_rule(rule, &rule_set.thresholds, report) {
            violations.push(v);
            if fail_fast {
                return GateVerdict::fail(violations);
            }
        }
    }

    if violations.is_empty() {
        GateVerdict::pass()
    } else {
        GateVerdict::fail(violations)
    }
}

fn check_rule(
    rule: &GateRule,
    thresholds: &EvalThresholds,
    report: &EvalReport,
) -> Option<Violation> {
    match rule {
        GateRule::MinPassRate => {
            if report.pass_rate < thresholds.min_pass_rate {
                Some(Violation {
                    rule: rule.clone(),
                    reason: format!(
                        "pass rate {:.2}% < required {:.2}%",
                        report.pass_rate * 100.0,
                        thresholds.min_pass_rate * 100.0,
                    ),
                })
            } else {
                None
            }
        }
        GateRule::MaxRegression => {
            if let Some(baseline) = report.baseline_pass_rate {
                let regression = baseline - report.pass_rate;
                if regression > thresholds.max_regression {
                    Some(Violation {
                        rule: rule.clone(),
                        reason: format!(
                            "regression {:.2}% > allowed {:.2}% (baseline {:.2}% → current {:.2}%)",
                            regression * 100.0,
                            thresholds.max_regression * 100.0,
                            baseline * 100.0,
                            report.pass_rate * 100.0,
                        ),
                    })
                } else {
                    None
                }
            } else {
                // No baseline → no regression to check
                None
            }
        }
        GateRule::RequireTag { tag } => {
            let tagged: Vec<&CaseResult> = report
                .case_results
                .iter()
                .filter(|c| c.tags.contains(tag))
                .collect();

            let failed: Vec<&str> = tagged
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.case_id.as_str())
                .collect();

            if failed.is_empty() {
                None
            } else {
                Some(Violation {
                    rule: rule.clone(),
                    reason: format!(
                        "{} of {} cases tagged '{}' failed: [{}]",
                        failed.len(),
                        tagged.len(),
                        tag,
                        failed.join(", "),
                    ),
                })
            }
        }
    }
}
