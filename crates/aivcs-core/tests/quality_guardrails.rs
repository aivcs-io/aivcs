use aivcs_core::{
    evaluate_quality_guardrails, read_guardrail_artifact, release_block_reason,
    write_guardrail_artifact, CheckFinding, CheckResult, GuardrailArtifact, GuardrailPolicyProfile,
    QualityCheck, QualitySeverity, ReleaseAction,
};
use tempfile::tempdir;

fn pass(check: QualityCheck) -> CheckResult {
    CheckResult {
        check,
        passed: true,
        findings: vec![],
    }
}

fn fail_with_finding(check: QualityCheck, severity: QualitySeverity) -> CheckResult {
    CheckResult {
        check,
        passed: false,
        findings: vec![CheckFinding {
            severity,
            message: "missing import".to_string(),
            file_path: Some("crates/aivcs-core/src/lib.rs".to_string()),
            line: Some(42),
        }],
    }
}

#[test]
fn failed_required_checks_block_publish() {
    let profile = GuardrailPolicyProfile::standard();
    let checks = vec![
        pass(QualityCheck::Fmt),
        fail_with_finding(QualityCheck::Lint, QualitySeverity::High),
        pass(QualityCheck::Test),
    ];

    let verdict = evaluate_quality_guardrails(&profile, &checks, ReleaseAction::Publish, true);

    assert!(!verdict.passed);
    assert!(verdict.blocked_checks.contains(&QualityCheck::Lint));
    let reason = release_block_reason(&verdict).expect("must block");
    assert!(reason.contains("required checks failed"));
}

#[test]
fn findings_are_actionable_with_file_and_line() {
    let profile = GuardrailPolicyProfile::standard();
    let checks = vec![
        pass(QualityCheck::Fmt),
        fail_with_finding(QualityCheck::Lint, QualitySeverity::High),
        pass(QualityCheck::Test),
    ];

    let verdict = evaluate_quality_guardrails(&profile, &checks, ReleaseAction::Promote, true);

    let finding = verdict.blocking_findings.first().expect("has finding");
    assert_eq!(
        finding.file_path.as_deref(),
        Some("crates/aivcs-core/src/lib.rs")
    );
    assert_eq!(finding.line, Some(42));
}

#[test]
fn high_risk_requires_explicit_approval() {
    let profile = GuardrailPolicyProfile::standard();
    let checks = vec![
        pass(QualityCheck::Fmt),
        pass(QualityCheck::Lint),
        pass(QualityCheck::Test),
    ];

    let verdict = evaluate_quality_guardrails(&profile, &checks, ReleaseAction::Publish, false);

    assert!(!verdict.passed);
    assert!(verdict.requires_approval);
    assert!(release_block_reason(&verdict)
        .unwrap_or_default()
        .contains("explicit approval"));
}

#[test]
fn strict_profile_requires_verification() {
    let profile = GuardrailPolicyProfile::strict();
    let checks = vec![
        pass(QualityCheck::Fmt),
        pass(QualityCheck::Lint),
        pass(QualityCheck::Test),
    ];

    let verdict = evaluate_quality_guardrails(&profile, &checks, ReleaseAction::Promote, true);

    assert!(!verdict.passed);
    assert!(verdict
        .missing_required_checks
        .contains(&QualityCheck::Verification));
}

#[test]
fn guardrail_artifact_is_auditable_from_run_artifacts() {
    let profile = GuardrailPolicyProfile::standard();
    let checks = vec![
        pass(QualityCheck::Fmt),
        pass(QualityCheck::Lint),
        pass(QualityCheck::Test),
    ];
    let verdict = evaluate_quality_guardrails(&profile, &checks, ReleaseAction::Promote, true);

    let artifact = GuardrailArtifact {
        run_id: "run-123".to_string(),
        profile_name: profile.name.to_string(),
        check_results: checks,
        verdict,
    };

    let dir = tempdir().expect("tempdir");
    let path = write_guardrail_artifact(&artifact, dir.path()).expect("write");
    assert!(path.exists());

    let loaded = read_guardrail_artifact("run-123", dir.path()).expect("read");
    assert_eq!(loaded.run_id, "run-123");
    assert!(loaded.verdict.passed);
    assert_eq!(loaded.verdict.coverage.required_checks, 3);
    assert_eq!(loaded.verdict.coverage.passed_required_checks, 3);
}
