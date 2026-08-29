//! Code quality guardrails for release/promotion decisions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{AivcsError, Result};
use oxidized_state::storage_traits::ContentDigest;

/// Required quality checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityCheck {
    Fmt,
    Lint,
    Test,
    Verification,
}

/// Finding severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualitySeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// Actionable finding produced by a quality check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFinding {
    pub severity: QualitySeverity,
    pub message: String,
    pub file_path: Option<String>,
    pub line: Option<u32>,
}

/// Result of one quality check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub check: QualityCheck,
    pub passed: bool,
    pub findings: Vec<CheckFinding>,
}

/// Release action guarded by quality policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseAction {
    Promote,
    Publish,
}

impl ReleaseAction {
    fn is_high_risk(self) -> bool {
        matches!(self, Self::Publish)
    }
}

/// Guardrail profile (`standard` or `strict`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailPolicyProfile {
    pub name: &'static str,
    pub required_checks: Vec<QualityCheck>,
    pub block_on_severity: QualitySeverity,
}

impl GuardrailPolicyProfile {
    pub fn standard() -> Self {
        Self {
            name: "standard",
            required_checks: vec![QualityCheck::Fmt, QualityCheck::Lint, QualityCheck::Test],
            block_on_severity: QualitySeverity::High,
        }
    }

    pub fn strict() -> Self {
        Self {
            name: "strict",
            required_checks: vec![
                QualityCheck::Fmt,
                QualityCheck::Lint,
                QualityCheck::Test,
                QualityCheck::Verification,
            ],
            block_on_severity: QualitySeverity::Medium,
        }
    }
}

/// Coverage metrics for required checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailCoverage {
    pub required_checks: usize,
    pub executed_required_checks: usize,
    pub passed_required_checks: usize,
}

/// Outcome of guardrail evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailVerdict {
    pub passed: bool,
    pub blocked_checks: Vec<QualityCheck>,
    pub missing_required_checks: Vec<QualityCheck>,
    pub blocking_findings: Vec<CheckFinding>,
    pub requires_approval: bool,
    pub coverage: GuardrailCoverage,
    pub evaluated_at: DateTime<Utc>,
}

/// Auditable run artifact for guardrail outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailArtifact {
    pub run_id: String,
    pub profile_name: String,
    pub check_results: Vec<CheckResult>,
    pub verdict: GuardrailVerdict,
}

/// Evaluate quality checks against a policy profile.
pub fn evaluate_quality_guardrails(
    profile: &GuardrailPolicyProfile,
    results: &[CheckResult],
    action: ReleaseAction,
    explicit_approval: bool,
) -> GuardrailVerdict {
    let mut by_check: HashMap<QualityCheck, &CheckResult> = HashMap::new();
    for r in results {
        by_check.insert(r.check, r);
    }

    let mut blocked_checks = Vec::new();
    let mut missing_required_checks = Vec::new();
    let mut blocking_findings = Vec::new();
    let mut executed_required_checks = 0usize;
    let mut passed_required_checks = 0usize;

    for required in &profile.required_checks {
        match by_check.get(required) {
            None => missing_required_checks.push(*required),
            Some(result) => {
                executed_required_checks += 1;
                if result.passed {
                    passed_required_checks += 1;
                } else {
                    blocked_checks.push(*required);
                }

                for f in &result.findings {
                    if f.severity >= profile.block_on_severity {
                        blocking_findings.push(f.clone());
                    }
                }
            }
        }
    }

    // Deduplicate blocked checks while preserving sort stability for determinism.
    let mut seen = HashSet::new();
    blocked_checks.retain(|c| seen.insert(*c));

    let requires_approval = action.is_high_risk() && !explicit_approval;
    let passed = blocked_checks.is_empty()
        && missing_required_checks.is_empty()
        && blocking_findings.is_empty()
        && !requires_approval;

    GuardrailVerdict {
        passed,
        blocked_checks,
        missing_required_checks,
        blocking_findings,
        requires_approval,
        coverage: GuardrailCoverage {
            required_checks: profile.required_checks.len(),
            executed_required_checks,
            passed_required_checks,
        },
        evaluated_at: Utc::now(),
    }
}

/// Human-readable release block reason, if blocked.
pub fn release_block_reason(verdict: &GuardrailVerdict) -> Option<String> {
    if verdict.passed {
        return None;
    }
    if verdict.requires_approval {
        return Some("high-risk action requires explicit approval".to_string());
    }
    if !verdict.missing_required_checks.is_empty() {
        return Some("required checks missing".to_string());
    }
    if !verdict.blocked_checks.is_empty() {
        return Some("required checks failed".to_string());
    }
    if !verdict.blocking_findings.is_empty() {
        return Some("blocking findings present".to_string());
    }
    Some("quality guardrail blocked".to_string())
}

/// Persist `<dir>/<run_id>/guardrails.json` and `<dir>/<run_id>/guardrails.digest`.
pub fn write_guardrail_artifact(artifact: &GuardrailArtifact, dir: &Path) -> Result<PathBuf> {
    let run_dir = dir.join(&artifact.run_id);
    std::fs::create_dir_all(&run_dir)?;

    let path = run_dir.join("guardrails.json");
    let digest_path = run_dir.join("guardrails.digest");
    let json = serde_json::to_vec_pretty(artifact)?;
    let digest = ContentDigest::from_bytes(&json).as_str().to_string();

    std::fs::write(&path, &json)?;
    std::fs::write(&digest_path, digest.as_bytes())?;

    Ok(path)
}

/// Read and verify `<dir>/<run_id>/guardrails.json` integrity.
pub fn read_guardrail_artifact(run_id: &str, dir: &Path) -> Result<GuardrailArtifact> {
    let run_dir = dir.join(run_id);
    let path = run_dir.join("guardrails.json");
    let digest_path = run_dir.join("guardrails.digest");

    let json = std::fs::read(&path)?;
    let digest = std::fs::read_to_string(&digest_path)?;
    let actual = ContentDigest::from_bytes(&json).as_str().to_string();
    if digest.trim() != actual {
        return Err(AivcsError::DigestMismatch {
            expected: digest.trim().to_string(),
            actual,
        });
    }
    let artifact: GuardrailArtifact = serde_json::from_slice(&json)?;
    Ok(artifact)
}
