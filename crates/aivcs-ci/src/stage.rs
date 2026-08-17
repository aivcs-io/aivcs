//! CI stage definitions and configuration.

use serde::{Deserialize, Serialize};

/// Builtin CI stages.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinStage {
    /// cargo fmt --all -- --check
    CargoFmt,

    /// cargo check --workspace
    CargoCheck,

    /// cargo clippy --workspace --all-targets -- -D warnings
    CargoClippy,

    /// cargo test --workspace
    CargoTest,
}

impl BuiltinStage {
    /// Get the stage name as a string.
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinStage::CargoFmt => "cargo_fmt",
            BuiltinStage::CargoCheck => "cargo_check",
            BuiltinStage::CargoClippy => "cargo_clippy",
            BuiltinStage::CargoTest => "cargo_test",
        }
    }

    /// Get the stage's main command.
    pub fn command(&self) -> Vec<String> {
        match self {
            BuiltinStage::CargoFmt => {
                vec![
                    "cargo".to_string(),
                    "fmt".to_string(),
                    "--all".to_string(),
                    "--".to_string(),
                    "--check".to_string(),
                ]
            }
            BuiltinStage::CargoCheck => {
                vec![
                    "cargo".to_string(),
                    "check".to_string(),
                    "--workspace".to_string(),
                ]
            }
            BuiltinStage::CargoClippy => {
                vec![
                    "cargo".to_string(),
                    "clippy".to_string(),
                    "--workspace".to_string(),
                    "--all-targets".to_string(),
                    "--".to_string(),
                    "-D".to_string(),
                    "warnings".to_string(),
                ]
            }
            BuiltinStage::CargoTest => {
                vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--workspace".to_string(),
                ]
            }
        }
    }

    /// Get the stage's auto-repair command (if available).
    pub fn fix_command(&self) -> Option<Vec<String>> {
        match self {
            BuiltinStage::CargoFmt => Some(vec![
                "cargo".to_string(),
                "fmt".to_string(),
                "--all".to_string(),
            ]),
            _ => None,
        }
    }
}

/// Configuration for a CI stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageConfig {
    /// Human-readable stage name.
    pub name: String,

    /// Command to execute (first element is executable).
    pub command: Vec<String>,

    /// Optional auto-repair command.
    pub fix_command: Option<Vec<String>>,

    /// Timeout in seconds.
    pub timeout_secs: u64,

    /// Whether this stage is enabled.
    pub enabled: bool,
}

impl StageConfig {
    /// Create a new stage configuration from a builtin stage.
    pub fn from_builtin(stage: BuiltinStage, timeout_secs: u64) -> Self {
        Self {
            name: stage.name().to_string(),
            command: stage.command(),
            fix_command: stage.fix_command(),
            timeout_secs,
            enabled: true,
        }
    }

    /// Create a custom stage configuration.
    pub fn custom(name: String, command: Vec<String>, timeout_secs: u64) -> Self {
        Self {
            name,
            command,
            fix_command: None,
            timeout_secs,
            enabled: true,
        }
    }

    /// Disable this stage.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_stage_names() {
        assert_eq!(BuiltinStage::CargoFmt.name(), "cargo_fmt");
        assert_eq!(BuiltinStage::CargoCheck.name(), "cargo_check");
        assert_eq!(BuiltinStage::CargoClippy.name(), "cargo_clippy");
        assert_eq!(BuiltinStage::CargoTest.name(), "cargo_test");
    }

    #[test]
    fn test_builtin_stage_commands() {
        let fmt_cmd = BuiltinStage::CargoFmt.command();
        assert_eq!(fmt_cmd[0], "cargo");
        assert!(fmt_cmd.contains(&"--check".to_string()));

        let check_cmd = BuiltinStage::CargoCheck.command();
        assert_eq!(check_cmd[0], "cargo");
        assert!(check_cmd.contains(&"check".to_string()));
    }

    #[test]
    fn test_builtin_stage_fix_command() {
        assert!(BuiltinStage::CargoFmt.fix_command().is_some());
        assert!(BuiltinStage::CargoCheck.fix_command().is_none());
        assert!(BuiltinStage::CargoClippy.fix_command().is_none());
        assert!(BuiltinStage::CargoTest.fix_command().is_none());
    }

    #[test]
    fn test_stage_config_from_builtin() {
        let config = StageConfig::from_builtin(BuiltinStage::CargoCheck, 300);
        assert_eq!(config.name, "cargo_check");
        assert_eq!(config.timeout_secs, 300);
        assert!(config.enabled);
    }

    #[test]
    fn test_stage_config_custom() {
        let config = StageConfig::custom(
            "my_stage".to_string(),
            vec!["echo".to_string(), "hello".to_string()],
            60,
        );
        assert_eq!(config.name, "my_stage");
        assert_eq!(config.timeout_secs, 60);
        assert!(config.enabled);
        assert!(config.fix_command.is_none());
    }

    #[test]
    fn test_stage_config_disabled() {
        let config = StageConfig::from_builtin(BuiltinStage::CargoCheck, 300).disabled();
        assert!(!config.enabled);
    }
}
