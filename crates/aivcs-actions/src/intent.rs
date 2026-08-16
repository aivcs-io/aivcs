//! The public **`AutomationIntent`** — AIVCS Actions v1.
//!
//! A caller (Propel, oci-builds, heirloom-crab, or the bootstrap lane) submits a
//! versioned `AutomationIntent`. apps-middle-ware authenticates and validates it
//! (see [`crate::admission`]); Sandlot only ever receives a job *derived* from an
//! admitted intent, never the raw intent. Branch names are metadata only — the
//! `ref` is an immutable commit SHA.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The one contract version this crate speaks. apps-middle-ware rejects anything
/// else (`aivcs.actions/v1`).
pub const SCHEMA_VERSION: &str = "aivcs.actions/v1";

/// The initial v1 kinds. The FR specifies these are the kinds "initially", so
/// more are expected — `#[non_exhaustive]` lets a future kind be added without
/// breaking every downstream `match` (an unknown kind is still rejected at
/// deserialization/admission, which is the correct fail-closed behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IntentKind {
    Ci,
    Build,
    Reconcile,
}

/// Who submitted the intent. `bootstrap` is the narrowly-scoped lane that stands
/// Sandlot up before it can dogfood the contract. `#[non_exhaustive]` for the
/// same forward-compatibility reason as [`IntentKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum IntentSource {
    Propel,
    OciBuilds,
    HeirloomCrab,
    Bootstrap,
}

/// Risk class drives whether approval is required before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Low,
    Standard,
    High,
    Critical,
}

/// Declared approval requirement. Destructive/production/credential/registry/
/// rollback work is not `not_required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalClass {
    NotRequired,
    Single,
    DualControl,
}

/// Policy metadata carried with every intent; Propel authorizes against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyContext {
    pub policy_version: String,
    pub environment: String,
    pub risk_class: RiskClass,
    pub approval_class: ApprovalClass,
}

/// CI references a **versioned check graph**, never an arbitrary shell command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiParameters {
    pub check_graph_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_graph_version: Option<String>,
}

/// Build carries the resolved build-intent identity, target, and registry policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildParameters {
    pub build_intent_id: String,
    pub build_intent_version: String,
    /// Flake attribute or build target.
    pub target: String,
    pub registry_destinations: Vec<String>,
    pub tag_policy: String,
    pub allowed_builders: Vec<String>,
}

/// Reconcile references an **approved declarative change**, never an imperative
/// command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileParameters {
    pub change_ref: String,
}

/// Typed, allow-listed parameters for the selected kind. Externally tagged so the
/// wire form is explicit (`{"ci": {…}}`) and `kind` can be cross-checked against
/// it at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentParameters {
    Ci(CiParameters),
    Build(BuildParameters),
    Reconcile(ReconcileParameters),
}

impl IntentParameters {
    /// The kind these parameters are for — used to reject a `kind`/`parameters`
    /// mismatch at admission.
    pub fn kind(&self) -> IntentKind {
        match self {
            IntentParameters::Ci(_) => IntentKind::Ci,
            IntentParameters::Build(_) => IntentKind::Build,
            IntentParameters::Reconcile(_) => IntentKind::Reconcile,
        }
    }

    /// The build target, if this is a build (used to form the concurrency key).
    pub fn target(&self) -> Option<&str> {
        match self {
            IntentParameters::Build(b) => Some(&b.target),
            _ => None,
        }
    }
}

/// The versioned public contract. This is the ONLY shape a caller submits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationIntent {
    pub schema_version: String,
    pub intent_id: Uuid,
    pub idempotency_key: String,
    pub requested_at: DateTime<Utc>,
    pub requested_by: String,
    pub source: IntentSource,
    /// Canonical `owner/name`.
    pub repository: String,
    /// Immutable commit SHA. Wire name is `ref`; branch names are metadata only.
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub kind: IntentKind,
    pub parameters: IntentParameters,
    pub policy_context: PolicyContext,
    pub trace_id: String,
}

impl AutomationIntent {
    /// The concurrency key that prevents duplicate active work:
    /// `repository + ref + kind + target`. Supersession is a separate, explicit
    /// policy decision — this key only detects the collision.
    pub fn concurrency_key(&self) -> String {
        let kind = match self.kind {
            IntentKind::Ci => "ci",
            IntentKind::Build => "build",
            IntentKind::Reconcile => "reconcile",
        };
        let target = self.parameters.target().unwrap_or("-");
        format!("{}@{}#{}:{}", self.repository, self.git_ref, kind, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ci() -> AutomationIntent {
        AutomationIntent {
            schema_version: SCHEMA_VERSION.to_string(),
            intent_id: Uuid::nil(),
            idempotency_key: "idem-1".into(),
            requested_at: "2026-08-09T00:00:00Z".parse().unwrap(),
            requested_by: "spiffe://lornu/propel".into(),
            source: IntentSource::Propel,
            repository: "lornu-ai/apps-middle-ware".into(),
            git_ref: "e".repeat(40),
            kind: IntentKind::Ci,
            parameters: IntentParameters::Ci(CiParameters {
                check_graph_ref: "checks/default".into(),
                check_graph_version: Some("v3".into()),
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
    fn intent_round_trips_through_json_with_the_wire_names() {
        let intent = sample_ci();
        let json = serde_json::to_value(&intent).unwrap();
        // `ref` is the wire name (not `git_ref`); source uses kebab-case.
        assert_eq!(json["ref"], "e".repeat(40));
        assert_eq!(json["source"], "propel");
        assert_eq!(json["kind"], "ci");
        // parameters are externally tagged by kind
        assert!(json["parameters"]["ci"]["check_graph_ref"] == "checks/default");
        let back: AutomationIntent = serde_json::from_value(json).unwrap();
        assert_eq!(back, intent);
    }

    #[test]
    fn parameters_report_their_kind_and_target() {
        let ci = IntentParameters::Ci(CiParameters {
            check_graph_ref: "c".into(),
            check_graph_version: None,
        });
        assert_eq!(ci.kind(), IntentKind::Ci);
        assert_eq!(ci.target(), None);

        let build = IntentParameters::Build(BuildParameters {
            build_intent_id: "bi".into(),
            build_intent_version: "1".into(),
            target: "packages.x86_64-linux.oci".into(),
            registry_destinations: vec!["ecr/foo".into()],
            tag_policy: "immutable".into(),
            allowed_builders: vec!["sandlot".into()],
        });
        assert_eq!(build.kind(), IntentKind::Build);
        assert_eq!(build.target(), Some("packages.x86_64-linux.oci"));
    }

    #[test]
    fn concurrency_key_includes_repo_ref_kind_and_target() {
        let ci = sample_ci();
        assert_eq!(
            ci.concurrency_key(),
            format!("lornu-ai/apps-middle-ware@{}#ci:-", "e".repeat(40))
        );
    }
}
