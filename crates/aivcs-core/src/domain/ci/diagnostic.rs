//! Normalized CI diagnostic types.

use serde::{Deserialize, Serialize};

/// Severity level for a diagnostic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Hint,
    Warning,
    Error,
}

/// Source tool that produced a diagnostic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    Rustc,
    Clippy,
    Fmt,
    Test,
    Custom,
}

/// A single normalized diagnostic from CI output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    /// Severity level.
    pub severity: Severity,

    /// Diagnostic/lint code (e.g. "clippy::needless_return").
    pub code: Option<String>,

    /// Human-readable message.
    pub message: String,

    /// Source file path (relative to workspace root).
    pub file: Option<String>,

    /// Line number (1-indexed).
    pub line: Option<u32>,

    /// Column number (1-indexed).
    pub column: Option<u32>,

    /// Which tool produced this diagnostic.
    pub source: DiagnosticSource,

    /// Evidence snippet from the original output.
    pub evidence: Option<String>,
}

impl Diagnostic {
    /// Create a new diagnostic.
    pub fn new(severity: Severity, message: String, source: DiagnosticSource) -> Self {
        Self {
            severity,
            code: None,
            message,
            file: None,
            line: None,
            column: None,
            source,
            evidence: None,
        }
    }

    /// Set file location.
    pub fn with_location(mut self, file: String, line: u32, column: u32) -> Self {
        self.file = Some(file);
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    /// Set diagnostic code.
    pub fn with_code(mut self, code: String) -> Self {
        self.code = Some(code);
        self
    }

    /// Set evidence snippet.
    pub fn with_evidence(mut self, evidence: String) -> Self {
        self.evidence = Some(evidence);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_serde() {
        let severities = [Severity::Hint, Severity::Warning, Severity::Error];
        for sev in &severities {
            let json = serde_json::to_string(sev).expect("serialize");
            let deserialized: Severity = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*sev, deserialized);
        }
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Hint < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn test_diagnostic_source_serde() {
        let sources = [
            DiagnosticSource::Rustc,
            DiagnosticSource::Clippy,
            DiagnosticSource::Fmt,
            DiagnosticSource::Test,
            DiagnosticSource::Custom,
        ];
        for src in &sources {
            let json = serde_json::to_string(src).expect("serialize");
            let deserialized: DiagnosticSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*src, deserialized);
        }
    }

    #[test]
    fn test_diagnostic_serde_roundtrip() {
        let diag = Diagnostic::new(
            Severity::Error,
            "unused variable `x`".to_string(),
            DiagnosticSource::Clippy,
        )
        .with_code("clippy::unused_variables".to_string())
        .with_location("src/main.rs".to_string(), 42, 9)
        .with_evidence("let x = 5;".to_string());

        let json = serde_json::to_string(&diag).expect("serialize");
        let deserialized: Diagnostic = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(diag, deserialized);
    }

    #[test]
    fn test_diagnostic_new_defaults() {
        let diag = Diagnostic::new(
            Severity::Warning,
            "test warning".to_string(),
            DiagnosticSource::Rustc,
        );
        assert!(diag.code.is_none());
        assert!(diag.file.is_none());
        assert!(diag.line.is_none());
        assert!(diag.column.is_none());
        assert!(diag.evidence.is_none());
    }
}
