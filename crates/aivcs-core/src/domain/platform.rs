//! Platform detection and environment validation.

use std::fs;
use std::path::Path;

/// Supported platforms for AIVCS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    GenericLinux,
    MacOS,
    NixOS,
    NixOSWSL,
    Unknown,
}

impl Platform {
    /// Detect the current platform.
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            return Self::MacOS;
        }

        if cfg!(target_os = "linux") {
            let is_nixos = Path::new("/etc/NIXOS").exists()
                || fs::read_to_string("/etc/os-release")
                    .map(|v| v.contains("ID=nixos"))
                    .unwrap_or(false);

            let is_wsl = fs::read_to_string("/proc/version")
                .map(|v| v.to_lowercase().contains("microsoft") || v.to_lowercase().contains("wsl"))
                .unwrap_or(false);

            return match (is_nixos, is_wsl) {
                (true, true) => Self::NixOSWSL,
                (true, false) => Self::NixOS,
                (false, _) => Self::GenericLinux,
            };
        }

        Self::Unknown
    }

    pub fn is_wsl(&self) -> bool {
        matches!(self, Self::NixOSWSL)
    }

    pub fn is_nixos(&self) -> bool {
        matches!(self, Self::NixOS | Self::NixOSWSL)
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GenericLinux => write!(f, "generic-linux"),
            Self::MacOS => write!(f, "macos"),
            Self::NixOS => write!(f, "nixos"),
            Self::NixOSWSL => write!(f, "nixos-wsl"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Validation result for the environment.
pub struct EnvValidation {
    pub platform: Platform,
    pub nix_available: bool,
    pub attic_available: bool,
    pub is_nix_shell: bool,
    pub tmpdir: Option<String>,
}

impl EnvValidation {
    pub fn check() -> Self {
        Self {
            platform: Platform::detect(),
            nix_available: is_command_available("nix"),
            attic_available: is_command_available("attic"),
            is_nix_shell: std::env::var("IN_NIX_SHELL").is_ok()
                || (std::env::var("SHLVL").map(|v| v == "2").unwrap_or(false)
                    && std::env::var("PATH")
                        .map(|v| v.contains("/nix/store"))
                        .unwrap_or(false)),
            tmpdir: std::env::var("TMPDIR").ok().or_else(|| {
                std::env::var("TEMP")
                    .ok()
                    .or_else(|| std::env::var("TMP").ok())
            }),
        }
    }

    /// Actionable guidance for operators, with WSL-specific hints when relevant.
    pub fn recommendations(&self) -> Vec<String> {
        let mut tips = Vec::new();

        if self.platform.is_wsl() {
            tips.push(
                "Keep AIVCS CAS and .aivcs/ state on the Linux filesystem (/home/...), not /mnt/c, to avoid WSL 9p rename errors.".into(),
            );

            if let Some(tmp) = &self.tmpdir {
                if tmp.starts_with("/mnt/") || tmp.contains(':') {
                    tips.push(
                        format!("TMPDIR is currently '{}'. Using a Windows-hosted temp directory in WSL is extremely slow and causes Nix build failures. Run `export TMPDIR=/tmp`.", tmp)
                    );
                }
            }

            if !self.is_nix_shell {
                tips.push(
                    "Enter a Nix shell (`nix develop`) or install the NixOS-WSL tarball before running CI locally.".into(),
                );
            }
        }

        if self.platform.is_nixos() && !self.nix_available {
            tips.push(
                "Nix was not found on PATH. Enable programs.nix or install nix in your NixOS configuration.".into(),
            );
        }

        if !self.attic_available {
            tips.push(
                "Attic is optional but recommended for faster rebuilds. Set ATTIC_SERVER and ATTIC_CACHE, or see docs/runbooks/nixos-wsl.md.".into(),
            );
        }

        tips
    }
}

fn is_command_available(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection_smoke() {
        let p = Platform::detect();
        if cfg!(target_os = "macos") {
            assert_eq!(p, Platform::MacOS);
        }
    }

    #[test]
    fn test_platform_display() {
        assert_eq!(Platform::NixOSWSL.to_string(), "nixos-wsl");
    }

    #[test]
    fn wsl_recommendations_include_linux_filesystem_hint() {
        let validation = EnvValidation {
            platform: Platform::NixOSWSL,
            nix_available: true,
            attic_available: false,
            is_nix_shell: false,
            tmpdir: Some("/mnt/c/Temp".to_string()),
        };
        let tips = validation.recommendations();
        assert!(tips.iter().any(|t| t.contains("/mnt/c")));
        assert!(tips
            .iter()
            .any(|t| t.contains("TMPDIR is currently '/mnt/c/Temp'")));
        assert!(tips.iter().any(|t| t.contains("nix develop")));
    }
}
