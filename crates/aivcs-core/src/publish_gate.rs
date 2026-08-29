//! Publish gate rules engine.
//!
//! Evaluates a [`PublishCandidate`] against a [`PublishRuleSet`] to produce a
//! [`PublishVerdict`] — the pass/fail decision that blocks or allows a release
//! promotion. Supports semver validation, version ordering, uniqueness checks,
//! and release-notes/spec-digest requirements.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Semver helpers (manual — no external dep)
// ---------------------------------------------------------------------------

/// Parsed semver: MAJOR.MINOR.PATCH with optional pre-release suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Semver {
    fn parse(input: &str) -> Option<Self> {
        let (version_part, pre) = match input.split_once('-') {
            Some((v, p)) if !p.is_empty() => (v, Some(p.to_string())),
            _ => (input, None),
        };

        let parts: Vec<&str> = version_part.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        let major = parts[0].parse::<u64>().ok()?;
        let minor = parts[1].parse::<u64>().ok()?;
        let patch = parts[2].parse::<u64>().ok()?;

        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// Compare two semver values. Pre-release < release for equal versions.
    fn cmp_version(&self, other: &Self) -> std::cmp::Ordering {
        let tuple_cmp =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if tuple_cmp != std::cmp::Ordering::Equal {
            return tuple_cmp;
        }
        // Same numeric version: pre-release < release
        match (&self.pre, &other.pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => cmp_prerelease(a, b),
        }
    }
}

/// Compare two pre-release strings per semver §11.
///
/// Identifiers are split on `.`; purely numeric identifiers compare
/// numerically (so `alpha.9 < alpha.10`), numeric identifiers rank lower than
/// alphanumeric ones, and a larger set of identifiers wins when all preceding
/// ones are equal. A plain `str::cmp` (the previous behaviour) is lexicographic
/// and wrongly ordered `alpha.10` before `alpha.9`, causing `VersionBump` to
/// reject legitimately higher pre-release versions.
fn cmp_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = cmp_identifier(x, y);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Compare a single pre-release identifier per semver §11.4. Numeric
/// identifiers (all ASCII digits) compare numerically and rank below
/// alphanumeric ones. Numeric comparison is done on the digit string
/// (length-then-lexicographic, ignoring leading zeros) rather than via
/// `u64`, so it stays correct for identifiers larger than `u64::MAX`.
fn cmp_identifier(x: &str, y: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let x_num = !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    let y_num = !y.is_empty() && y.bytes().all(|c| c.is_ascii_digit());
    match (x_num, y_num) {
        (true, true) => {
            let xt = x.trim_start_matches('0');
            let yt = y.trim_start_matches('0');
            xt.len().cmp(&yt.len()).then_with(|| xt.cmp(yt))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => x.cmp(y),
    }
}

// ---------------------------------------------------------------------------
// Publish candidate (input to the gate)
// ---------------------------------------------------------------------------

/// The release candidate being evaluated for publish readiness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublishCandidate {
    /// Version label to publish (e.g. "1.2.3").
    pub version_label: Option<String>,
    /// Previous published version (for ordering checks).
    pub previous_version: Option<String>,
    /// All previously published version labels (for uniqueness checks).
    pub existing_versions: Vec<String>,
    /// Release notes content.
    pub notes: Option<String>,
    /// Spec digest string.
    pub spec_digest: String,
}

// ---------------------------------------------------------------------------
// Publish rules
// ---------------------------------------------------------------------------

/// A single publish gate rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublishRule {
    /// `version_label` must parse as valid semver (MAJOR.MINOR.PATCH, optional
    /// pre-release suffix).
    SemverFormat,
    /// Version must be strictly greater than `previous_version` (skipped when
    /// no previous version exists).
    VersionBump,
    /// Version must not already appear in `existing_versions`.
    UniqueVersion,
    /// Release notes must be present and non-empty.
    RequireNotes,
    /// `spec_digest` must be non-empty.
    RequireSpecDigest,
}

/// A set of publish rules with a fail-fast flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublishRuleSet {
    pub rules: Vec<PublishRule>,
    pub fail_fast: bool,
}

impl PublishRuleSet {
    /// Standard rule set: `SemverFormat` + `VersionBump` + `RequireSpecDigest`.
    pub fn standard() -> Self {
        Self {
            rules: vec![
                PublishRule::SemverFormat,
                PublishRule::VersionBump,
                PublishRule::RequireSpecDigest,
            ],
            fail_fast: false,
        }
    }

    /// Append a rule.
    pub fn with_rule(mut self, rule: PublishRule) -> Self {
        self.rules.push(rule);
        self
    }
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// A single rule violation.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishViolation {
    /// Which rule was violated.
    pub rule: PublishRule,
    /// Human-readable explanation.
    pub reason: String,
}

/// The outcome of evaluating a publish rule set against a candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishVerdict {
    /// Whether the gate passed (no violations).
    pub passed: bool,
    /// Violations found (empty when passed).
    pub violations: Vec<PublishViolation>,
}

impl PublishVerdict {
    fn pass() -> Self {
        Self {
            passed: true,
            violations: Vec::new(),
        }
    }

