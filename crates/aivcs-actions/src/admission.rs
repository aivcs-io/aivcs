//! apps-middle-ware admission: the fail-closed checks an intent must pass before
//! Propel authorizes it and Sandlot derives a job. These are the schema/
//! provenance rules apps-middle-ware owns — NOT policy authorization (repository/
//! kind/environment/risk decisions belong to Propel) and NOT approval (heirloom-
//! crab). Everything here rejects by default.

use chrono::{DateTime, Duration, Utc};

use crate::intent::{AutomationIntent, SCHEMA_VERSION};

/// Why an intent was refused at admission. All variants are hard rejects.
/// `#[non_exhaustive]`: new rejection reasons are additive; callers handle the
/// `Err` generically and must not exhaustively match every reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AdmissionError {
    #[error("unsupported schema_version {got:?}; expected {expected:?}")]
    UnsupportedSchemaVersion { got: String, expected: &'static str },
    #[error("repository {0:?} is not in canonical owner/name form")]
    MalformedRepository(String),
    #[error("repository {0:?} is not declared in the admission allow-list")]
    UndeclaredRepository(String),
    #[error("ref {0:?} is mutable or malformed; an immutable commit SHA is required")]
    MutableRef(String),
    #[error("kind {kind} does not match the supplied parameters ({params})")]
    KindParametersMismatch {
        kind: &'static str,
        params: &'static str,
    },
    #[error("requested_at {requested_at} is outside the acceptance window (now {now})")]
    TimestampOutOfWindow {
        requested_at: DateTime<Utc>,
        now: DateTime<Utc>,
    },
    #[error("missing provenance: {0}")]
    MissingProvenance(&'static str),
    #[error("empty required parameter: {0}")]
    EmptyParameter(&'static str),
}

/// Admission bounds. `allowed_repositories` is a deny-by-default allow-list: an
/// empty list admits nothing (undeclared repositories fail closed).
#[derive(Debug, Clone)]
pub struct AdmissionConfig {
    pub allowed_repositories: Vec<String>,
    /// Reject intents older than this (stale replays).
    pub max_age: Duration,
    /// Tolerated forward clock skew for `requested_at`.
    pub max_clock_skew: Duration,
}

impl AdmissionConfig {
    /// Sensible defaults: 15-minute acceptance window, 2-minute forward skew.
    pub fn new(allowed_repositories: Vec<String>) -> Self {
        Self {
            allowed_repositories,
            max_age: Duration::minutes(15),
            max_clock_skew: Duration::minutes(2),
        }
    }
}

/// Validate an intent against the v1 contract. `now` is injected so the check is
/// deterministic and testable. Returns the first failing rule; order is fixed so
/// callers get a stable reason.
pub fn validate_intent(
    intent: &AutomationIntent,
    now: DateTime<Utc>,
    cfg: &AdmissionConfig,
) -> Result<(), AdmissionError> {
    // 1. Contract version.
    if intent.schema_version != SCHEMA_VERSION {
        return Err(AdmissionError::UnsupportedSchemaVersion {
            got: intent.schema_version.clone(),
            expected: SCHEMA_VERSION,
        });
    }

    // 2. Provenance must be present (source/kind are enums = always present).
    if intent.idempotency_key.trim().is_empty() {
        return Err(AdmissionError::MissingProvenance("idempotency_key"));
    }
    if intent.requested_by.trim().is_empty() {
        return Err(AdmissionError::MissingProvenance("requested_by"));
    }
    if intent.trace_id.trim().is_empty() {
        return Err(AdmissionError::MissingProvenance("trace_id"));
    }

    // 3. Repository is canonical owner/name AND declared.
    if !is_owner_name(&intent.repository) {
        return Err(AdmissionError::MalformedRepository(
            intent.repository.clone(),
        ));
    }
    if !cfg
        .allowed_repositories
        .iter()
        .any(|r| r == &intent.repository)
    {
        return Err(AdmissionError::UndeclaredRepository(
            intent.repository.clone(),
        ));
    }

    // 4. Ref must be an immutable commit SHA (branch names are metadata only).
    if !is_commit_sha(&intent.git_ref) {
        return Err(AdmissionError::MutableRef(intent.git_ref.clone()));
    }

    // 5. kind must match the parameters variant.
    if intent.kind != intent.parameters.kind() {
        return Err(AdmissionError::KindParametersMismatch {
            kind: kind_str(intent.kind),
            params: kind_str(intent.parameters.kind()),
        });
    }

    // 5b. Required references inside the parameters must be present. This is the
    // schema-completeness line apps-middle-ware owns; WHICH builders/registries
    // are permitted is Propel policy and is deliberately not checked here.
    validate_parameters(&intent.parameters)?;

    // 6. Timestamp inside the acceptance window (not stale, not far-future).
    let age = now - intent.requested_at;
    if age > cfg.max_age || (intent.requested_at - now) > cfg.max_clock_skew {
        return Err(AdmissionError::TimestampOutOfWindow {
            requested_at: intent.requested_at,
            now,
        });
    }

    Ok(())
}

/// Reject a schema-incomplete parameter set: the identity/target references a
/// kind cannot function without must be non-empty. Registry destinations and
/// allowed builders are intentionally NOT checked here — which ones are permitted
/// is Propel policy, not apps-middle-ware schema validation.
fn validate_parameters(params: &crate::intent::IntentParameters) -> Result<(), AdmissionError> {
    use crate::intent::IntentParameters::*;
    let empty = |s: &str| s.trim().is_empty();
    match params {
        Ci(p) if empty(&p.check_graph_ref) => {
            Err(AdmissionError::EmptyParameter("ci.check_graph_ref"))
        }
        Build(p) if empty(&p.build_intent_id) => {
            Err(AdmissionError::EmptyParameter("build.build_intent_id"))
        }
        Build(p) if empty(&p.build_intent_version) => {
            Err(AdmissionError::EmptyParameter("build.build_intent_version"))
        }
        Build(p) if empty(&p.target) => Err(AdmissionError::EmptyParameter("build.target")),
        Reconcile(p) if empty(&p.change_ref) => {
            Err(AdmissionError::EmptyParameter("reconcile.change_ref"))
        }
        Ci(_) | Build(_) | Reconcile(_) => Ok(()),
    }
}

fn kind_str(kind: crate::intent::IntentKind) -> &'static str {
    use crate::intent::IntentKind::*;
    match kind {
        Ci => "ci",
        Build => "build",
        Reconcile => "reconcile",
    }
}

/// A commit SHA is 40 (sha-1) or 64 (sha-256) lowercase hex chars. This rejects
/// branch names, tags, `HEAD`, and short SHAs — the mutable-ref class.
fn is_commit_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Exactly two non-empty DNS-ish segments (`owner/name`), each 1–100 chars of
/// `[a-z0-9._-]`. Blocks traversal, extra segments, and empty halves.
fn is_owner_name(s: &str) -> bool {
    let mut parts = s.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|seg| {
        !seg.is_empty()
            && seg.len() <= 100
            && seg != &"."
            && seg != &".."
            && seg.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{
        ApprovalClass, BuildParameters, CiParameters, IntentKind, IntentParameters, IntentSource,
        PolicyContext, ReconcileParameters, RiskClass,
    };
    use uuid::Uuid;

    fn now() -> DateTime<Utc> {
        "2026-08-09T12:00:00Z".parse().unwrap()
    }

    fn cfg() -> AdmissionConfig {
        AdmissionConfig::new(vec!["lornu-ai/apps-middle-ware".into()])
    }

    fn valid_intent() -> AutomationIntent {
        AutomationIntent {
            schema_version: SCHEMA_VERSION.to_string(),
            intent_id: Uuid::nil(),
            idempotency_key: "idem-1".into(),
            requested_at: now(),
            requested_by: "spiffe://lornu/propel".into(),
            source: IntentSource::Propel,
            repository: "lornu-ai/apps-middle-ware".into(),
            git_ref: "a".repeat(40),
            kind: IntentKind::Ci,
            parameters: IntentParameters::Ci(CiParameters {
                check_graph_ref: "checks/default".into(),
                check_graph_version: None,
            }),
            policy_context: PolicyContext {
                policy_version: "2026-08-01".into(),
                environment: "prod".into(),
                risk_class: RiskClass::Low,
                approval_class: ApprovalClass::NotRequired,
            },
            trace_id: "trace-1".into(),
        }
    }

    #[test]
    fn a_well_formed_intent_is_admitted() {
        assert_eq!(validate_intent(&valid_intent(), now(), &cfg()), Ok(()));
        // sha-256 length ref is also accepted
        let mut i = valid_intent();
        i.git_ref = "b".repeat(64);
        assert_eq!(validate_intent(&i, now(), &cfg()), Ok(()));
    }

    #[test]
    fn unknown_schema_version_fails_closed() {
        let mut i = valid_intent();
        i.schema_version = "aivcs.actions/v2".into();
        assert!(matches!(
            validate_intent(&i, now(), &cfg()),
            Err(AdmissionError::UnsupportedSchemaVersion { .. })
        ));
    }

    #[test]
    fn mutable_refs_are_rejected() {
        for bad in [
            "main",
            "HEAD",
            "v1.2.3",
            &"a".repeat(7),
            &"A".repeat(40),
            &"g".repeat(40),
        ] {
            let mut i = valid_intent();
            i.git_ref = bad.to_string();
            assert!(
                matches!(
                    validate_intent(&i, now(), &cfg()),
                    Err(AdmissionError::MutableRef(_))
                ),
                "ref {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn undeclared_or_malformed_repository_fails_closed() {
        let mut i = valid_intent();
        i.repository = "lornu-ai/not-allowed".into();
        assert!(matches!(
            validate_intent(&i, now(), &cfg()),
            Err(AdmissionError::UndeclaredRepository(_))
        ));

        for bad in [
            "noslash",
            "a/b/c",
            "/name",
            "owner/",
            "../etc",
            "Owner/Name",
        ] {
            let mut i = valid_intent();
            i.repository = bad.into();
            let r = validate_intent(&i, now(), &cfg());
            assert!(
                matches!(
                    r,
                    Err(AdmissionError::MalformedRepository(_))
                        | Err(AdmissionError::UndeclaredRepository(_))
                ),
                "repo {bad:?} should be rejected, got {r:?}"
            );
        }
        // empty allow-list admits nothing (deny-by-default)
        let empty = AdmissionConfig::new(vec![]);
        assert!(matches!(
            validate_intent(&valid_intent(), now(), &empty),
            Err(AdmissionError::UndeclaredRepository(_))
        ));
    }

    #[test]
    fn kind_parameters_mismatch_is_rejected() {
        let mut i = valid_intent();
        i.kind = IntentKind::Build; // parameters are still Ci
        assert!(matches!(
            validate_intent(&i, now(), &cfg()),
            Err(AdmissionError::KindParametersMismatch { .. })
        ));
    }

    #[test]
    fn missing_provenance_is_rejected() {
        for mutate in [
            (|i: &mut AutomationIntent| i.idempotency_key = "  ".into()),
            (|i: &mut AutomationIntent| i.requested_by = "".into()),
            (|i: &mut AutomationIntent| i.trace_id = "".into()),
        ] {
            let mut i = valid_intent();
            mutate(&mut i);
            assert!(matches!(
                validate_intent(&i, now(), &cfg()),
                Err(AdmissionError::MissingProvenance(_))
            ));
        }
    }

    #[test]
    fn stale_and_far_future_timestamps_are_rejected() {
        // stale: 16 min old (> 15 min window)
        let mut stale = valid_intent();
        stale.requested_at = now() - Duration::minutes(16);
        assert!(matches!(
            validate_intent(&stale, now(), &cfg()),
            Err(AdmissionError::TimestampOutOfWindow { .. })
        ));
        // far future: 3 min ahead (> 2 min skew)
        let mut future = valid_intent();
        future.requested_at = now() + Duration::minutes(3);
        assert!(matches!(
            validate_intent(&future, now(), &cfg()),
            Err(AdmissionError::TimestampOutOfWindow { .. })
        ));
    }

    #[test]
    fn schema_incomplete_parameters_are_rejected() {
        // CI with a blank check-graph ref.
        let mut ci = valid_intent();
        ci.parameters = IntentParameters::Ci(CiParameters {
            check_graph_ref: "  ".into(),
            check_graph_version: None,
        });
        assert_eq!(
            validate_intent(&ci, now(), &cfg()),
            Err(AdmissionError::EmptyParameter("ci.check_graph_ref"))
        );

        // Build missing its target.
        let mut build = valid_intent();
        build.kind = IntentKind::Build;
        build.parameters = IntentParameters::Build(BuildParameters {
            build_intent_id: "bi".into(),
            build_intent_version: "1".into(),
            target: "".into(),
            registry_destinations: vec!["ecr/x".into()],
            tag_policy: "immutable".into(),
            allowed_builders: vec!["sandlot".into()],
        });
        assert_eq!(
            validate_intent(&build, now(), &cfg()),
            Err(AdmissionError::EmptyParameter("build.target"))
        );

        // Reconcile missing its change ref.
        let mut reconcile = valid_intent();
        reconcile.kind = IntentKind::Reconcile;
        reconcile.parameters = IntentParameters::Reconcile(ReconcileParameters {
            change_ref: "".into(),
        });
        assert_eq!(
            validate_intent(&reconcile, now(), &cfg()),
            Err(AdmissionError::EmptyParameter("reconcile.change_ref"))
        );

        // A build with a valid identity+target but EMPTY registry/builders is
        // admitted here — which registries/builders are permitted is Propel
        // policy, not apps-middle-ware schema validation.
        let mut policy_only = valid_intent();
        policy_only.kind = IntentKind::Build;
        policy_only.parameters = IntentParameters::Build(BuildParameters {
            build_intent_id: "bi".into(),
            build_intent_version: "1".into(),
            target: "packages.x86_64-linux.oci".into(),
            registry_destinations: vec![],
            tag_policy: "immutable".into(),
            allowed_builders: vec![],
        });
        assert_eq!(validate_intent(&policy_only, now(), &cfg()), Ok(()));
    }
}
