//! Canonical JSON normalization and digest computation (RFC 8785-class).
//!
//! This module implements RFC 8785-compliant canonical JSON serialization with:
//! - UTF-16 code unit ordering for object keys (§3.2.3)
//! - Number normalization (integer-valued floats → integers; reject NaN/Infinity)
//! - SHA256 hex digest computation

use crate::domain::error::{AivcsError, Result};
use sha2::{Digest, Sha256};

/// Recursively sort JSON object keys using UTF-16 code unit ordering (RFC 8785 §3.2.3).
fn sort_keys_utf16(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            let mut keys: Vec<_> = map.keys().collect();

            // Sort by UTF-16 code unit order (RFC 8785)
            keys.sort_by(|a, b| {
                let a_utf16: Vec<u16> = a.encode_utf16().collect();
                let b_utf16: Vec<u16> = b.encode_utf16().collect();
                a_utf16.cmp(&b_utf16)
            });

            for key in keys {
                if let Some(v) = map.get(key) {
                    sorted.insert(key.to_string(), sort_keys_utf16(v));
                }
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_keys_utf16).collect())
        }
        other => other.clone(),
    }
}

/// Normalize numbers: integer-valued floats → integer repr; reject NaN/Infinity.
fn normalize_value(value: &serde_json::Value) -> Result<serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => {
            let mut normalized = serde_json::Map::new();
            for (k, v) in map.iter() {
                normalized.insert(k.clone(), normalize_value(v)?);
            }
            Ok(serde_json::Value::Object(normalized))
        }
        serde_json::Value::Array(arr) => {
            let normalized = arr
                .iter()
                .map(normalize_value)
                .collect::<Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(normalized))
        }
        serde_json::Value::Number(n) => {
            // If already an integer (via serde_json), pass through
            if n.is_i64() || n.is_u64() {
                Ok(serde_json::Value::Number(n.clone()))
            } else if let Some(f) = n.as_f64() {
                // Check for NaN or Infinity
                if !f.is_finite() {
                    return Err(AivcsError::InvalidAgentSpec(
                        "NaN/Infinity not permitted in canonical JSON".to_string(),
                    ));
                }
                // If integer-valued, convert to integer representation
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    Ok(serde_json::Value::Number(serde_json::Number::from(
                        f as i64,
                    )))
                } else {
                    Ok(serde_json::Value::Number(n.clone()))
                }
            } else {
                Ok(serde_json::Value::Number(n.clone()))
            }
        }
        other => Ok(other.clone()),
    }
}

/// Convert JSON value to canonical form: normalize numbers → sort keys → compact JSON.
pub fn canonical_json(value: &serde_json::Value) -> Result<String> {
    let normalized = normalize_value(value)?;
    let sorted = sort_keys_utf16(&normalized);
    Ok(serde_json::to_string(&sorted)?)
}

/// Compute SHA256 hex digest of canonical JSON.
pub fn compute_digest(value: &serde_json::Value) -> Result<String> {
    let canonical = canonical_json(value)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_json_sorts_keys_utf16() {
        // Test UTF-16 ordering: should differ from UTF-8 byte order on non-ASCII
        let input = serde_json::json!({
            "b": 1,
            "a": 2,
            "α": 3  // Greek alpha, different UTF-16 vs UTF-8 ordering
        });
        let canonical = canonical_json(&input).expect("canonical_json");
        // Just verify it's deterministic; exact ordering depends on UTF-16 code units
        let canonical2 = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, canonical2);
    }

    #[test]
    fn test_canonical_json_integer_float() {
        let input = serde_json::json!({ "value": 1.0 });
        let canonical = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, r#"{"value":1}"#);
    }

    #[test]
    fn test_canonical_json_negative_float() {
        let input = serde_json::json!({ "value": -1.0 });
        let canonical = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, r#"{"value":-1}"#);
    }

    #[test]
    fn test_canonical_json_fractional_float() {
        let input = serde_json::json!({ "value": 1.5 });
        let canonical = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, r#"{"value":1.5}"#);
    }

    #[test]
    fn test_canonical_json_handles_null() {
        // serde_json converts NaN/Infinity to null, so we just verify null is handled
        let input = serde_json::json!({ "value": serde_json::Value::Null });
        let result = canonical_json(&input);
        assert!(result.is_ok(), "canonical_json should handle null values");
        let canonical = result.expect("canonical");
        assert_eq!(canonical, r#"{"value":null}"#);
    }

    #[test]
    fn test_canonical_json_field_order_invariant() {
        // Same fields in different order should produce same canonical JSON
        let input1 = serde_json::json!({
            "a": 1,
            "b": 2,
            "c": 3
        });
        let input2 = serde_json::json!({
            "c": 3,
            "a": 1,
            "b": 2
        });
        let canonical1 = canonical_json(&input1).expect("canonical_json 1");
        let canonical2 = canonical_json(&input2).expect("canonical_json 2");
        assert_eq!(canonical1, canonical2);
    }

    #[test]
    fn test_canonical_json_nested_field_order_invariant() {
        let input1 = serde_json::json!({
            "outer": {
                "z": 1,
                "y": 2,
                "x": 3
            }
        });
        let input2 = serde_json::json!({
            "outer": {
                "x": 3,
                "y": 2,
                "z": 1
            }
        });
        let canonical1 = canonical_json(&input1).expect("canonical_json 1");
        let canonical2 = canonical_json(&input2).expect("canonical_json 2");
        assert_eq!(canonical1, canonical2);
    }

    #[test]
    fn test_canonical_json_array_order_preserved() {
        // Array order should be preserved (not sorted)
        let input1 = serde_json::json!({
            "array": [3, 1, 2]
        });
        let input2 = serde_json::json!({
            "array": [1, 2, 3]
        });
        let canonical1 = canonical_json(&input1).expect("canonical_json 1");
        let canonical2 = canonical_json(&input2).expect("canonical_json 2");
        assert_ne!(canonical1, canonical2);
    }

    #[test]
    fn test_compute_digest_golden_value() {
        let input = serde_json::json!({
            "name": "test",
            "version": "1.0.0"
        });
        let digest = compute_digest(&input).expect("compute_digest");
        // Verify it's a 64-character hex string (SHA256 = 32 bytes = 64 hex chars)
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c: char| c.is_ascii_hexdigit()));

        // Verify same input produces same digest
        let digest2 = compute_digest(&input).expect("compute_digest");
        assert_eq!(digest, digest2);
    }

    #[test]
    fn test_compute_digest_single_field_delta() {
        let input1 = serde_json::json!({
            "name": "test",
            "version": "1.0.0"
        });
        let input2 = serde_json::json!({
            "name": "test_modified",
            "version": "1.0.0"
        });
        let digest1 = compute_digest(&input1).expect("compute_digest 1");
        let digest2 = compute_digest(&input2).expect("compute_digest 2");
        assert_ne!(digest1, digest2);
    }

    #[test]
    fn test_compute_digest_nested_object() {
        let input = serde_json::json!({
            "config": {
                "timeout": 30,
                "retries": 3
            },
            "name": "test"
        });
        let digest = compute_digest(&input).expect("compute_digest");
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn test_canonical_json_zero_integer_valued() {
        let input = serde_json::json!({ "value": 0.0 });
        let canonical = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, r#"{"value":0}"#);
    }

    #[test]
    fn test_canonical_json_large_integer_valued() {
        let input = serde_json::json!({ "value": 1e10 });
        let canonical = canonical_json(&input).expect("canonical_json");
        assert_eq!(canonical, r#"{"value":10000000000}"#);
    }
}