    fn fail(violations: Vec<PublishViolation>) -> Self {
        Self {
            passed: false,
            violations,
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Evaluate a [`PublishCandidate`] against a [`PublishRuleSet`], returning a
/// [`PublishVerdict`].
///
/// When `fail_fast` is true, evaluation stops at the first violation.
pub fn evaluate_publish_gate(
    rule_set: &PublishRuleSet,
    candidate: &PublishCandidate,
) -> PublishVerdict {
    let mut violations = Vec::new();

    for rule in &rule_set.rules {
        if let Some(v) = check_rule(rule, candidate) {
            violations.push(v);
            if rule_set.fail_fast {
                return PublishVerdict::fail(violations);
            }
        }
    }

    if violations.is_empty() {
        PublishVerdict::pass()
    } else {
        PublishVerdict::fail(violations)
    }
}

fn check_rule(rule: &PublishRule, candidate: &PublishCandidate) -> Option<PublishViolation> {
    match rule {
        PublishRule::SemverFormat => {
            let label = match &candidate.version_label {
                Some(l) if !l.is_empty() => l,
                _ => {
                    return Some(PublishViolation {
                        rule: rule.clone(),
                        reason: "version_label is missing or empty".to_string(),
                    });
                }
            };
            if Semver::parse(label).is_none() {
                Some(PublishViolation {
                    rule: rule.clone(),
                    reason: format!(
                        "'{}' is not valid semver (expected MAJOR.MINOR.PATCH)",
                        label
                    ),
                })
            } else {
                None
            }
        }

        PublishRule::VersionBump => {
            let current_label = match &candidate.version_label {
                Some(l) if !l.is_empty() => l,
                _ => return None, // no label → nothing to compare
            };
            let prev_label = match &candidate.previous_version {
                Some(l) if !l.is_empty() => l,
                _ => return None, // no previous → skip
            };
            let current = match Semver::parse(current_label) {
                Some(s) => s,
                None => {
                    return Some(PublishViolation {
                        rule: rule.clone(),
                        reason: format!(
                            "version '{}' is not valid semver (cannot compare to previous '{}')",
                            current_label, prev_label,
                        ),
                    });
                }
            };
            let previous = match Semver::parse(prev_label) {
                Some(s) => s,
                None => {
                    return Some(PublishViolation {
                        rule: rule.clone(),
                        reason: format!(
                            "previous_version '{}' is not valid semver (cannot compare to '{}')",
                            prev_label, current_label,
                        ),
                    });
                }
            };
            if current.cmp_version(&previous) != std::cmp::Ordering::Greater {
                Some(PublishViolation {
                    rule: rule.clone(),
                    reason: format!(
                        "version '{}' is not greater than previous '{}'",
                        current_label, prev_label,
                    ),
                })
            } else {
                None
            }
        }

        PublishRule::UniqueVersion => {
            let label = match &candidate.version_label {
                Some(l) if !l.is_empty() => l,
                _ => return None,
            };
            if candidate.existing_versions.contains(label) {
                Some(PublishViolation {
                    rule: rule.clone(),
                    reason: format!("version '{}' already exists in history", label),
                })
            } else {
                None
            }
        }

        PublishRule::RequireNotes => match &candidate.notes {
            Some(n) if !n.trim().is_empty() => None,
            _ => Some(PublishViolation {
                rule: rule.clone(),
                reason: "release notes are missing or empty".to_string(),
            }),
        },

        PublishRule::RequireSpecDigest => {
            if candidate.spec_digest.trim().is_empty() {
                Some(PublishViolation {
                    rule: rule.clone(),
                    reason: "spec_digest is missing or empty".to_string(),
                })
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_prerelease_identifiers_compare_numerically() {
        let lo = Semver::parse("1.0.0-alpha.9").unwrap();
        let hi = Semver::parse("1.0.0-alpha.10").unwrap();
        // Lexicographically "alpha.10" < "alpha.9"; numerically alpha.10 wins.
        assert_eq!(lo.cmp_version(&hi), std::cmp::Ordering::Less);
        assert_eq!(hi.cmp_version(&lo), std::cmp::Ordering::Greater);
    }

    #[test]
    fn huge_numeric_prerelease_identifiers_compare_numerically() {
        // Both exceed u64::MAX, so a u64-parse approach would fall back to a
        // (wrong) lexicographic compare. 1e20 (21 digits) > 9.9e19 (20 digits).
        let lo = Semver::parse("1.0.0-alpha.99999999999999999999").unwrap();
        let hi = Semver::parse("1.0.0-alpha.100000000000000000000").unwrap();
        assert_eq!(lo.cmp_version(&hi), std::cmp::Ordering::Less);
        assert_eq!(hi.cmp_version(&lo), std::cmp::Ordering::Greater);
    }

    #[test]
    fn prerelease_is_less_than_release() {
        let pre = Semver::parse("1.0.0-rc.1").unwrap();
        let rel = Semver::parse("1.0.0").unwrap();
        assert_eq!(pre.cmp_version(&rel), std::cmp::Ordering::Less);
    }

    #[test]
    fn version_bump_accepts_higher_numeric_prerelease() {
        let candidate = PublishCandidate {
            version_label: Some("1.0.0-alpha.10".to_string()),
            previous_version: Some("1.0.0-alpha.9".to_string()),
            existing_versions: vec![],
            notes: Some("notes".to_string()),
            spec_digest: "digest".to_string(),
        };
        let violation = check_rule(&PublishRule::VersionBump, &candidate);
        assert!(
            violation.is_none(),
            "1.0.0-alpha.10 should be accepted over 1.0.0-alpha.9, got {violation:?}"
        );
    }
}
