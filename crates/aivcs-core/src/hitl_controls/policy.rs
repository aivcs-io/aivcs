//! Approval policy rules and configuration.

use serde::{Deserialize, Serialize};

use super::risk::RiskTier;

/// A single approval policy rule that maps a label pattern to a risk tier.
///
/// Rules are evaluated first-match-wins against checkpoint labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRule {
    /// Pattern to match against checkpoint labels (substring match).
    pub label_pattern: String,
    /// Risk tier assigned when this rule matches.
    pub risk_tier: RiskTier,
    /// Timeout in seconds for the approval checkpoint. `None` means no timeout.
    pub timeout_secs: Option<u64>,
}

impl ApprovalRule {
    /// Create a new rule.
    pub fn new(
        label_pattern: impl Into<String>,
        risk_tier: RiskTier,
        timeout_secs: Option<u64>,
    ) -> Self {
        Self {
            label_pattern: label_pattern.into(),
            risk_tier,
            timeout_secs,
        }
    }

    /// Returns `true` if this rule matches the given checkpoint label.
    pub fn matches(&self, label: &str) -> bool {
        label.contains(&self.label_pattern)
    }
}

/// An ordered set of approval rules evaluated first-match-wins.
///
/// If no rule matches, the default risk tier is `Low` (no approval needed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    pub rules: Vec<ApprovalRule>,
    /// Default timeout for checkpoints not matched by a specific rule.
    pub default_timeout_secs: Option<u64>,
}

impl ApprovalPolicy {
    /// An empty policy where everything defaults to low risk.
    pub fn permissive() -> Self {
        Self {
            rules: Vec::new(),
            default_timeout_secs: None,
        }
    }

    /// Append a rule (builder pattern).
    pub fn with_rule(mut self, rule: ApprovalRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Look up the risk tier for a given checkpoint label.
    ///
    /// Returns the first matching rule's tier, or `Low` if nothing matches.
    pub fn evaluate_risk(&self, label: &str) -> (RiskTier, Option<u64>) {
        for rule in &self.rules {
            if rule.matches(label) {
                return (
                    rule.risk_tier,
                    rule.timeout_secs.or(self.default_timeout_secs),
                );
            }
        }
        (RiskTier::Low, self.default_timeout_secs)
    }

    /// Standard policy with sensible production defaults.
    ///
    /// | Pattern          | Tier     | Timeout |
    /// |------------------|----------|---------|
    /// | deploy-prod      | Critical | 600s    |
    /// | deploy-staging   | High     | 300s    |
    /// | publish          | High     | 300s    |
    /// | schema-migration | Critical | 900s    |
    /// | rollback         | High     | 180s    |
    pub fn standard() -> Self {
        Self {
            rules: vec![
                ApprovalRule::new("deploy-prod", RiskTier::Critical, Some(600)),
                ApprovalRule::new("deploy-staging", RiskTier::High, Some(300)),
                ApprovalRule::new("publish", RiskTier::High, Some(300)),
                ApprovalRule::new("schema-migration", RiskTier::Critical, Some(900)),
                ApprovalRule::new("rollback", RiskTier::High, Some(180)),
            ],
            default_timeout_secs: Some(300),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_matches() {
        let rule = ApprovalRule::new("deploy-prod", RiskTier::Critical, None);
        assert!(rule.matches("deploy-prod-us-east"));
        assert!(rule.matches("deploy-prod"));
        assert!(!rule.matches("deploy-staging"));
    }

    #[test]
    fn test_evaluate_risk_first_match() {
        let policy = ApprovalPolicy::standard();
        let (tier, timeout) = policy.evaluate_risk("deploy-prod-us-east");
        assert_eq!(tier, RiskTier::Critical);
        assert_eq!(timeout, Some(600));
    }

    #[test]
    fn test_evaluate_risk_default_low() {
        let policy = ApprovalPolicy::standard();
        let (tier, timeout) = policy.evaluate_risk("run-unit-tests");
        assert_eq!(tier, RiskTier::Low);
        assert_eq!(timeout, Some(300)); // default timeout
    }

    #[test]
    fn test_permissive_policy() {
        let policy = ApprovalPolicy::permissive();
        let (tier, timeout) = policy.evaluate_risk("deploy-prod");
        assert_eq!(tier, RiskTier::Low);
        assert_eq!(timeout, None);
    }

    #[test]
    fn test_with_rule_builder() {
        let policy = ApprovalPolicy::permissive().with_rule(ApprovalRule::new(
            "danger",
            RiskTier::High,
            Some(60),
        ));
        assert_eq!(policy.rules.len(), 1);
        let (tier, _) = policy.evaluate_risk("danger-zone");
        assert_eq!(tier, RiskTier::High);
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = ApprovalPolicy::standard();
        let json = serde_json::to_string(&policy).unwrap();
        let back: ApprovalPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }
}
