//! Sparse-registry index path layout and index-entry construction.
//!
//! Cargo derives an object key from the crate name alone. The prefix rules are
//! fixed by the sparse protocol, not by us — see the Cargo book, "Registry
//! Index". Names are lowercased for lookup because the index is
//! case-insensitive while object keys are not.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Deps with no `registry` in publish metadata come from crates.io: cargo omits
/// the field for the default registry. In *our* index that must be spelled out,
/// or cargo resolves them against `aivcs` and fails with
/// "no matching package named `serde` found".
pub const CRATES_IO_INDEX: &str = "https://github.com/rust-lang/crates.io-index";

fn default_true() -> bool {
    true
}

/// A dependency as it appears in the body of `PUT /api/v1/crates/new`.
#[derive(Debug, Deserialize)]
pub struct PublishDep {
    pub name: String,
    /// Publish metadata calls this `version_req`; the index calls it `req`.
    pub version_req: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default = "default_true")]
    pub default_features: bool,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub registry: Option<String>,
    /// Set when the dep is renamed in Cargo.toml (`foo = { package = "bar" }`).
    #[serde(default)]
    pub explicit_name_in_toml: Option<String>,
}

/// The publish payload. Everything not listed here (description, license,
/// readme, badges …) is registry bookkeeping and must NOT reach the index.
#[derive(Debug, Deserialize)]
pub struct PublishMeta {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<PublishDep>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub links: Option<String>,
    #[serde(default)]
    pub rust_version: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct IndexDep {
    pub name: String,
    pub req: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub default_features: bool,
    pub target: Option<String>,
    pub kind: String,
    pub registry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    pub deps: Vec<IndexDep>,
    pub cksum: String,
    pub features: BTreeMap<String, Vec<String>>,
    pub yanked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<String>,
    /// Index schema version. Omitted, which means 1.
    ///
    /// `v: 2` exists to let old cargo (<1.60) recognise `features2`, and this
    /// registry does not split those out — the records already in the index
    /// carry `dep:` syntax directly in `features` with no `v`, and cargo
    /// resolves them. Emitting `v` would make every regenerated record differ
    /// from the valid ones already stored, turning a targeted repair into a
    /// full rewrite of the index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<String>,
}

/// Canonicalise a version requirement the way cargo records it in the index.
///
/// A bare requirement in a manifest (`version = "0.4"`) is *semantically*
/// caret, and cargo writes it to the index in that explicit form (`^0.4`).
/// Emitting the bare string would resolve identically but differ textually
/// from every record cargo itself produced — which, for repair, means a
/// byte-comparison flags healthy entries as broken.
///
/// Anything already carrying an operator (`^ ~ = > <`), a wildcard, or a
/// comma-separated set is left untouched.
pub fn normalize_req(req: &str) -> String {
    req.split(',')
        .map(|part| {
            let t = part.trim();
            match t.chars().next() {
                Some(c) if c.is_ascii_digit() => format!("^{t}"),
                _ => t.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Normalise a registry URL for comparison: `sparse+` prefix and a trailing
/// slash are cosmetic, but a mismatch would leave a self-referential dep
/// pointing at an absolute URL instead of `null`.
fn same_registry(dep: &str, own: &str) -> bool {
    let strip = |s: &str| {
        s.trim_start_matches("sparse+")
            .trim_end_matches('/')
            .to_string()
    };
    strip(dep) == strip(own)
}

/// Build the index record for a published crate.
///
/// The publish payload and the index record are **different schemas**. Writing
/// the former into the index yields entries with `version_req` instead of `req`
/// and no `cksum` at all, which cargo cannot resolve.
///
/// `own_registry` is this registry's own index URL; deps pointing at it become
/// `null`, which is how the sparse protocol spells "same registry".
pub fn index_entry(
    meta: &PublishMeta,
    crate_bytes: &[u8],
    own_registry: Option<&str>,
) -> IndexEntry {
    let cksum = hex::encode(Sha256::digest(crate_bytes));

    let deps = meta
        .deps
        .iter()
        .map(|d| {
            // A renamed dep is keyed in the index under the name used in
            // Cargo.toml, with `package` carrying the real crate name.
            let (name, package) = match &d.explicit_name_in_toml {
                Some(alias) => (alias.clone(), Some(d.name.clone())),
                None => (d.name.clone(), None),
            };
            let registry = match d.registry.as_deref() {
                None => Some(CRATES_IO_INDEX.to_string()),
                Some(u) => match own_registry {
                    Some(own) if same_registry(u, own) => None,
                    _ => Some(u.to_string()),
                },
            };
            IndexDep {
                name,
                req: normalize_req(&d.version_req),
                features: d.features.clone(),
                optional: d.optional,
                default_features: d.default_features,
                target: d.target.clone(),
                kind: d.kind.clone().unwrap_or_else(|| "normal".to_string()),
                registry,
                package,
            }
        })
        .collect();

    IndexEntry {
        name: meta.name.clone(),
        vers: meta.vers.clone(),
        deps,
        cksum,
        features: meta.features.clone(),
        yanked: false,
        links: meta.links.clone(),
        v: None,
        rust_version: meta.rust_version.clone(),
    }
}

/// Object key for a crate's index entry, e.g. `da/ta/data-mesh-client`.
pub fn index_key(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.len() {
        0 => n,
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{}", &n[0..1], n),
        _ => format!("{}/{}/{}", &n[0..2], &n[2..4], n),
    }
}

/// True when `path` is a sparse index request rather than an API or config route.
///
/// Everything that is not `/config.json`, `/api/…`, or a probe is an index
/// lookup; the worker this replaces used the same negative test.
pub fn is_index_path(path: &str) -> bool {
    if !path.starts_with('/') || path == "/" {
        return false;
    }
    !(path.starts_with("/api/")
        || path == "/config.json"
        || path == "/healthz"
        || path == "/readyz")
}

/// The sparse protocol is ndjson: every record line must be newline-terminated,
/// including the last. Objects written before this was enforced may lack the
/// trailing newline, and cargo rejects the whole index if it is missing — so
/// normalise on read rather than trusting stored bytes.
pub fn ensure_trailing_newline(mut body: Vec<u8>) -> Vec<u8> {
    if !body.is_empty() && body.last() != Some(&b'\n') {
        body.push(b'\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_key_uses_the_sparse_prefix_layout() {
        assert_eq!(index_key("a"), "1/a");
        assert_eq!(index_key("ab"), "2/ab");
        assert_eq!(index_key("abc"), "3/a/abc");
        assert_eq!(index_key("serde"), "se/rd/serde");
        assert_eq!(index_key("data-mesh-client"), "da/ta/data-mesh-client");
        assert_eq!(index_key("envelope-brains"), "en/ve/envelope-brains");
    }

    #[test]
    fn index_key_is_case_insensitive() {
        assert_eq!(index_key("Data-Mesh-Client"), index_key("data-mesh-client"));
    }

    #[test]
    fn is_index_path_excludes_api_config_and_probes() {
        assert!(is_index_path("/da/ta/data-mesh-client"));
        assert!(is_index_path("/1/a"));
        assert!(!is_index_path("/config.json"));
        assert!(!is_index_path("/api/v1/crates/x/1.0.0/download"));
        assert!(!is_index_path("/healthz"));
        assert!(!is_index_path("/readyz"));
        assert!(!is_index_path("/"));
        assert!(!is_index_path("no-leading-slash"));
    }

    fn meta(json: &str) -> PublishMeta {
        serde_json::from_str(json).expect("publish metadata parses")
    }

    const OWN: &str = "sparse+https://registry.aivcs.io/";

    #[test]
    fn cksum_is_the_sha256_of_the_crate_bytes() {
        let m = meta(r#"{"name":"x","vers":"0.1.0"}"#);
        let e = index_entry(&m, b"", Some(OWN));
        // SHA-256 of the empty input, so the value is checkable by hand.
        assert_eq!(
            e.cksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(index_entry(&m, b"abc", Some(OWN)).cksum.len(), 64);
    }

    #[test]
    fn version_req_becomes_req() {
        // The defect this guards: publish metadata was written to the index
        // verbatim, leaving `version_req`, which cargo does not read.
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","deps":[
            {"name":"serde","version_req":"^1","features":["derive"],
             "optional":false,"default_features":true,"target":null,
             "kind":"normal","registry":null}]}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        assert_eq!(e.deps[0].req, "^1");
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"req\":\"^1\""));
        assert!(!json.contains("version_req"));
    }

    #[test]
    fn null_dep_registry_resolves_to_crates_io() {
        // A null registry in publish metadata means crates.io. Left as null in
        // our index it means "this registry", and resolution fails with
        // "no matching package named `serde` found".
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","deps":[
            {"name":"serde","version_req":"^1","registry":null}]}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        assert_eq!(e.deps[0].registry.as_deref(), Some(CRATES_IO_INDEX));
    }

    #[test]
    fn self_referential_dep_registry_becomes_null() {
        // envelope-data-mesh -> data-mesh-client, both on aivcs.
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","deps":[
            {"name":"data-mesh-client","version_req":"^0.1",
             "registry":"https://registry.aivcs.io"}]}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        assert_eq!(e.deps[0].registry, None, "same registry is spelled null");
    }

