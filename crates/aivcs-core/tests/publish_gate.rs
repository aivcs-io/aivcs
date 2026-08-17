use aivcs_core::publish_gate::{
    evaluate_publish_gate, PublishCandidate, PublishRule, PublishRuleSet,
};

fn candidate(
    version: Option<&str>,
    previous: Option<&str>,
    existing: &[&str],
    notes: Option<&str>,
    spec_digest: &str,
) -> PublishCandidate {
    PublishCandidate {
        version_label: version.map(|s| s.to_string()),
        previous_version: previous.map(|s| s.to_string()),
        existing_versions: existing.iter().map(|s| s.to_string()).collect(),
        notes: notes.map(|s| s.to_string()),
        spec_digest: spec_digest.to_string(),
    }
}

// ---- SemverFormat ----

#[test]
fn valid_semver_passes() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::SemverFormat],
        fail_fast: false,
    };
    let c = candidate(Some("1.2.3"), None, &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

#[test]
fn invalid_semver_rejected() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::SemverFormat],
        fail_fast: false,
    };
    let c = candidate(Some("not-a-version"), None, &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(!v.passed);
    assert!(v
        .violations
        .iter()
        .any(|viol| matches!(&viol.rule, PublishRule::SemverFormat)));
}

#[test]
fn prerelease_semver_passes() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::SemverFormat],
        fail_fast: false,
    };
    let c = candidate(Some("1.0.0-rc.1"), None, &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

// ---- VersionBump ----

#[test]
fn version_bump_passes() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("1.1.0"), Some("1.0.0"), &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

#[test]
fn version_bump_rejected_when_downgrade() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("0.9.0"), Some("1.0.0"), &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(!v.passed);
    assert!(v
        .violations
        .iter()
        .any(|viol| matches!(&viol.rule, PublishRule::VersionBump)));
}

#[test]
fn version_bump_rejected_when_equal() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("1.0.0"), Some("1.0.0"), &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(!v.passed);
}

#[test]
fn version_bump_skipped_no_previous() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("1.0.0"), None, &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

#[test]
fn version_bump_rejects_unparseable_previous() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("1.0.0"), Some("not-a-version"), &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(
        !v.passed,
        "unparseable previous_version must not silently pass"
    );
    assert!(v
        .violations
        .iter()
        .any(|viol| matches!(&viol.rule, PublishRule::VersionBump)));
}

#[test]
fn version_bump_rejects_unparseable_current() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::VersionBump],
        fail_fast: false,
    };
    let c = candidate(Some("not-a-version"), Some("1.0.0"), &[], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(
        !v.passed,
        "unparseable version_label must not silently pass VersionBump"
    );
    assert!(v
        .violations
        .iter()
        .any(|viol| matches!(&viol.rule, PublishRule::VersionBump)));
}

// ---- UniqueVersion ----

#[test]
fn unique_version_passes() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::UniqueVersion],
        fail_fast: false,
    };
    let c = candidate(Some("2.0.0"), None, &["1.0.0", "1.1.0"], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

#[test]
fn unique_version_rejected_duplicate() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::UniqueVersion],
        fail_fast: false,
    };
    let c = candidate(Some("1.0.0"), None, &["1.0.0", "1.1.0"], None, "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(!v.passed);
    assert!(v
        .violations
        .iter()
        .any(|viol| matches!(&viol.rule, PublishRule::UniqueVersion)));
}

// ---- RequireNotes ----

#[test]
fn require_notes_passes() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::RequireNotes],
        fail_fast: false,
    };
    let c = candidate(None, None, &[], Some("Fixed bug #42"), "abc");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
}

#[test]
fn require_notes_rejected_empty() {
    let rs = PublishRuleSet {
        rules: vec![PublishRule::RequireNotes],
        fail_fast: false,
    };
    // None case
    let c1 = candidate(None, None, &[], None, "abc");
    assert!(!evaluate_publish_gate(&rs, &c1).passed);

    // Empty string case
    let c2 = candidate(None, None, &[], Some(""), "abc");
    assert!(!evaluate_publish_gate(&rs, &c2).passed);
}

// ---- Standard ruleset full pass ----

#[test]
fn standard_ruleset_full_pass() {
    let rs = PublishRuleSet::standard();
    let c = candidate(
        Some("1.1.0"),
        Some("1.0.0"),
        &["0.9.0", "1.0.0"],
        None,
        "sha256:abc123",
    );
    let v = evaluate_publish_gate(&rs, &c);
    assert!(v.passed);
    assert!(v.violations.is_empty());
}

// ---- Fail-fast ----

#[test]
fn fail_fast_stops_at_first_violation() {
    let rs = PublishRuleSet {
        rules: vec![
            PublishRule::SemverFormat,
            PublishRule::RequireNotes,
            PublishRule::RequireSpecDigest,
        ],
        fail_fast: true,
    };
    // All three rules will fail: bad semver, no notes, empty digest
    let c = candidate(Some("bad"), None, &[], None, "");
    let v = evaluate_publish_gate(&rs, &c);
    assert!(!v.passed);
    assert_eq!(
        v.violations.len(),
        1,
        "fail_fast should stop at first violation"
    );
}

// ---- Builder ----

#[test]
fn standard_constructor_has_three_rules() {
    let rs = PublishRuleSet::standard();
    assert_eq!(rs.rules.len(), 3);
    assert!(matches!(rs.rules[0], PublishRule::SemverFormat));
    assert!(matches!(rs.rules[1], PublishRule::VersionBump));
    assert!(matches!(rs.rules[2], PublishRule::RequireSpecDigest));
}

#[test]
fn with_rule_builder_appends() {
    let rs = PublishRuleSet::standard().with_rule(PublishRule::RequireNotes);
    assert_eq!(rs.rules.len(), 4);
}
