//! Risk tiers for human-in-the-loop approval routing.

use serde::{Deserialize, Serialize};

/// Risk tier assigned to an action or checkpoint.
///
/// Higher risk actions require more stringent approval workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    /// Informational — no approval needed, logged only.
    Low,
    /// Moderate risk — single reviewer approval sufficient.
    Medium,
    /// High risk — always requires explicit human approval.
    High,
    /// Critical — requires multiple approvals and cannot be auto-approved.
    Critical,
}

impl RiskTier {
    /// Whether this tier requires at least one human approval.
    pub fn requires_approval(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    /// Minimum number of approvals needed for this tier.
    pub fn min_approvals(self) -> u32 {
        match self {
            Self::Low => 0,
            Self::Medium => 0,
            Self::High => 1,
            Self::Critical => 2,
        }
    }
}

impl std::fmt::Display for RiskTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_tier_ordering() {
        assert!(RiskTier::Low < RiskTier::Medium);
        assert!(RiskTier::Medium < RiskTier::High);
        assert!(RiskTier::High < RiskTier::Critical);
    }

    #[test]
    fn test_requires_approval() {
        assert!(!RiskTier::Low.requires_approval());
        assert!(!RiskTier::Medium.requires_approval());
        assert!(RiskTier::High.requires_approval());
        assert!(RiskTier::Critical.requires_approval());
    }

    #[test]
    fn test_min_approvals() {
        assert_eq!(RiskTier::Low.min_approvals(), 0);
        assert_eq!(RiskTier::Medium.min_approvals(), 0);
        assert_eq!(RiskTier::High.min_approvals(), 1);
        assert_eq!(RiskTier::Critical.min_approvals(), 2);
    }

    #[test]
    fn test_serde_roundtrip() {
        for tier in [
            RiskTier::Low,
            RiskTier::Medium,
            RiskTier::High,
            RiskTier::Critical,
        ] {
            let json = serde_json::to_string(&tier).unwrap();
            let back: RiskTier = serde_json::from_str(&json).unwrap();
            assert_eq!(tier, back);
        }
    }
}