    #[test]
    fn renamed_dep_keeps_the_real_crate_in_package() {
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","deps":[
            {"name":"real-crate","version_req":"^1",
             "explicit_name_in_toml":"alias"}]}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        assert_eq!(e.deps[0].name, "alias");
        assert_eq!(e.deps[0].package.as_deref(), Some("real-crate"));
    }

    #[test]
    fn entry_carries_the_fields_cargo_requires() {
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","description":"ignored",
            "license":"Apache-2.0","readme":"README.md","badges":{}}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        // `v` is omitted: the valid records already in the index carry no `v`,
        // so emitting one would make every rebuild differ from them.
        assert_eq!(e.v, None);
        assert!(!serde_json::to_string(&e).unwrap().contains("\"v\":"));
        assert!(!e.yanked);
        let json = serde_json::to_string(&e).unwrap();
        // Publish-only bookkeeping must not leak into the index record.
        for leaked in ["description", "license", "readme", "badges"] {
            assert!(!json.contains(leaked), "{leaked} leaked into index entry");
        }
        // Absent optionals are omitted rather than written as null.
        assert!(!json.contains("links"));
        assert!(!json.contains("rust_version"));
    }

    #[test]
    fn dep_defaults_apply_when_fields_are_absent() {
        let m = meta(
            r#"{"name":"x","vers":"0.1.0","deps":[
            {"name":"serde","version_req":"^1"}]}"#,
        );
        let e = index_entry(&m, b"", Some(OWN));
        assert_eq!(e.deps[0].kind, "normal");
        assert!(
            e.deps[0].default_features,
            "default_features defaults to true"
        );
        assert!(!e.deps[0].optional);
    }

    #[test]
    fn trailing_newline_is_added_only_when_missing() {
        assert_eq!(ensure_trailing_newline(b"{}".to_vec()), b"{}\n".to_vec());
        assert_eq!(ensure_trailing_newline(b"{}\n".to_vec()), b"{}\n".to_vec());
        // Empty stays empty: a 404 is the caller's job, not a bare newline.
        assert_eq!(ensure_trailing_newline(Vec::new()), Vec::<u8>::new());
    }
}
