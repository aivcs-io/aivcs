use aivcs_core::{
    evaluate_gate, CaseResult, EvalReport, EvalThresholds, GateRule, GateRuleSet, GateVerdict,
};

fn passing_case(id: &str, tags: &[&str]) -> CaseResult {
    CaseResult {
        case_id: id.to_string(),
        score: 1.0,
        passed: true,
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

fn failing_case(id: &str, tags: &[&str]) -> CaseResult {
    CaseResult {
        case_id: id.to_string(),
        score: 0.0,
        passed: false,
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

fn report(pass_rate: f32, cases: Vec<CaseResult>, baseline: Option<f32>) -> EvalReport {
    EvalReport {
        case_results: cases,
        pass_rate,
        baseline_pass_rate: baseline,
    }
}

// ---- MinPassRate rule ----

#[test]
fn all_passing_meets_threshold() {
    let rule_set = GateRuleSet::standard();
    let r = report(1.0, vec![passing_case("c1", &[])], None);
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed());
    assert!(verdict.violations.is_empty());
}

#[test]
fn below_min_pass_rate_fails() {
    let rule_set = GateRuleSet::standard();
    let r = report(
        0.5,
        vec![passing_case("c1", &[]), failing_case("c2", &[])],
        None,
    );
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed());
    assert!(verdict
        .violations
        .iter()
        .any(|v| matches!(&v.rule, GateRule::MinPassRate)));
}

#[test]
fn exactly_at_threshold_passes() {
    let rule_set = GateRuleSet::standard(); // min_pass_rate = 0.95
    let r = report(0.95, vec![passing_case("c1", &[])], None);
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed());
}

// ---- MaxRegression rule ----

#[test]
fn no_baseline_skips_regression_check() {
    let rule_set = GateRuleSet::standard();
    let r = report(0.96, vec![passing_case("c1", &[])], None);
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed(), "no baseline should skip regression rule");
}

#[test]
fn regression_within_limit_passes() {
    let rule_set = GateRuleSet::standard(); // max_regression = 0.05
                                            // baseline 1.0 → current 0.96 = regression 0.04 < 0.05
    let r = report(0.96, vec![passing_case("c1", &[])], Some(1.0));
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed());
}

#[test]
fn regression_exceeding_limit_fails() {
    // Isolate regression rule: baseline 1.0 → current 0.90 = regression 0.10 > 0.05
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds {
            min_pass_rate: 0.0,
            max_regression: 0.05,
            fail_fast: false,
        },
        rules: vec![GateRule::MaxRegression],
    };
    let r = report(0.90, vec![], Some(1.0));
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed());
    assert!(verdict
        .violations
        .iter()
        .any(|v| matches!(&v.rule, GateRule::MaxRegression)));
}

// ---- RequireTag rule ----

#[test]
fn required_tag_all_pass() {
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds::default(),
        rules: vec![GateRule::RequireTag {
            tag: "critical".to_string(),
        }],
    };
    let r = report(
        1.0,
        vec![
            passing_case("c1", &["critical"]),
            passing_case("c2", &["critical"]),
            passing_case("c3", &[]),
        ],
        None,
    );
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed());
}

#[test]
fn required_tag_some_fail() {
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds {
            min_pass_rate: 0.0,
            max_regression: 1.0,
            fail_fast: false,
        },
        rules: vec![GateRule::RequireTag {
            tag: "critical".to_string(),
        }],
    };
    let r = report(
        0.5,
        vec![
            passing_case("c1", &["critical"]),
            failing_case("c2", &["critical"]),
            failing_case("c3", &[]),
        ],
        None,
    );
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed());
    let tag_violation = verdict
        .violations
        .iter()
        .find(|v| matches!(&v.rule, GateRule::RequireTag { .. }))
        .expect("should have RequireTag violation");
    assert!(tag_violation.reason.contains("c2"));
    assert!(!tag_violation.reason.contains("c3")); // c3 is not tagged critical
}

// ---- Fail-fast ----

#[test]
fn fail_fast_stops_at_first_violation() {
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds {
            min_pass_rate: 0.99,
            max_regression: 0.01,
            fail_fast: true,
        },
        rules: vec![
            GateRule::MinPassRate,
            GateRule::MaxRegression,
            GateRule::RequireTag {
                tag: "critical".to_string(),
            },
        ],
    };
    // This report violates all three rules
    let r = report(0.50, vec![failing_case("c1", &["critical"])], Some(1.0));
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed());
    assert_eq!(
        verdict.violations.len(),
        1,
        "fail_fast should stop at first violation"
    );
}

// ---- Edge cases ----

#[test]
fn empty_rules_always_passes() {
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds::default(),
        rules: vec![],
    };
    let r = report(0.0, vec![failing_case("c1", &[])], Some(1.0));
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(verdict.passed(), "no rules means gate always passes");
}

#[test]
fn empty_cases_with_zero_pass_rate_checked() {
    let rule_set = GateRuleSet::standard();
    let r = report(0.0, vec![], None);
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed(), "0% pass rate should fail MinPassRate");
}

#[test]
fn standard_constructor_has_two_rules() {
    let rule_set = GateRuleSet::standard();
    assert_eq!(rule_set.rules.len(), 2);
    assert!(matches!(rule_set.rules[0], GateRule::MinPassRate));
    assert!(matches!(rule_set.rules[1], GateRule::MaxRegression));
}

#[test]
fn multiple_violations_collected_without_fail_fast() {
    let rule_set = GateRuleSet {
        thresholds: EvalThresholds {
            min_pass_rate: 0.99,
            max_regression: 0.01,
            fail_fast: false,
        },
        rules: vec![GateRule::MinPassRate, GateRule::MaxRegression],
    };
    let r = report(0.50, vec![], Some(1.0));
    let verdict = evaluate_gate(&rule_set, &r);
    assert!(!verdict.passed());
    assert_eq!(
        verdict.violations.len(),
        2,
        "both violations should be collected"
    );
}

#[test]
fn with_rule_builder_appends() {
    let rule_set = GateRuleSet::standard().with_rule(GateRule::RequireTag {
        tag: "smoke".to_string(),
    });
    assert_eq!(rule_set.rules.len(), 3);
}

#[test]
fn verdict_pass_returns_true() {
    let v = GateVerdict { violations: vec![] };
    assert!(v.passed());
}
