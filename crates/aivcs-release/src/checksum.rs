//! SHA-256 helpers for release artifacts.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Write `SHA256SUMS` in `shasum -a 256` compatible form (`<hex>  <name>`).
pub fn write_sha256sums(dir: &Path, names: &[&str]) -> Result<Vec<(String, String)>> {
    let mut out = fs::File::create(dir.join("SHA256SUMS")).context("create SHA256SUMS")?;
    let mut pairs = Vec::new();
    for name in names {
        let digest = sha256_file(&dir.join(name))?;
        writeln!(out, "{digest}  {name}")?;
        pairs.push((name.to_string(), digest));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn checksum_roundtrip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("aivcs-darwin-arm64");
        fs::write(&p, b"fake-binary").unwrap();
        let d = sha256_file(&p).unwrap();
        assert_eq!(d.len(), 64);
        let pairs = write_sha256sums(dir.path(), &["aivcs-darwin-arm64"]).unwrap();
        assert_eq!(pairs[0].1, d);
    }
}
