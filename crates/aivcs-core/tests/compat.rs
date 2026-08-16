use aivcs_core::domain::release::{Release, ReleaseEnvironment};
use aivcs_core::{
    evaluate_compat, CompatRule, CompatRuleSet, CompatVerdict, CompatViolation, PromoteContext,
};

/// Valid 64-char lowercase hex digest for tests.
const VALID_DIGEST: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const VALID_TOOLS: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const VALID_GRAPH: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn make_release(spec: &str, tools: &str, graph: &str) -> Release {
    Release::new(
        "test-agent".to_string(),
        spec.to_string(),
        tools.to_string(),
        graph.to_string(),
        "1.0.0".to_string(),
        ReleaseEnvironment::Staging,
        "ci".to_string(),
    )
}

// ── SpecDigestValid ─────────────────────────────────────────────────────

#[test]
fn spec_digest_valid_passes_well_formed() {
    let candidate = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::SpecDigestValid],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(verdict.passed());
}

#[test]
fn spec_digest_valid_fails_empty() {
    let candidate = make_release("", VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::SpecDigestValid],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
    assert_eq!(verdict.violations.len(), 1);
    assert_eq!(verdict.violations[0].rule, CompatRule::SpecDigestValid);
}

#[test]
fn spec_digest_valid_fails_wrong_length() {
    let candidate = make_release("abcdef", VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::SpecDigestValid],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
}

#[test]
fn spec_digest_valid_fails_non_hex() {
    // 64 chars but contains 'g' which is not hex
    let bad = "gggggggg0123456789abcdef0123456789abcdef0123456789abcdef01234567";
    assert_eq!(bad.len(), 64);
    let candidate = make_release(bad, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::SpecDigestValid],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
}

// ── RequireToolsDigest / RequireGraphDigest ──────────────────────────────

#[test]
fn require_tools_digest_fails_empty() {
    let candidate = make_release(VALID_DIGEST, "", VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::RequireToolsDigest],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
    assert_eq!(verdict.violations[0].rule, CompatRule::RequireToolsDigest);
}

#[test]
fn require_graph_digest_fails_empty() {
    let candidate = make_release(VALID_DIGEST, VALID_TOOLS, "");
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::RequireGraphDigest],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
    assert_eq!(verdict.violations[0].rule, CompatRule::RequireGraphDigest);
}

// ── NoToolsChange / NoGraphChange ───────────────────────────────────────

#[test]
fn no_tools_change_passes_same_digest() {
    let candidate = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let current = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: Some(&current),
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::NoToolsChange],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(verdict.passed());
}

#[test]
fn no_tools_change_fails_different_digest() {
    let other_tools = "3333333333333333333333333333333333333333333333333333333333333333";
    let candidate = make_release(VALID_DIGEST, other_tools, VALID_GRAPH);
    let current = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: Some(&current),
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::NoToolsChange],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
    assert_eq!(verdict.violations[0].rule, CompatRule::NoToolsChange);
}

#[test]
fn no_graph_change_fails_different_digest() {
    let other_graph = "4444444444444444444444444444444444444444444444444444444444444444";
    let candidate = make_release(VALID_DIGEST, VALID_TOOLS, other_graph);
    let current = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: Some(&current),
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::NoGraphChange],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(!verdict.passed());
    assert_eq!(verdict.violations[0].rule, CompatRule::NoGraphChange);
}

#[test]
fn no_tools_change_passes_when_no_current_release() {
    let candidate = make_release(VALID_DIGEST, VALID_TOOLS, VALID_GRAPH);
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet {
        rules: vec![CompatRule::NoToolsChange, CompatRule::NoGraphChange],
    };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(verdict.passed());
}

// ── Edge cases ──────────────────────────────────────────────────────────

#[test]
fn empty_rules_always_passes() {
    let candidate = make_release("", "", "");
    let ctx = PromoteContext {
        candidate: &candidate,
        current: None,
    };
    let rules = CompatRuleSet { rules: vec![] };
    let verdict = evaluate_compat(&rules, &ctx);
    assert!(verdict.passed());
    assert!(verdict.violations.is_empty());
}

#[test]
fn compat_verdict_serde_roundtrip() {
    let verdict = CompatVerdict {
        violations: vec![CompatViolation {
            rule: CompatRule::SpecDigestValid,
            reason: "bad digest".to_string(),
        }],
    };
    let json = serde_json::to_string(&verdict).expect("serialize");
    let deserialized: CompatVerdict = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(verdict, deserialized);
}
