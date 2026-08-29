//! Forbidden deployment hostnames / fleet secrets must not appear in shipped binaries.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

/// Substrings that must never appear in the shipped `aivcs` CLI binary.
pub const FORBIDDEN_DEPLOYMENT_PATTERNS: &[&str] = &[
    "forge-v2.aivcs.io",
    "issuer.aivcs.io",
    "aivcsd.aivcs.io",
    "forge-v2-token",
    "import@aivcs.io",
];

/// Scan raw bytes for forbidden patterns (portable — no `strings(1)` required).
pub fn audit_binary_bytes(bytes: &[u8], patterns: &[&str]) -> Result<(), Vec<String>> {
    let mut hits = Vec::new();
    for pat in patterns {
        let needle = pat.as_bytes();
        if bytes.windows(needle.len()).any(|w| w == needle) {
            hits.push((*pat).to_string());
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        Err(hits)
    }
}

pub fn audit_binary_file(path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match audit_binary_bytes(&bytes, FORBIDDEN_DEPLOYMENT_PATTERNS) {
        Ok(()) => Ok(()),
        Err(hits) => {
            bail!(
                "{} contains forbidden deployment strings: {}",
                path.display(),
                hits.join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_bytes_pass() {
        assert!(audit_binary_bytes(b"hello aivcs release", FORBIDDEN_DEPLOYMENT_PATTERNS).is_ok());
    }

    #[test]
    fn detects_forge_host() {
        let blob = b"xxxforge-v2.aivcs.iovvv";
        let err = audit_binary_bytes(blob, FORBIDDEN_DEPLOYMENT_PATTERNS).unwrap_err();
        assert!(err.iter().any(|h| h == "forge-v2.aivcs.io"));
    }
}
