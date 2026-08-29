pub mod fs;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use thiserror::Error;

/// SHA-256 digest used as a content address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Compute the SHA-256 digest of `data`.
    pub fn compute(data: &[u8]) -> Self {
        let hash = Sha256::digest(data);
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);
        Self(bytes)
    }

    /// Return the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex-encoded string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.to_hex();
        write!(f, "Digest({})", hex.get(..12).unwrap_or(&hex))
    }
}

impl FromStr for Digest {
    type Err = CasError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bytes = hex::decode(s).map_err(|_| CasError::InvalidDigest(s.to_string()))?;
        if bytes.len() != 32 {
            return Err(CasError::InvalidDigest(s.to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

/// Errors from CAS operations.
#[derive(Debug, Error)]
pub enum CasError {
    #[error("blob not found: {0}")]
    NotFound(Digest),

    #[error("invalid digest hex: {0}")]
    InvalidDigest(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CasError>;

/// Content-addressed store interface.
pub trait CasStore: Send + Sync {
    /// Store `data` and return its digest. Deduplicates automatically.
    fn put(&self, data: &[u8]) -> Result<Digest>;

    /// Retrieve the blob for `digest`.
    fn get(&self, digest: &Digest) -> Result<Vec<u8>>;

    /// Check whether `digest` exists without reading the blob.
    fn exists(&self, digest: &Digest) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_display_fromstr_roundtrip() {
        let d = Digest::compute(b"hello world");
        let hex = d.to_string();
        assert_eq!(hex.len(), 64);
        let parsed: Digest = hex.parse().unwrap();
        assert_eq!(d, parsed);
    }

    #[test]
    fn digest_fromstr_invalid_hex() {
        assert!("not-valid-hex".parse::<Digest>().is_err());
    }

    #[test]
    fn digest_fromstr_wrong_length() {
        assert!("abcd".parse::<Digest>().is_err());
    }

    #[test]
    fn digest_deterministic() {
        let a = Digest::compute(b"test data");
        let b = Digest::compute(b"test data");
        assert_eq!(a, b);
    }

    #[test]
    fn digest_different_data_different_hash() {
        let a = Digest::compute(b"data a");
        let b = Digest::compute(b"data b");
        assert_ne!(a, b);
    }
}
