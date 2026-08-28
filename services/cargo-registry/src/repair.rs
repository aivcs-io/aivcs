//! Rebuild sparse-index records from the published `.crate` artifacts.
//!
//! Index records written before the publish handler was fixed are malformed in
//! two ways: entries written by this service carried the publish payload
//! verbatim (no `cksum`, `version_req` instead of `req`), and entries mirrored
//! from the retired Cloudflare Worker left `registry` null on crates.io
//! dependencies, which the sparse protocol reads as *this* registry.
//!
//! Repair cannot go through `publish`: published versions are immutable and a
//! re-publish is a 409 by design. So the record is rebuilt from the artifact,
//! which is the authoritative copy — a published `.crate` contains the
//! *normalized* `Cargo.toml` cargo generated at publish time, with `path` deps
//! already rewritten to registry deps and `registry-index` naming the registry.
//!
//! `cksum` is the digest of the stored bytes, so a regenerated record cannot
//! disagree with the artifact it describes.

use std::collections::BTreeMap;
use std::io::Read;

use serde::Deserialize;

use crate::index::{normalize_req, IndexDep, IndexEntry, CRATES_IO_INDEX};

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ManifestDep {
    /// `serde = "1"`.
    Simple(String),
    Detailed(Box<DetailedDep>),
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DetailedDep {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default = "default_true")]
    default_features: bool,
    /// Present only for deps from a non-default registry; this is how an
    /// `aivcs` dep is distinguished from a crates.io one.
    #[serde(default)]
    registry_index: Option<String>,
    /// Set when the dep is renamed: the table key is the alias.
    #[serde(default)]
    package: Option<String>,
}

impl ManifestDep {
    fn detail(&self) -> DetailedDep {
        match self {
            ManifestDep::Simple(v) => DetailedDep {
                version: Some(v.clone()),
                ..Default::default()
            },
            ManifestDep::Detailed(d) => DetailedDep {
                version: d.version.clone(),
                features: d.features.clone(),
                optional: d.optional,
                default_features: d.default_features,
                registry_index: d.registry_index.clone(),
                package: d.package.clone(),
            },
        }
    }
}

