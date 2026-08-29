//! Compatibility validator for release promotions.
//!
//! Evaluates a candidate [`Release`] against a [`CompatRuleSet`] to produce a
//! [`CompatVerdict`] — the pass/fail decision that blocks or allows a promote.

use serde::{Deserialize, Serialize};

use crate::domain::release::Release;

/// A single compatibility rule that can block a promotion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompatRule {
    /// Candidate `spec_digest` must be a valid 64-char lowercase hex string.
    SpecDigestValid,
    /// `tools_digest` must be non-empty.
    RequireToolsDigest,
    /// `graph_digest` must be non-empty.
    RequireGraphDigest,
    /// `tools_digest` must not change vs. the current release (if one exists).
    NoToolsChange,
    /// `graph_digest` must not change vs. the current release (if one exists).
    NoGraphChange,
}

/// A set of compatibility rules to evaluate before promoting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompatRuleSet {
    pub rules: Vec<CompatRule>,
}

impl CompatRuleSet {
    /// Standard rule set: `SpecDigestValid` + `RequireToolsDigest` + `RequireGraphDigest`.
    pub fn standard() -> Self {
        Self {
            rules: vec![
                CompatRule::SpecDigestValid,
                CompatRule::RequireToolsDigest,
                CompatRule::RequireGraphDigest,
            ],
        }
    }

    /// Add a rule to this set (builder pattern).
    pub fn with_rule(mut self, rule: CompatRule) -> Self {
        self.rules.push(rule);
        self
    }
}

/// Context for evaluating compatibility of a candidate release.
pub struct PromoteContext<'a> {
    /// The candidate release to validate.
    pub candidate: &'a Release,
    /// The existing release at the target environment (if any).
    pub current: Option<&'a Release>,
}

/// A single rule violation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompatViolation {
    /// Which rule was violated.
    pub rule: CompatRule,
    /// Human-readable explanation.
    pub reason: String,
}

/// The outcome of evaluating a compat rule set against a promote context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompatVerdict {
    /// Violations found (empty when passed).
    pub violations: Vec<CompatViolation>,
}

impl CompatVerdict {
    /// Whether the verdict passed (no violations).
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Evaluate a [`PromoteContext`] against a [`CompatRuleSet`], returning a [`CompatVerdict`].
pub fn evaluate_compat(rule_set: &CompatRuleSet, ctx: &PromoteContext) -> CompatVerdict {
    let mut violations = Vec::new();

    for rule in &rule_set.rules {
        if let Some(v) = check_rule(rule, ctx) {
            violations.push(v);
        }
    }

    CompatVerdict { violations }
}

fn is_valid_hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn check_rule(rule: &CompatRule, ctx: &PromoteContext) -> Option<CompatViolation> {
    match rule {
        CompatRule::SpecDigestValid => {
            if !is_valid_hex_digest(&ctx.candidate.spec_digest) {
                Some(CompatViolation {
                    rule: rule.clone(),
                    reason: format!(
                        "spec_digest '{}' is not a valid 64-char lowercase hex string",
                        ctx.candidate.spec_digest,
                    ),
                })
            } else {
                None
            }
        }
        CompatRule::RequireToolsDigest => {
            if ctx.candidate.tools_digest.is_empty() {
                Some(CompatViolation {
                    rule: rule.clone(),
                    reason: "tools_digest is empty".to_string(),
                })
            } else {
                None
            }
        }
        CompatRule::RequireGraphDigest => {
            if ctx.candidate.graph_digest.is_empty() {
                Some(CompatViolation {
                    rule: rule.clone(),
                    reason: "graph_digest is empty".to_string(),
                })
            } else {
                None
            }
        }
        CompatRule::NoToolsChange => {
            if let Some(current) = ctx.current {
                if ctx.candidate.tools_digest != current.tools_digest {
                    Some(CompatViolation {
                        rule: rule.clone(),
                        reason: format!(
                            "tools_digest changed: '{}' -> '{}'",
                            current.tools_digest, ctx.candidate.tools_digest,
                        ),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
        CompatRule::NoGraphChange => {
            if let Some(current) = ctx.current {
                if ctx.candidate.graph_digest != current.graph_digest {
                    Some(CompatViolation {
                        rule: rule.clone(),
                        reason: format!(
                            "graph_digest changed: '{}' -> '{}'",
                            current.graph_digest, ctx.candidate.graph_digest,
                        ),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}