type DepTable = BTreeMap<String, ManifestDep>;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DepSections {
    #[serde(default)]
    dependencies: DepTable,
    #[serde(default)]
    dev_dependencies: DepTable,
    #[serde(default)]
    build_dependencies: DepTable,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PackageSection {
    name: String,
    version: String,
    #[serde(default)]
    links: Option<String>,
    #[serde(default)]
    rust_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Manifest {
    package: PackageSection,
    #[serde(flatten)]
    deps: DepSections,
    /// `[target."cfg(unix)".dependencies]` and friends.
    #[serde(default)]
    target: BTreeMap<String, DepSections>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

/// Pull the normalized `Cargo.toml` out of a `.crate` (gzipped tar).
///
/// Skips `Cargo.toml.orig`, which is the author's original file and may still
/// contain `path` or `git` dependencies that were rewritten for publication.
pub fn manifest_from_crate(crate_bytes: &[u8]) -> anyhow::Result<String> {
    let gz = flate2::read::GzDecoder::new(crate_bytes);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        // `<name>-<version>/Cargo.toml`, exactly two components.
        let is_manifest =
            path.file_name().is_some_and(|f| f == "Cargo.toml") && path.components().count() == 2;
        if is_manifest {
            let mut s = String::new();
            entry.read_to_string(&mut s)?;
            return Ok(s);
        }
    }
    anyhow::bail!("no Cargo.toml in .crate archive")
}

fn same_registry(a: &str, b: &str) -> bool {
    let strip = |s: &str| {
        s.trim_start_matches("sparse+")
            .trim_end_matches('/')
            .to_string()
    };
    strip(a) == strip(b)
}

fn to_index_deps(
    table: &DepTable,
    kind: &str,
    target: Option<&str>,
    own_registry: &str,
    out: &mut Vec<IndexDep>,
) {
    for (key, dep) in table {
        let d = dep.detail();
        // A renamed dep is keyed under the alias, with `package` naming the
        // real crate.
        let (name, package) = match &d.package {
            Some(real) => (key.clone(), Some(real.clone())),
            None => (key.clone(), None),
        };
        let registry = match d.registry_index.as_deref() {
            // No registry-index means the default registry: crates.io. Left
            // null it would mean *this* registry, which is the corruption
            // being repaired.
            None => Some(CRATES_IO_INDEX.to_string()),
            Some(u) if same_registry(u, own_registry) => None,
            Some(u) => Some(u.to_string()),
        };
        out.push(IndexDep {
            name,
            req: normalize_req(&d.version.clone().unwrap_or_else(|| "*".to_string())),
            features: d.features.clone(),
            optional: d.optional,
            default_features: d.default_features,
            target: target.map(str::to_string),
            kind: kind.to_string(),
            registry,
            package,
        });
    }
}

/// Rebuild the index record for one published version.
///
/// `yanked` is not recoverable from the artifact; callers preserve it from the
/// existing record rather than silently un-yanking a crate.
pub fn regenerate_entry(
    manifest_toml: &str,
    crate_bytes: &[u8],
    own_registry: &str,
    yanked: bool,
) -> anyhow::Result<IndexEntry> {
    let m: Manifest = toml::from_str(manifest_toml)?;

    let mut deps = Vec::new();
    to_index_deps(
        &m.deps.dependencies,
        "normal",
        None,
        own_registry,
        &mut deps,
    );
    to_index_deps(
        &m.deps.dev_dependencies,
        "dev",
        None,
        own_registry,
        &mut deps,
    );
    to_index_deps(
        &m.deps.build_dependencies,
        "build",
        None,
        own_registry,
        &mut deps,
    );
    for (target, sections) in &m.target {
        let t = Some(target.as_str());
        to_index_deps(&sections.dependencies, "normal", t, own_registry, &mut deps);
        to_index_deps(
            &sections.dev_dependencies,
            "dev",
            t,
            own_registry,
            &mut deps,
        );
        to_index_deps(
            &sections.build_dependencies,
            "build",
            t,
            own_registry,
            &mut deps,
        );
    }

    Ok(IndexEntry {
        name: m.package.name,
        vers: m.package.version,
        deps,
        cksum: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(crate_bytes)),
        features: m.features,
        yanked,
        links: m.package.links,
        v: None,
        rust_version: m.package.rust_version,
    })
}

/// True when a stored record already matches what the artifact says it should be.
///
/// Correctness cannot be judged from the record's *shape*. A null dependency
/// `registry` is the Worker-era corruption when the dep came from crates.io,
/// but it is the correct encoding when the dep is from this registry
/// (`envelope-data-mesh` → `data-mesh-client`). The only sound test is to
/// rebuild from the artifact and compare, which also makes repair idempotent:
/// a record that already agrees is never rewritten.
///
/// Compared as JSON values, not bytes, so key order does not force a rewrite.
fn record_matches(line: &[u8], rebuilt: &IndexEntry) -> bool {
    let (Ok(stored), Ok(want)) = (
        serde_json::from_slice::<serde_json::Value>(line),
        serde_json::to_value(rebuilt),
    ) else {
        return false;
    };
    stored == want
}

/// What a repair pass found, so a dry run can be read before anything is written.
#[derive(Debug, Default, PartialEq)]
pub struct RepairReport {
    pub records: usize,
    pub already_valid: usize,
    pub repaired: Vec<String>,
    /// Records that could not be rebuilt, with the reason. These are left
    /// exactly as they were.
    pub failed: Vec<(String, String)>,
}

/// Rebuild every malformed record in the index.
///
/// Dry by default: `apply == false` reports what would change and writes
/// nothing. Valid records are never rewritten, so a repair run is idempotent
/// and safe to repeat.
pub async fn run(
    store: &dyn crate::store::Store,
    own_registry: &str,
    apply: bool,
) -> anyhow::Result<RepairReport> {
    use crate::store::Bucket;

    let mut report = RepairReport::default();
    let keys = store
        .list(Bucket::Index, "")
        .await
        .map_err(|e| anyhow::anyhow!("listing index: {e}"))?;

    for key in keys {
        let body = match store.get(Bucket::Index, &key).await {
            Ok(b) => b,
            Err(e) => {
                report.failed.push((key.clone(), format!("read: {e}")));
                continue;
            }
        };

        let mut out: Vec<u8> = Vec::new();
        let mut changed = false;

        for line in body.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            report.records += 1;

            let parsed: serde_json::Value = match serde_json::from_slice(line) {
                Ok(v) => v,
                Err(e) => {
                    report
                        .failed
                        .push((key.clone(), format!("unparseable: {e}")));
                    out.extend_from_slice(line);
                    out.push(b'\n');
                    continue;
                }
            };
            let name = parsed["name"].as_str().unwrap_or_default().to_string();
            let vers = parsed["vers"].as_str().unwrap_or_default().to_string();
            let id = format!("{name}@{vers}");
            // A record that was never yanked has no `yanked` key; absent means
            // false, but an existing `true` must survive the rebuild.
            let yanked = parsed["yanked"].as_bool().unwrap_or(false);

            let crate_key = format!("{}/{}", name.to_ascii_lowercase(), vers);
            let bytes = match store.get(Bucket::Crates, &crate_key).await {
                Ok(b) => b,
                Err(e) => {
                    report.failed.push((id, format!("artifact: {e}")));
                    out.extend_from_slice(line);
                    out.push(b'\n');
                    continue;
                }
            };

            let rebuilt = manifest_from_crate(&bytes)
                .and_then(|m| regenerate_entry(&m, &bytes, own_registry, yanked));

            match rebuilt {
                Ok(entry) => {
                    if record_matches(line, &entry) {
                        // Already agrees with the artifact — leave the stored
                        // bytes untouched rather than rewriting them.
                        report.already_valid += 1;
                        out.extend_from_slice(line);
                        out.push(b'\n');
                        continue;
                    }
                    match serde_json::to_vec(&entry) {
                        Ok(new_line) => {
                            out.extend_from_slice(&new_line);
                            out.push(b'\n');
                            report.repaired.push(id);
                            changed = true;
                        }
                        Err(e) => {
                            report.failed.push((id, format!("encode: {e}")));
                            out.extend_from_slice(line);
                            out.push(b'\n');
                        }
                    }
                }
                Err(e) => {
                    report.failed.push((id, e.to_string()));
                    out.extend_from_slice(line);
                    out.push(b'\n');
                }
            }
        }

        if changed && apply {
            if let Err(e) = store.put(Bucket::Index, &key, out).await {
                report.failed.push((key, format!("write: {e}")));
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &str = "https://registry.aivcs.io";

    // Shaped after the real normalized manifest inside envelope-data-mesh
    // 0.1.0, which mixes crates.io deps with aivcs registry deps.
    const MANIFEST: &str = r#"
[package]
name = "envelope-data-mesh"
version = "0.1.0"
edition = "2021"
description = "ignored"
license = "Apache-2.0"

[dependencies.chrono]
version = "0.4"
features = ["clock", "serde"]

[dependencies.data-mesh-client]
version = "0.1"
registry-index = "sparse+https://registry.aivcs.io/"

[dependencies.serde]
version = "1"
features = ["derive"]

[dev-dependencies.tokio]
version = "1"
features = ["macros"]
"#;

    fn entry() -> IndexEntry {
        regenerate_entry(MANIFEST, b"artifact-bytes", OWN, false).unwrap()
    }

    fn dep<'a>(e: &'a IndexEntry, name: &str) -> &'a IndexDep {
        e.deps.iter().find(|d| d.name == name).expect("dep present")
    }

    #[test]
    fn crates_io_deps_get_an_explicit_registry() {
        // The corruption in the Worker-mirrored entries: null here means
        // "this registry", so cargo searched aivcs for serde.
        let e = entry();
        assert_eq!(dep(&e, "serde").registry.as_deref(), Some(CRATES_IO_INDEX));
        assert_eq!(dep(&e, "chrono").registry.as_deref(), Some(CRATES_IO_INDEX));
    }

    #[test]
    fn own_registry_deps_are_null() {
        // Normalised past the `sparse+` prefix and trailing slash.
        assert_eq!(dep(&entry(), "data-mesh-client").registry, None);
    }

    #[test]
    fn requirement_and_features_survive() {
        let e = entry();
        // Caret form, as cargo writes it — the manifest says `version = "0.4"`.
        assert_eq!(dep(&e, "chrono").req, "^0.4");
        assert_eq!(dep(&e, "chrono").features, vec!["clock", "serde"]);
        assert!(dep(&e, "chrono").default_features);
    }

    #[test]
    fn dev_dependencies_keep_their_kind() {
        let e = entry();
        assert_eq!(dep(&e, "tokio").kind, "dev");
        assert_eq!(dep(&e, "serde").kind, "normal");
    }

    #[test]
    fn cksum_is_the_digest_of_the_supplied_artifact() {
        let e = entry();
        assert_eq!(e.cksum.len(), 64);
        // Same artifact, same digest — regeneration is deterministic, so a
        // repair run is safe to re-run.
        assert_eq!(e.cksum, entry().cksum);
        let other = regenerate_entry(MANIFEST, b"different", OWN, false).unwrap();
        assert_ne!(e.cksum, other.cksum);
    }

    #[test]
    fn yanked_is_carried_in_not_inferred() {
        // Nothing in the artifact records a yank; regenerating must not
        // resurrect a withdrawn version.
        assert!(regenerate_entry(MANIFEST, b"x", OWN, true).unwrap().yanked);
    }

    #[test]
    fn publish_only_fields_do_not_leak() {
        let json = serde_json::to_string(&entry()).unwrap();
        for leaked in ["description", "license", "edition"] {
            assert!(!json.contains(leaked), "{leaked} leaked into index entry");
        }
        assert!(
            !json.contains("\"v\":"),
            "no `v`: the valid records already stored omit it"
        );
    }

    #[test]
    fn target_scoped_deps_carry_their_target() {
        let m =
            format!("{MANIFEST}\n[target.\"cfg(unix)\".dependencies.nix]\nversion = \"0.27\"\n");
        let e = regenerate_entry(&m, b"x", OWN, false).unwrap();
        assert_eq!(dep(&e, "nix").target.as_deref(), Some("cfg(unix)"));
        assert_eq!(dep(&e, "nix").kind, "normal");
    }

    #[test]
    fn renamed_dep_records_the_real_crate() {
        let m = format!(
            "{MANIFEST}\n[dependencies.alias]\nversion = \"1\"\npackage = \"real-crate\"\n"
        );
        let e = regenerate_entry(&m, b"x", OWN, false).unwrap();
        assert_eq!(dep(&e, "alias").package.as_deref(), Some("real-crate"));
    }

    #[test]
    fn a_null_registry_is_only_corruption_when_the_dep_is_not_ours() {
        // Why repair compares against the artifact instead of inspecting the
        // record's shape: `registry: null` is BOTH the Worker-era corruption on
        // a crates.io dep AND the correct encoding for a same-registry dep.
        // Judging by the field alone would rewrite valid records forever.
        let e = entry();
        assert_eq!(dep(&e, "data-mesh-client").registry, None, "ours: null");
        assert_eq!(
            dep(&e, "serde").registry.as_deref(),
            Some(CRATES_IO_INDEX),
            "crates.io: spelled out"
        );

        let rebuilt = serde_json::to_vec(&e).unwrap();
        assert!(
            record_matches(&rebuilt, &e),
            "a rebuilt record matches itself"
        );

        // The service-era corruption does not match, so it is repaired.
        let service_bug = br#"{"name":"envelope-data-mesh","vers":"0.1.0","deps":[{"name":"serde","version_req":"^1"}]}"#;
        assert!(!record_matches(service_bug, &e));
        assert!(!record_matches(b"not json", &e));
    }

    /// The real normalized manifest from the published `data-mesh-client
    /// 0.1.0` artifact, verbatim.
    const DATA_MESH_CLIENT: &str = r#"
[package]
name = "data-mesh-client"
version = "0.1.0"
edition = "2021"
description = "Rust client for the data-mesh standard storage offering"
license = "Apache-2.0"

[features]
blocking = ["dep:ureq"]

[dependencies.percent-encoding]
version = "2"

[dependencies.reqwest]
version = "0.12"
features = ["json"]

[dependencies.serde]
version = "1.0"
features = ["derive"]

[dependencies.serde_json]
version = "1.0"

[dependencies.thiserror]
version = "1.0"

[dependencies.tokio]
version = "1.0"
features = ["full"]

[dependencies.ureq]
version = "2"
features = ["json"]
optional = true
"#;

    /// Regression against production data.
    ///
    /// `data-mesh-client 0.1.0` is the one record in the live index that cargo
    /// resolves correctly, so a rebuild of it must reproduce that record — not
    /// merely something valid. This is what stops a repair pass rewriting the
    /// whole index: the first implementation emitted bare requirements (`0.4`)
    /// and a `v` field, so every healthy record compared unequal and would have
    /// been "repaired".
    #[test]
    fn rebuilding_a_healthy_record_reproduces_it_exactly() {
        let e = regenerate_entry(DATA_MESH_CLIENT, b"x", OWN, false).unwrap();

        // Requirements as the live record spells them: caret form.
        for (name, req) in [
            ("percent-encoding", "^2"),
            ("reqwest", "^0.12"),
            ("serde", "^1.0"),
            ("serde_json", "^1.0"),
            ("thiserror", "^1.0"),
            ("tokio", "^1.0"),
            ("ureq", "^2"),
        ] {
            assert_eq!(dep(&e, name).req, req, "{name} requirement");
        }
        assert!(dep(&e, "ureq").optional, "optional survives");

        // The live record's exact top-level shape: no `v`, and `dep:` syntax
        // stays in `features` rather than being split into `features2`.
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            ["cksum", "deps", "features", "name", "vers", "yanked"]
        );
        assert_eq!(v["features"]["blocking"][0], "dep:ureq");
    }

    #[test]
    fn requirements_are_canonicalised_the_way_cargo_writes_them() {
        assert_eq!(normalize_req("0.4"), "^0.4");
        assert_eq!(normalize_req("1"), "^1");
        // Already explicit — must not be double-prefixed.
        assert_eq!(normalize_req("^1.0"), "^1.0");
        assert_eq!(normalize_req("~1.2"), "~1.2");
        assert_eq!(normalize_req("=1.2.3"), "=1.2.3");
        assert_eq!(normalize_req(">=1.0"), ">=1.0");
        assert_eq!(normalize_req("*"), "*");
        // Comma sets: each part judged on its own.
        assert_eq!(normalize_req(">=1.0, <2.0"), ">=1.0, <2.0");
    }

    /// A real `.crate`: gzipped tar containing the normalized manifest.
    fn fake_crate(manifest: &str) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut tar_buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_buf);
            let mut h = tar::Header::new_gnu();
            h.set_size(manifest.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "pkg-0.1.0/Cargo.toml", manifest.as_bytes())
                .unwrap();
            b.finish().unwrap();
        }
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_buf).unwrap();
        gz.finish().unwrap()
    }

    async fn seeded() -> (std::sync::Arc<crate::store::MemoryStore>, Vec<u8>) {
        use crate::store::{Bucket, MemoryStore, Store};
        let store = std::sync::Arc::new(MemoryStore::new());
        let krate = fake_crate(MANIFEST);
        store
            .put(Bucket::Crates, "envelope-data-mesh/0.1.0", krate.clone())
            .await
            .unwrap();
        // The exact corruption this service used to write.
        let broken = br#"{"name":"envelope-data-mesh","vers":"0.1.0","deps":[{"name":"serde","version_req":"^1"}],"description":"leaked"}"#;
        store
            .put(
                Bucket::Index,
                "en/ve/envelope-data-mesh",
                [broken.as_slice(), b"\n"].concat(),
            )
            .await
            .unwrap();
        (store, krate)
    }

    #[tokio::test]
    async fn dry_run_reports_without_writing() {
        use crate::store::{Bucket, Store};
        let (store, _) = seeded().await;
        let before = store
            .get(Bucket::Index, "en/ve/envelope-data-mesh")
            .await
            .unwrap();

        let r = run(store.as_ref(), OWN, false).await.unwrap();
        assert_eq!(r.repaired, vec!["envelope-data-mesh@0.1.0"]);
        assert!(r.failed.is_empty());

        let after = store
            .get(Bucket::Index, "en/ve/envelope-data-mesh")
            .await
            .unwrap();
        assert_eq!(before, after, "a dry run must not write");
    }

    #[tokio::test]
    async fn apply_rewrites_the_record_and_is_idempotent() {
        use crate::store::{Bucket, Store};
        let (store, krate) = seeded().await;

        let r = run(store.as_ref(), OWN, true).await.unwrap();
        assert_eq!(r.repaired.len(), 1);

        let body = store
            .get(Bucket::Index, "en/ve/envelope-data-mesh")
            .await
            .unwrap();
        assert_eq!(body.last(), Some(&b'\n'), "ndjson invariant holds");
        let v: serde_json::Value =
            serde_json::from_slice(body.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(
            v["cksum"].as_str().unwrap(),
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&krate)),
            "cksum matches the stored artifact"
        );
        assert!(v.get("description").is_none(), "leaked field removed");
        assert_eq!(v["deps"][0]["req"], "^0.4");

        // Re-running must be a no-op: nothing left to repair.
        let again = run(store.as_ref(), OWN, true).await.unwrap();
        assert!(again.repaired.is_empty(), "repair is idempotent");
        assert_eq!(again.already_valid, again.records);
    }

    #[tokio::test]
    async fn a_missing_artifact_is_reported_and_the_record_is_left_alone() {
        use crate::store::{Bucket, MemoryStore, Store};
        let store = std::sync::Arc::new(MemoryStore::new());
        let broken =
            br#"{"name":"ghost","vers":"9.9.9","deps":[{"name":"serde","version_req":"^1"}]}"#;
        store
            .put(
                Bucket::Index,
                "gh/os/ghost",
                [broken.as_slice(), b"\n"].concat(),
            )
            .await
            .unwrap();

        let r = run(store.as_ref(), OWN, true).await.unwrap();
        assert!(r.repaired.is_empty());
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].0.starts_with("ghost@9.9.9"));
        // Unrepairable must mean untouched, not dropped.
        let after = store.get(Bucket::Index, "gh/os/ghost").await.unwrap();
        assert_eq!(after, [broken.as_slice(), b"\n"].concat());
    }

    #[test]
    fn manifest_is_read_from_the_archive_not_the_orig() {
        // Cargo.toml.orig may still carry the pre-publication `path`/`git`
        // deps, so reading it would reintroduce exactly what we are removing.
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut tar_buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_buf);
            let mut add = |name: &str, body: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, body).unwrap();
            };
            add("pkg-0.1.0/Cargo.toml.orig", b"ORIG");
            add("pkg-0.1.0/Cargo.toml", MANIFEST.as_bytes());
            b.finish().unwrap();
        }
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_buf).unwrap();
        let krate = gz.finish().unwrap();

        let got = manifest_from_crate(&krate).unwrap();
        assert!(got.contains("envelope-data-mesh"));
        assert!(!got.contains("ORIG"));
    }
}
