//! `.hexon` packages on fe-format's archive: build seals, open verifies (see `src/AGENTS.md` §unified-format).

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use anyhow::Context;
use ed25519_dalek::SigningKey;
use fe_format::{HexonArchive, IntegrityStatus};
use serde::{Deserialize, Serialize};

use crate::manifest::{AssetEntry, HexonManifest, License};

/// Baked splat coverage buffer for one terrain tile (track
/// `splat_hexon_bake_20260712`, FR-2). Optional/versioned: an absent field on
/// a `TerrainTileset` hexon means the runtime falls back to live
/// `synthesize_splats` at native density (no crash, no live fill — see
/// `fe-terrain/src/splat/format.rs`, which owns the actual
/// `terrain/splats/{cache_key}.bin` on-disk shape embedded in the tileset ZIP
/// via `fe_terrain::splat::format::{append,read}_baked_splats_to_archive`).
/// Mirrored here (not re-exported, `fe-terrain` is not a dependency of this
/// crate) so hexon-format consumers outside `fe-terrain` have a canonical
/// serde description of the per-tile record without a crate-graph edge back
/// into `fe-terrain`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BakedSplatTileRecord {
    /// Format version; bump on any incompatible field/encoding change.
    pub version: u16,
    /// Tile cache key, `{z}/{x}/{y}`.
    pub cache_key: String,
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub scales: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
}

/// Current on-disk format version for [`BakedSplatTileRecord`]; kept in lockstep
/// with `fe_terrain::splat::format::BAKED_SPLAT_FORMAT_VERSION`.
pub const BAKED_SPLAT_TILE_RECORD_VERSION: u16 = 1;

/// Optional package icon member.
pub const ICON_PATH: &str = "icon.png";
/// Prefix of the optional preview image members (`preview/{name}`).
pub const PREVIEW_PREFIX: &str = "preview/";
/// Optional package readme member.
pub const README_PATH: &str = "README.md";
/// Optional fe-format `SchemaDefinition` member.
pub const SCHEMA_PATH: &str = "schema.json";

/// A parsed `.hexon` package: fe-format sections plus the optional presentation members.
#[derive(Debug, Clone)]
pub struct HexonPackageData {
    pub manifest: HexonManifest,
    pub entries: Vec<AssetEntry>,
    /// The package's `license.json`, when it carries one.
    pub license: Option<License>,
    pub icon: Option<Vec<u8>>,
    pub previews: HashMap<String, Vec<u8>>,
    /// Asset bytes keyed by `asset_hash`.
    pub assets: HashMap<String, Vec<u8>>,
    pub readme: Option<String>,
    pub schema: Option<String>,
    /// What fe-format's seal check established on open.
    pub integrity: IntegrityStatus,
    /// True when the archive is sealed by its manifest's publisher and the manifest signature verifies.
    pub signature_verified: bool,
}

impl HexonPackageData {
    /// Returns true if the seal and manifest signature verified on open.
    pub fn signature_valid(&self) -> bool {
        self.signature_verified
    }

    /// `{hexon_id}@{version}` of this package.
    pub fn hexon_uri(&self) -> String {
        hexon_uri(&self.manifest)
    }
}

/// Builds and opens `.hexon` packages through `fe_format::HexonArchive`.
pub struct HexonPackage;

impl HexonPackage {
    /// Export, append the presentation members, then seal and sign with `signing_key`.
    ///
    /// The seal sets `publisher_did` to the key's DID; every asset must be referenced by `entries`.
    #[allow(clippy::too_many_arguments)] // package builder — params mirror archive sections
    pub fn build(
        manifest: HexonManifest,
        entries: Vec<AssetEntry>,
        license: Option<License>,
        assets: HashMap<String, Vec<u8>>,
        icon: Option<Vec<u8>>,
        previews: HashMap<String, Vec<u8>>,
        readme: Option<String>,
        schema: Option<String>,
        signing_key: &SigningKey,
    ) -> Result<Vec<u8>, anyhow::Error> {
        let mut manifest = manifest;
        manifest.integrity = None;
        manifest.signature = None;

        let mut assets: Vec<(String, Vec<u8>)> = assets.into_iter().collect();
        assets.sort_by(|left, right| left.0.cmp(&right.0));
        let exported = HexonArchive::export(manifest, entries, assets, license, None, None, None)
            .context("exporting hexon package")?;

        let mut extra_members: Vec<(String, Vec<u8>)> = Vec::new();
        if let Some(icon_bytes) = icon {
            extra_members.push((ICON_PATH.to_string(), icon_bytes));
        }
        let mut previews: Vec<(String, Vec<u8>)> = previews.into_iter().collect();
        previews.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, bytes) in previews {
            anyhow::ensure!(is_plain_member_name(&name), "invalid preview name {name:?}");
            extra_members.push((format!("{PREVIEW_PREFIX}{name}"), bytes));
        }
        if let Some(readme_text) = readme {
            extra_members.push((README_PATH.to_string(), readme_text.into_bytes()));
        }
        if let Some(schema_text) = schema {
            serde_json::from_str::<fe_format::SchemaDefinition>(&schema_text)
                .context("schema.json must be an fe-format SchemaDefinition")?;
            extra_members.push((SCHEMA_PATH.to_string(), schema_text.into_bytes()));
        }

        let unsealed = if extra_members.is_empty() {
            exported
        } else {
            append_members(exported, &extra_members)?
        };
        fe_format::seal(&unsealed, signing_key).context("sealing hexon package")
    }

    /// Import under fe-format's caps and seal check, then read the presentation members.
    ///
    /// A tampered sealed archive is an error; an unsealed one opens with `signature_valid() == false`.
    pub fn open(bytes: &[u8]) -> Result<HexonPackageData, anyhow::Error> {
        use fe_format::integrity::MANIFEST_PATH;
        use fe_format::limits::{
            check_entry_count, MAX_ARCHIVE_UNCOMPRESSED_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES,
            MAX_JSON_ENTRY_BYTES,
        };

        let data = HexonArchive::import_with_budget(bytes, MAX_ARCHIVE_UNCOMPRESSED_BYTES)?;

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| anyhow::anyhow!("invalid ZIP archive: {}", e))?;
        check_entry_count(archive.len())?;
        // One decompression budget for every member read here (zip-bomb guard,
        // fe-format AGENTS.md §decompression-limits).
        let mut budget = MAX_ARCHIVE_UNCOMPRESSED_BYTES;
        let (json, binary) = (MAX_JSON_ENTRY_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES);

        // The signature is checked over the stored manifest bytes, never a re-serialization.
        let manifest_raw = read_named(&mut archive, MANIFEST_PATH, json, &mut budget)?
            .context("missing manifest.json")?;
        let signature_verified = sealed_and_signed(&data.manifest, &data.integrity, &manifest_raw);

        let icon = read_named(&mut archive, ICON_PATH, binary, &mut budget)?;
        let preview_names: Vec<String> = archive
            .file_names()
            .filter(|name| {
                name.strip_prefix(PREVIEW_PREFIX)
                    .is_some_and(is_plain_member_name)
            })
            .map(str::to_string)
            .collect();
        let mut previews = HashMap::new();
        for name in preview_names {
            if let Some(bytes) = read_named(&mut archive, &name, binary, &mut budget)? {
                previews.insert(name[PREVIEW_PREFIX.len()..].to_string(), bytes);
            }
        }
        // Non-UTF-8 text members read as absent.
        let readme = read_named(&mut archive, README_PATH, json, &mut budget)?
            .and_then(|b| String::from_utf8(b).ok());
        let schema = read_named(&mut archive, SCHEMA_PATH, json, &mut budget)?
            .and_then(|b| String::from_utf8(b).ok());

        Ok(HexonPackageData {
            manifest: data.manifest,
            entries: data.entries,
            license: data.license,
            icon,
            previews,
            assets: data.assets.into_iter().collect(),
            readme,
            schema,
            integrity: data.integrity,
            signature_verified,
        })
    }
}

/// Sealed by the manifest's own publisher, and the canonical manifest signature verifies (fail closed).
fn sealed_and_signed(manifest: &HexonManifest, integrity: &IntegrityStatus, manifest_raw: &[u8]) -> bool {
    let sealed_by_publisher = matches!(
        integrity,
        IntegrityStatus::Sealed { publisher_did } if *publisher_did == manifest.publisher_did
    );
    sealed_by_publisher
        && manifest.signature.as_deref().is_some_and(|signature| {
            crate::signature::verify_manifest(manifest_raw, signature, &manifest.publisher_did)
                .unwrap_or(false)
        })
}

/// One path segment: non-empty, no separators, not `.`/`..`.
fn is_plain_member_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// Append deflated members to an exported archive (zip 2 append mode).
fn append_members(archive: Vec<u8>, members: &[(String, Vec<u8>)]) -> Result<Vec<u8>, anyhow::Error> {
    let mut writer = zip::ZipWriter::new_append(Cursor::new(archive))
        .context("reopening exported archive for append")?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(name.as_str(), options)?;
        writer.write_all(bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Read `name` under `cap` and the shared `budget`; `Ok(None)` when the entry is absent.
fn read_named<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
    budget: &mut u64,
) -> Result<Option<Vec<u8>>, anyhow::Error> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let declared = file.size();
    fe_format::read_entry_capped(&mut file, declared, cap, budget)
        .map(Some)
        .map_err(|e| e.context(format!("reading {name}")))
}

/// `{hexon_id}@{version}` of a manifest.
pub fn hexon_uri(manifest: &HexonManifest) -> String {
    format!("{}@{}", manifest.hexon_id, manifest.version)
}

// ---- sealed plugin packages (fe-format/src/AGENTS.md §plugin-package; docs/extensions/package-format.md) ----

/// Host ceiling for `plugin/extension.json`.
pub const PLUGIN_EXTENSION_MAX_BYTES: u64 = 64 * 1024;
/// Host ceiling for `plugin/component.wasm`.
pub const PLUGIN_COMPONENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Ceiling for `plugin/countersignatures.json`.
pub const PLUGIN_COUNTERSIGNATURES_MAX_BYTES: u64 = 64 * 1024;
/// Most counter-signature entries a package may carry.
pub const PLUGIN_COUNTERSIGNATURES_MAX_ENTRIES: usize = 16;
/// Contract prefix a plugin component must target.
const PLUGIN_CONTRACT_PREFIX: &str = "fractalengine:plugin@2.";
/// Minor of the WIT the host binds (`fe-plugin/wit/fractal-plugin-2.wit` is `2.0.0`); a newer minor is refused (G0-C15).
const HOST_CONTRACT_MINOR: u64 = 0;

/// `fractalengine:plugin@2.<minor>.<patch>` with `minor` no newer than the host binds.
fn contract_supported(contract: &str) -> bool {
    let Some(rest) = contract.strip_prefix(PLUGIN_CONTRACT_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(minor), Some(patch), None) => {
            // Ceiling that rises with the host contract; keep `<=` though it is `== 0` today.
            #[allow(clippy::absurd_extreme_comparisons)]
            let supported = minor.parse::<u64>().is_ok_and(|minor| minor <= HOST_CONTRACT_MINOR);
            patch.parse::<u64>().is_ok() && supported
        }
        _ => false,
    }
}
/// Wasm binary preamble of a *component* (core modules carry version `01 00 00 00`).
const WASM_COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

/// Typed refusal of a plugin package; the reason strings are the contract's (J5 table).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginPackageError {
    #[error("package-invalid ({0})")]
    Invalid(String),
    #[error("untrusted-signer ({0})")]
    UntrustedSigner(String),
    #[error("unsupported-contract")]
    UnsupportedContract,
}

fn invalid(reason: impl Into<String>) -> PluginPackageError {
    PluginPackageError::Invalid(reason.into())
}

/// Who may sign a plugin package: trust roots by `did:key`, plus the dev escape hatch.
pub struct PluginTrust<'a> {
    /// True when the `did:key` is a configured trust root (e.g. `trusted_publishers.toml`).
    pub is_trust_root: &'a dyn Fn(&str) -> bool,
    /// Accept a wholly unsigned package; callers must keep this unreachable from release builds.
    pub allow_unsigned_dev: bool,
}

/// A plugin package that passed [`verify_plugin_package`]; the sealed bytes stay with the caller.
#[derive(Debug, Clone)]
pub struct PluginPackage {
    pub package_id: String,
    pub version: String,
    pub publisher_did: String,
    /// blake3 of the canonical signed manifest bytes: one value per sealed build.
    pub content_hash: String,
    /// blake3 of `plugin/component.wasm` as stored.
    pub component_digest: String,
    pub extension_json: Vec<u8>,
    pub license_json: Option<Vec<u8>>,
    pub component: Vec<u8>,
    /// The trust root that vouched for the package (publisher or counter-signer); `None` for dev-unsigned.
    pub trusted_by: Option<String>,
}

impl PluginPackage {
    /// The two values a revocation entry may name; either one revokes the package under any id.
    pub fn revocation_keys(&self) -> [&str; 2] {
        [&self.content_hash, &self.component_digest]
    }
}

#[derive(Deserialize)]
struct CountersignatureDoc {
    schema_version: u32,
    content_hash: String,
    entries: Vec<CountersignatureEntry>,
}

#[derive(Deserialize)]
struct CountersignatureEntry {
    key_id: String,
    signature: String,
}

/// Map a seal failure to the contract's refusal vocabulary.
fn integrity_refusal(err: fe_format::IntegrityError) -> PluginPackageError {
    use fe_format::IntegrityError as E;
    match err {
        E::NotSealed => PluginPackageError::UntrustedSigner("unsigned".into()),
        E::BadSignature => invalid("bad-signature"),
        E::DigestMismatch { path } => invalid(format!("digest-mismatch {path}")),
        E::Uncovered { path } => invalid(format!("uncommitted-member {path}")),
        E::MissingMember { path } => invalid(format!("missing-member {path}")),
        other => invalid(other.to_string()),
    }
}

fn read_plugin_member<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
    budget: &mut u64,
) -> Result<Option<Vec<u8>>, PluginPackageError> {
    read_named(archive, name, cap, budget).map_err(|e| invalid(format!("{e:#}")))
}

/// Verify sealed plugin bytes end to end: seal, closed member set, identity, contract, trust.
/// Nothing is stored; `expected_id` is the id the install asked for (REST body or registry document).
pub fn verify_plugin_package(
    bytes: &[u8],
    expected_id: Option<&str>,
    trust: &PluginTrust<'_>,
) -> Result<PluginPackage, PluginPackageError> {
    use fe_format::integrity::{
        content_hash, digest_hex, ENTRIES_PATH, MANIFEST_PATH, MEMBERS_DOC_PATH,
        PLUGIN_COMPONENT_PATH, PLUGIN_COUNTERSIGNATURES_PATH, PLUGIN_EXTENSION_PATH,
    };
    use fe_format::limits::{check_entry_count, MAX_ARCHIVE_UNCOMPRESSED_BYTES, MAX_JSON_ENTRY_BYTES};

    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| invalid(format!("invalid ZIP archive: {e}")))?;
    check_entry_count(archive.len()).map_err(|e| invalid(format!("{e:#}")))?;
    let mut budget = MAX_ARCHIVE_UNCOMPRESSED_BYTES;

    let manifest_raw =
        read_plugin_member(&mut archive, MANIFEST_PATH, MAX_JSON_ENTRY_BYTES, &mut budget)?
            .ok_or_else(|| invalid("missing-member manifest.json"))?;
    let manifest: fe_format::HexonManifest = serde_json::from_slice(&manifest_raw)
        .map_err(|e| invalid(format!("manifest.json: {e}")))?;
    if !matches!(manifest.hexon_type, fe_format::HexonType::Plugin) {
        return Err(invalid("not-a-plugin"));
    }
    // The id check runs before anything else is trusted (revocation-by-renaming steer).
    if let Some(expected) = expected_id {
        if expected != manifest.hexon_id {
            return Err(invalid("id-mismatch"));
        }
    }

    let unsigned = manifest.signature.as_deref().is_none_or(str::is_empty);
    if unsigned {
        if !trust.allow_unsigned_dev || manifest.integrity.is_some() {
            return Err(PluginPackageError::UntrustedSigner("unsigned".into()));
        }
    } else {
        fe_format::verify_archive(bytes, &fe_format::IntegrityPolicy::RequireSealed { pin: None })
            .map_err(integrity_refusal)?;
    }

    // Closed member set (a sealed package already refused uncommitted extras above).
    let allowed = [
        MANIFEST_PATH,
        ENTRIES_PATH,
        MEMBERS_DOC_PATH,
        PLUGIN_EXTENSION_PATH,
        PLUGIN_COMPONENT_PATH,
        PLUGIN_COUNTERSIGNATURES_PATH,
        "license.json",
    ];
    for name in archive.file_names() {
        if !name.ends_with('/') && !allowed.contains(&name) {
            return Err(invalid(format!("unexpected-member {name}")));
        }
    }

    let extension_json = read_plugin_member(
        &mut archive,
        PLUGIN_EXTENSION_PATH,
        PLUGIN_EXTENSION_MAX_BYTES,
        &mut budget,
    )?
    .ok_or_else(|| invalid(format!("missing-member {PLUGIN_EXTENSION_PATH}")))?;
    let component = read_plugin_member(
        &mut archive,
        PLUGIN_COMPONENT_PATH,
        PLUGIN_COMPONENT_MAX_BYTES,
        &mut budget,
    )?
    .ok_or_else(|| invalid(format!("missing-member {PLUGIN_COMPONENT_PATH}")))?;
    let license_json =
        read_plugin_member(&mut archive, "license.json", MAX_JSON_ENTRY_BYTES, &mut budget)?;
    let countersignatures = read_plugin_member(
        &mut archive,
        PLUGIN_COUNTERSIGNATURES_PATH,
        PLUGIN_COUNTERSIGNATURES_MAX_BYTES,
        &mut budget,
    )?;
    // `HexonArchiveIndex::open` (registry protocol v1) needs it; a plugin catalogues no assets.
    let entries = read_plugin_member(&mut archive, ENTRIES_PATH, MAX_JSON_ENTRY_BYTES, &mut budget)?
        .ok_or_else(|| invalid(format!("missing-member {ENTRIES_PATH}")))?;
    if serde_json::from_slice::<serde_json::Value>(&entries).ok() != Some(serde_json::json!({"entries": []})) {
        return Err(invalid("entries-not-empty"));
    }

    // The bytes returned are the ones this reader read, so they are re-checked against the root
    // `verify_archive` already verified: one source of truth, no parser differential.
    if !unsigned {
        let committed: Vec<String> = manifest
            .integrity
            .as_ref()
            .map(|root| root.header.keys().cloned().collect())
            .unwrap_or_default();
        let mut header_members = std::collections::BTreeMap::new();
        for path in committed {
            let bytes = match path.as_str() {
                PLUGIN_EXTENSION_PATH => extension_json.clone(),
                ENTRIES_PATH => entries.clone(),
                "license.json" if license_json.is_some() => license_json.clone().unwrap_or_default(),
                _ => read_plugin_member(&mut archive, &path, MAX_JSON_ENTRY_BYTES, &mut budget)?
                    .ok_or_else(|| invalid(format!("missing-member {path}")))?,
            };
            header_members.insert(path, bytes);
        }
        let policy = fe_format::IntegrityPolicy::RequireSealed { pin: None };
        match fe_format::verify_sealed_header(&manifest_raw, &header_members, &policy)
            .map_err(integrity_refusal)?
        {
            fe_format::SealCheck::Sealed(coverage) => {
                for (path, bytes) in [(PLUGIN_EXTENSION_PATH, &extension_json), (PLUGIN_COMPONENT_PATH, &component)] {
                    coverage.check_member(path, bytes).map_err(integrity_refusal)?;
                }
            }
            fe_format::SealCheck::Unsealed => {
                return Err(PluginPackageError::UntrustedSigner("unsigned".into()))
            }
        }
    }

    // Identity: the extension manifest must name the signed id and version.
    let extension: serde_json::Value = serde_json::from_slice(&extension_json)
        .map_err(|e| invalid(format!("extension.json: {e}")))?;
    let field = |key: &str| {
        extension
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    };
    if field("id") != manifest.hexon_id {
        return Err(invalid("id-mismatch"));
    }
    if field("version") != manifest.version {
        return Err(invalid("version-mismatch"));
    }
    if !contract_supported(field("contract"))
        || !component.starts_with(&WASM_COMPONENT_PREAMBLE)
    {
        return Err(PluginPackageError::UnsupportedContract);
    }

    let content_hash = content_hash(&manifest_raw).map_err(|e| invalid(format!("{e:#}")))?;
    let component_digest = digest_hex(&component);

    // Counter-signatures sign the same canonical bytes; only trust-root keys are checked.
    let mut countersigner: Option<String> = None;
    if let Some(raw) = countersignatures {
        let doc: CountersignatureDoc = serde_json::from_slice(&raw)
            .map_err(|_| invalid("countersignatures-malformed"))?;
        if doc.schema_version != 1 {
            return Err(invalid("countersignatures-malformed"));
        }
        if doc.content_hash != content_hash {
            return Err(invalid("countersignature-mismatch"));
        }
        if doc.entries.len() > PLUGIN_COUNTERSIGNATURES_MAX_ENTRIES {
            return Err(invalid("countersignatures-too-many"));
        }
        for entry in &doc.entries {
            if !(trust.is_trust_root)(&entry.key_id) {
                continue;
            }
            // fe-format's canonical verifier (`"signature": ""`), as `fe-ext countersign` signs (G3-C4).
            let verified =
                fe_format::verify_manifest(&manifest_raw, &entry.signature, &entry.key_id)
                    .unwrap_or(false);
            if !verified {
                return Err(invalid("countersignature-mismatch"));
            }
            countersigner.get_or_insert_with(|| entry.key_id.clone());
        }
    }

    let trusted_by = if unsigned {
        None
    } else if (trust.is_trust_root)(&manifest.publisher_did) {
        Some(manifest.publisher_did.clone())
    } else if countersigner.is_some() {
        countersigner
    } else {
        return Err(PluginPackageError::UntrustedSigner("no-trust-root".into()));
    };

    Ok(PluginPackage {
        package_id: manifest.hexon_id,
        version: manifest.version,
        publisher_did: manifest.publisher_did,
        content_hash,
        component_digest,
        extension_json,
        license_json,
        component,
        trusted_by,
    })
}

/// Why a store install failed: a verifier refusal (store untouched) or an I/O error.
#[derive(Debug, thiserror::Error)]
pub enum PluginInstallError {
    #[error(transparent)]
    Refused(#[from] PluginPackageError),
    #[error("package store: {0}")]
    Store(#[from] std::io::Error),
    /// An accepted kill list names the build's `content_hash` or `component_digest` (under any id).
    #[error("revoked ({hash})")]
    Revoked { hash: String },
}

/// Directory store of verified sealed plugin packages: `{root}/{package_id}/{content_hash}.hexon`.
pub struct PluginPackageStore {
    root: std::path::PathBuf,
}

impl PluginPackageStore {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Store path for a build; ids and hashes are charset-checked so a name never escapes the root.
    fn path_for(
        &self,
        package_id: &str,
        content_hash: &str,
    ) -> std::io::Result<std::path::PathBuf> {
        let id_ok = package_id.len() <= 128
            && package_id.split('.').count() >= 2
            && package_id.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
        let hash_ok = content_hash.len() == 64
            && content_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !id_ok || !hash_ok {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsafe package id or hash",
            ));
        }
        Ok(self
            .root
            .join(package_id)
            .join(format!("{content_hash}.hexon")))
    }

    /// Verify, refuse a kill-listed build, then store; a refusal leaves the store untouched. Idempotent
    /// per `(id, content_hash)`. `is_revoked` is the caller's effective list (minus any audited Owner override).
    pub fn install(
        &self,
        sealed: &[u8],
        expected_id: Option<&str>,
        trust: &PluginTrust<'_>,
        is_revoked: &dyn Fn(&str) -> bool,
    ) -> Result<PluginPackage, PluginInstallError> {
        let package = verify_plugin_package(sealed, expected_id, trust)?;
        if let Some(hash) = package.revocation_keys().into_iter().find(|hash| is_revoked(hash)) {
            return Err(PluginInstallError::Revoked { hash: hash.to_string() });
        }
        let path = self.path_for(&package.package_id, &package.content_hash)?;
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let staging = path.with_extension("hexon.tmp");
            std::fs::write(&staging, sealed)?;
            std::fs::rename(&staging, &path)?;
        }
        Ok(package)
    }

    /// Sealed bytes of an installed build; callers always re-verify before use.
    pub fn get(&self, package_id: &str, content_hash: &str) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.path_for(package_id, content_hash)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete one installed build; absent is not an error.
    pub fn remove(&self, package_id: &str, content_hash: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.path_for(package_id, content_hash)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Every installed `(package_id, content_hash)`, sorted.
    pub fn list(&self) -> std::io::Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let Ok(ids) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        for id in ids.flatten().filter(|e| e.path().is_dir()) {
            let package_id = id.file_name().to_string_lossy().into_owned();
            for file in std::fs::read_dir(id.path())?.flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if let Some(hash) = name.strip_suffix(".hexon") {
                    out.push((package_id.clone(), hash.to_string()));
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::collections::HashMap;

    use crate::manifest::{AssetEntry, EntryKind, HexonManifest, HexonType, License, LicenseType};
    use crate::signature::asset_hash;

    const MODEL_BYTES: &[u8] = b"fake gltf data";

    fn test_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn test_manifest(keypair: &SigningKey) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "test-model-v1".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: fe_format::verifying_key_to_did(&keypair.verifying_key()),
            publisher_name: Some("Test Publisher".to_string()),
            version: "1.0.0".to_string(),
            build_id: Some("build-001".to_string()),
            name: "Test Model".to_string(),
            description: Some("A test model".to_string()),
            tags: vec!["test".to_string()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(1024),
            min_engine_version: Some("0.1.0".to_string()),
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }

    /// One catalog entry with every optional field empty.
    fn entry(entry_id: &str, kind: EntryKind, data: &[u8], format: &str, label: &str) -> AssetEntry {
        AssetEntry {
            entry_id: entry_id.to_string(),
            kind,
            asset_hash: asset_hash(data),
            format: format.to_string(),
            label: label.to_string(),
            tags: vec![],
            description: None,
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }
    }

    fn test_entries() -> Vec<AssetEntry> {
        vec![entry("model_001", EntryKind::Model, MODEL_BYTES, "gltf", "Test Model")]
    }

    fn test_license() -> Option<License> {
        Some(License::default())
    }

    fn model_assets() -> HashMap<String, Vec<u8>> {
        HashMap::from([(asset_hash(MODEL_BYTES), MODEL_BYTES.to_vec())])
    }

    #[test]
    fn test_build_and_open_roundtrip() {
        let keypair = test_signing_key();
        let manifest = test_manifest(&keypair);
        let schema = r#"{"field_defs":[]}"#.to_string();

        let zip_bytes = HexonPackage::build(
            manifest.clone(),
            test_entries(),
            test_license(),
            model_assets(),
            Some(b"icon png".to_vec()),
            HashMap::from([("thumbnail.png".to_string(), b"thumb".to_vec())]),
            Some("# Test README".to_string()),
            Some(schema.clone()),
            &keypair,
        )
        .expect("build should succeed");

        let package = HexonPackage::open(&zip_bytes).expect("open should succeed");
        assert_eq!(package.manifest.hexon_id, manifest.hexon_id);
        assert_eq!(package.hexon_uri(), "test-model-v1@1.0.0");
        assert_eq!(package.entries.len(), 1);
        assert_eq!(package.assets.get(&asset_hash(MODEL_BYTES)).map(Vec::as_slice), Some(MODEL_BYTES));
        assert_eq!(package.icon.as_deref(), Some(&b"icon png"[..]));
        assert_eq!(package.previews.get("thumbnail.png").map(Vec::as_slice), Some(&b"thumb"[..]));
        assert_eq!(package.readme.as_deref(), Some("# Test README"));
        assert_eq!(package.schema.as_deref(), Some(schema.as_str()));
        assert!(matches!(package.license.as_ref().map(|l| &l.license_type), Some(LicenseType::Free)));
        assert_eq!(
            package.integrity,
            IntegrityStatus::Sealed { publisher_did: manifest.publisher_did.clone() }
        );
        assert!(package.signature_valid(), "signature should be valid");

        // The same bytes pass fe-format's own strict reader: one format, one signing scheme.
        let policy = fe_format::IntegrityPolicy::RequireSealed { pin: Some(manifest.publisher_did) };
        assert!(fe_format::verify_archive(&zip_bytes, &policy).is_ok());

        // A preview name that would escape `preview/` is refused at build.
        let escaping = HexonPackage::build(
            test_manifest(&keypair),
            test_entries(),
            None,
            model_assets(),
            None,
            HashMap::from([("../manifest.json".to_string(), vec![1])]),
            None,
            None,
            &keypair,
        );
        assert!(escaping.is_err(), "preview path traversal must be refused");
    }

    #[test]
    fn test_asset_hashes_match_blake3() {
        let keypair = test_signing_key();
        let entries = vec![
            entry("model_001", EntryKind::Model, MODEL_BYTES, "gltf", "Test Model"),
            entry("texture_001", EntryKind::Texture, b"fake png bytes", "png", "Diffuse"),
        ];
        let mut assets = model_assets();
        assets.insert(entries[1].asset_hash.clone(), b"fake png bytes".to_vec());

        let zip_bytes = HexonPackage::build(
            test_manifest(&keypair),
            entries,
            None,
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert!(package.license.is_none());
        assert_eq!(package.entries.len(), 2);
        for entry in &package.entries {
            let blob = package
                .assets
                .get(&entry.asset_hash)
                .expect("asset blob must exist for entry");
            assert_eq!(asset_hash(blob), entry.asset_hash, "BLAKE3 hash mismatch for entry {}", entry.entry_id);
        }
    }

    #[test]
    fn test_multi_format_entries() {
        let keypair = test_signing_key();
        let manifest = HexonManifest {
            hexon_type: HexonType::Material,
            ..test_manifest(&keypair)
        };

        let albedo_data = b"albedo texture bytes";
        let normal_data = b"normal map bytes";
        let roughness_data = b"roughness bytes";

        let material = AssetEntry {
            tags: vec!["pbr".to_string()],
            description: Some("A PBR material with sub-assets".to_string()),
            metadata: Some(serde_json::json!({"shader": "standard"})),
            sub_assets: Some(HashMap::from([
                ("normal".to_string(), asset_hash(normal_data)),
                ("roughness".to_string(), asset_hash(roughness_data)),
            ])),
            ..entry("mat_pbr", EntryKind::Material, albedo_data, "pbr_bundle", "PBR Material")
        };
        let entries = vec![
            material,
            entry("tex_normal", EntryKind::Texture, normal_data, "png", "Normal Map"),
            entry("tex_roughness", EntryKind::Texture, roughness_data, "png", "Roughness"),
        ];

        let license = License {
            license_type: LicenseType::Attribution,
            attribution: Some("CC-BY 4.0".to_string()),
            free_entries: vec!["mat_pbr".to_string()],
            ..License::default()
        };

        let assets: HashMap<String, Vec<u8>> = [&albedo_data[..], &normal_data[..], &roughness_data[..]]
            .into_iter()
            .map(|data| (asset_hash(data), data.to_vec()))
            .collect();

        let zip_bytes = HexonPackage::build(
            manifest,
            entries,
            Some(license),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert!(package.signature_valid());
        assert_eq!(package.manifest.hexon_type, HexonType::Material);
        assert_eq!(package.entries.len(), 3);
        assert_eq!(package.assets.len(), 3);
        let license = package.license.as_ref().expect("licence present");
        assert!(matches!(license.license_type, LicenseType::Attribution));
        assert_eq!(license.free_entries, vec!["mat_pbr"]);

        let mat = package
            .entries
            .iter()
            .find(|e| e.entry_id == "mat_pbr")
            .unwrap();
        let sub = mat.sub_assets.as_ref().unwrap();
        assert!(sub.contains_key("normal"));
        assert!(sub.contains_key("roughness"));

        for entry in &package.entries {
            let blob = package.assets.get(&entry.asset_hash).unwrap();
            assert_eq!(asset_hash(blob), entry.asset_hash);
        }
    }

    #[test]
    fn test_terrain_and_skybox_entries() {
        let keypair = test_signing_key();

        // Terrain package
        let terrain_manifest = HexonManifest {
            hexon_id: "terrain-alps-v1".to_string(),
            hexon_type: HexonType::Terrain,
            name: "Alps Terrain".to_string(),
            ..test_manifest(&keypair)
        };

        let tile_data = b"fake terrain tile rgb bytes";
        let gpx_data = b"<?xml version='1.0'?><gpx></gpx>";

        let entries = vec![
            AssetEntry {
                tags: vec!["terrain".to_string(), "elevation".to_string()],
                metadata: Some(
                    serde_json::json!({"zoom_min": 5, "zoom_max": 14, "bounds": [5.9, 45.8, 10.5, 47.8]}),
                ),
                ..entry("tileset_01", EntryKind::TerrainTileset, tile_data, "terrain_rgb", "Alps Elevation")
            },
            AssetEntry {
                is_placeable: false,
                ..entry("gpx_trail", EntryKind::GpxTrack, gpx_data, "gpx", "Alpine Trail")
            },
        ];

        let assets = HashMap::from([
            (asset_hash(tile_data), tile_data.to_vec()),
            (asset_hash(gpx_data), gpx_data.to_vec()),
        ]);

        let zip_bytes = HexonPackage::build(
            terrain_manifest,
            entries,
            test_license(),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert_eq!(package.manifest.hexon_type, HexonType::Terrain);
        assert_eq!(package.hexon_uri(), "terrain-alps-v1@1.0.0");
        assert_eq!(package.entries.len(), 2);
        assert!(matches!(package.entries[1].kind, EntryKind::GpxTrack));
        assert!(package.signature_valid());

        // Skybox package
        let skybox_manifest = HexonManifest {
            hexon_id: "skybox-sunset-v1".to_string(),
            hexon_type: HexonType::Skybox,
            name: "Sunset Skybox".to_string(),
            ..test_manifest(&keypair)
        };

        let hdr_data = b"fake HDR image data";
        let skybox_entries = vec![AssetEntry {
            tags: vec!["skybox".to_string()],
            metadata: Some(serde_json::json!({"resolution": [4096, 2048]})),
            ..entry("sky_hdr", EntryKind::Skybox, hdr_data, "hdr", "Sunset HDR")
        }];
        let skybox_assets = HashMap::from([(asset_hash(hdr_data), hdr_data.to_vec())]);

        let paid = License {
            license_type: LicenseType::Paid,
            payment_provider: Some("example-pay".to_string()),
            payment_verification_url: Some("https://example.com/verify".to_string()),
            ..License::default()
        };

        let zip2 = HexonPackage::build(
            skybox_manifest,
            skybox_entries,
            Some(paid),
            skybox_assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let pkg2 = HexonPackage::open(&zip2).unwrap();
        assert_eq!(pkg2.manifest.hexon_type, HexonType::Skybox);
        let license = pkg2.license.as_ref().expect("licence present");
        assert!(matches!(license.license_type, LicenseType::Paid));
        assert!(license.payment_provider.is_some());
        assert!(pkg2.signature_valid());
    }

    #[test]
    fn test_baked_splat_tile_record_json_roundtrip() {
        let record = BakedSplatTileRecord {
            version: BAKED_SPLAT_TILE_RECORD_VERSION,
            cache_key: "10/512/340".to_string(),
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            colors: vec![[1.0, 0.0, 0.0, 1.0]; 2],
            scales: vec![[0.5, 0.4]; 2],
            normals: vec![[0.0, 1.0, 0.0]; 2],
        };
        let json = serde_json::to_string(&record).expect("serialize should succeed");
        let back: BakedSplatTileRecord =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(back.cache_key, record.cache_key);
        assert_eq!(back.positions, record.positions);
        assert_eq!(back.colors, record.colors);
        assert_eq!(back.scales, record.scales);
        assert_eq!(back.normals, record.normals);
        assert_eq!(back.version, record.version);
    }

    #[test]
    fn test_baked_splat_tile_record_defaults_empty() {
        let record = BakedSplatTileRecord::default();
        assert!(record.positions.is_empty());
        assert!(record.cache_key.is_empty());
    }

    #[test]
    fn test_tampered_package_fails_signature() {
        let keypair = test_signing_key();
        let zip_bytes = HexonPackage::build(
            test_manifest(&keypair),
            test_entries(),
            test_license(),
            model_assets(),
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .expect("build should succeed");
        assert!(HexonPackage::open(&zip_bytes).unwrap().signature_valid());

        // J5 package tamper table: sealed plugin packages, each variant refused with its
        // own reason and the package store left empty (docs/extensions/package-format.md).
        let publisher = SigningKey::from_bytes(&[11u8; 32]);
        let outsider = SigningKey::from_bytes(&[12u8; 32]);
        let reviewer = SigningKey::from_bytes(&[13u8; 32]);
        let did = |key: &SigningKey| fe_format::verifying_key_to_did(&key.verifying_key());
        let (publisher_did, reviewer_did) = (did(&publisher), did(&reviewer));
        let extension = br#"{"id":"org.example.fleet-sim","version":"1.0.0","name":"Fleet","contract":"fractalengine:plugin@2.0.0"}"#;
        let component = [&WASM_COMPONENT_PREAMBLE[..], b"body"].concat();
        let pack = |key: &SigningKey, component: &[u8]| {
            crate::publisher::PluginHexonBuilder::new(&extension[..], component)
                .build(key)
                .expect("plugin builds")
        };
        // Rewrite a sealed package: `edit` maps (member, bytes) to new bytes or None to drop it.
        type MemberEdit<'a> = &'a dyn Fn(&str, Vec<u8>) -> Option<Vec<u8>>;
        let rezip = |bytes: &[u8], edit: MemberEdit<'_>, extra: Option<(&str, Vec<u8>)>| {
            let mut source = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default();
            for i in 0..source.len() {
                let mut file = source.by_index(i).unwrap();
                let name = file.name().to_string();
                let mut body = Vec::new();
                file.read_to_end(&mut body).unwrap();
                if let Some(body) = edit(&name, body) {
                    out.start_file(name, options).unwrap();
                    out.write_all(&body).unwrap();
                }
            }
            if let Some((name, body)) = extra {
                out.start_file(name, options).unwrap();
                out.write_all(&body).unwrap();
            }
            out.finish().unwrap().into_inner()
        };
        let keep = |_: &str, body: Vec<u8>| Some(body);

        // Hexon package tamper table: each edit of a sealed package fails closed, either refused at
        // open (seal check) or opened unsigned (seal stripped), never `signature_valid()`.
        let flip_asset = |name: &str, mut body: Vec<u8>| {
            if name.starts_with(fe_format::integrity::ASSETS_PREFIX) {
                body[0] ^= 1;
            }
            Some(body)
        };
        let edit_manifest = |field: &'static str, value: Option<serde_json::Value>| {
            move |name: &str, body: Vec<u8>| {
                if name != "manifest.json" {
                    return Some(body);
                }
                let mut doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let object = doc.as_object_mut().unwrap();
                match &value {
                    Some(value) => object.insert(field.to_string(), value.clone()),
                    None => object.remove(field),
                };
                Some(serde_json::to_vec(&doc).unwrap())
            }
        };
        let rename = edit_manifest("name", Some(serde_json::json!("Renamed")));
        let strip_seal = edit_manifest("integrity", None);
        let swap_publisher = edit_manifest(
            "publisher_did",
            Some(serde_json::json!(fe_format::verifying_key_to_did(&test_signing_key().verifying_key()))),
        );
        let package_rows: [(&str, MemberEdit<'_>); 4] = [
            ("flipped asset byte", &flip_asset),
            ("renamed in manifest", &rename),
            ("seal stripped", &strip_seal),
            ("publisher swapped", &swap_publisher),
        ];
        for (label, edit) in package_rows {
            let tampered = rezip(&zip_bytes, edit, None);
            let verified = HexonPackage::open(&tampered).map(|package| package.signature_valid());
            assert!(!verified.unwrap_or(false), "{label}: tampered package must not verify");
        }
        let trusted_publisher = |did: &str| did == publisher_did;
        let trust = PluginTrust {
            is_trust_root: &trusted_publisher,
            allow_unsigned_dev: false,
        };
        let sealed = pack(&publisher, &component);
        let store_dir = std::env::temp_dir().join(format!("fe-plugin-store-{}", ulid::Ulid::new()));
        let store = PluginPackageStore::new(&store_dir);

        let flip = |name: &str, mut body: Vec<u8>| {
            if name == "plugin/component.wasm" {
                body[10] ^= 1;
            }
            Some(body)
        };
        let widen = |name: &str, body: Vec<u8>| {
            if name == "plugin/extension.json" {
                return Some(br#"{"id":"org.example.fleet-sim","version":"1.0.0","name":"Fleet","contract":"fractalengine:plugin@2.0.0","scene":{"read":true}}"#.to_vec());
            }
            Some(body)
        };
        let drop_component = |name: &str, body: Vec<u8>| (name != "plugin/component.wasm").then_some(body);
        let drop_extension = |name: &str, body: Vec<u8>| (name != "plugin/extension.json").then_some(body);
        let unsign = |name: &str, body: Vec<u8>| {
            if name == "manifest.json" {
                let mut doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
                doc.as_object_mut().unwrap().remove("signature");
                return Some(serde_json::to_vec(&doc).unwrap());
            }
            Some(body)
        };
        let manifest_json = {
            let mut archive = zip::ZipArchive::new(Cursor::new(&sealed[..])).unwrap();
            let mut body = Vec::new();
            archive.by_name("manifest.json").unwrap().read_to_end(&mut body).unwrap();
            body
        };
        let bad_countersig = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "content_hash": fe_format::integrity::content_hash(&manifest_json).unwrap(),
            "entries": [{"key_id": reviewer_did, "signature": "AAAA"}],
        }))
        .unwrap();
        let reviewer_trust = |did: &str| did == publisher_did || did == reviewer_did;
        let with_reviewer = PluginTrust {
            is_trust_root: &reviewer_trust,
            allow_unsigned_dev: false,
        };

        type Refusal = PluginPackageError;
        type RefusalRow<'a> = (&'a str, Vec<u8>, Option<&'a str>, &'a PluginTrust<'a>, Refusal);
        let rows: Vec<RefusalRow<'_>> = vec![
            (
                "flipped component byte",
                rezip(&sealed, &flip, None),
                None,
                &trust,
                invalid("digest-mismatch plugin/component.wasm"),
            ),
            (
                "edited extension.json",
                rezip(&sealed, &widen, None),
                None,
                &trust,
                invalid("digest-mismatch plugin/extension.json"),
            ),
            (
                "uncommitted extra member",
                rezip(&sealed, &keep, Some(("plugin/extra.bin", vec![1]))),
                None,
                &trust,
                invalid("uncommitted-member plugin/extra.bin"),
            ),
            (
                "removed component member",
                rezip(&sealed, &drop_component, None),
                None,
                &trust,
                invalid("missing-member plugin/component.wasm"),
            ),
            (
                "removed extension member",
                rezip(&sealed, &drop_extension, None),
                None,
                &trust,
                invalid("missing-member plugin/extension.json"),
            ),
            (
                "signature removed",
                rezip(&sealed, &unsign, None),
                None,
                &trust,
                Refusal::UntrustedSigner("unsigned".into()),
            ),
            (
                "signed by a non-root key",
                pack(&outsider, &component),
                None,
                &trust,
                Refusal::UntrustedSigner("no-trust-root".into()),
            ),
            (
                "trusted counter-signature with a bad signature",
                rezip(&sealed, &keep, Some(("plugin/countersignatures.json", bad_countersig))),
                None,
                &with_reviewer,
                invalid("countersignature-mismatch"),
            ),
            (
                "core module packaged as plugin",
                pack(&publisher, b"\0asm\x01\0\0\0body"),
                None,
                &trust,
                Refusal::UnsupportedContract,
            ),
            (
                "contract minor newer than the host binds",
                crate::publisher::PluginHexonBuilder::new(
                    &br#"{"id":"org.example.fleet-sim","version":"1.0.0","name":"Fleet","contract":"fractalengine:plugin@2.1.0"}"#[..],
                    &component[..],
                )
                .build(&publisher)
                .unwrap(),
                None,
                &trust,
                Refusal::UnsupportedContract,
            ),
            (
                "requested under another id",
                sealed.clone(),
                Some("org.example.other"),
                &trust,
                invalid("id-mismatch"),
            ),
        ];
        // The untouched control verifies under the same trust the tamper rows are refused under.
        let control = verify_plugin_package(&sealed, None, &trust).expect("untouched control verifies");
        assert_eq!(control.package_id, "org.example.fleet-sim");
        let never_revoked = |_: &str| false;
        for (label, bytes, expected_id, trust, expected) in rows {
            let err = store
                .install(&bytes, expected_id, trust, &never_revoked)
                .expect_err(label);
            assert!(
                matches!(&err, PluginInstallError::Refused(got) if *got == expected),
                "{label}: got {err:?}, want {expected:?}"
            );
            assert!(store.list().unwrap().is_empty(), "{label}: store must stay empty");
        }

        // Revocation matches by hash under any id: the content hash listed under another package id,
        // and the same component re-sealed under a new id with its component digest listed.
        let listed_hash = verify_plugin_package(&sealed, None, &trust).unwrap().content_hash;
        let resealed = crate::publisher::PluginHexonBuilder::new(
            &br#"{"id":"org.example.renamed","version":"1.0.0","name":"Fleet","contract":"fractalengine:plugin@2.0.0"}"#[..],
            &component[..],
        )
        .build(&publisher)
        .unwrap();
        let component_digest = fe_format::integrity::digest_hex(&component);
        for (label, bytes, listed) in [
            ("content hash listed under another id", &sealed, listed_hash),
            ("component re-sealed under a new id", &resealed, component_digest),
        ] {
            let kill_list = |hash: &str| hash == listed;
            let err = store.install(bytes, None, &trust, &kill_list).expect_err(label);
            assert!(matches!(err, PluginInstallError::Revoked { .. }), "{label}: got {err:?}");
            assert!(store.list().unwrap().is_empty(), "{label}: store must stay empty");
        }

        // A trust-root counter-signature over the canonical signing bytes vouches for an outsider's build.
        let outsider_sealed = pack(&outsider, &component);
        let outsider_manifest = {
            let mut archive = zip::ZipArchive::new(Cursor::new(&outsider_sealed[..])).unwrap();
            let mut body = Vec::new();
            archive.by_name("manifest.json").unwrap().read_to_end(&mut body).unwrap();
            body
        };
        let good_countersig = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "content_hash": fe_format::integrity::content_hash(&outsider_manifest).unwrap(),
            "entries": [{"key_id": reviewer_did, "signature": fe_format::sign_manifest(&outsider_manifest, &reviewer).unwrap()}],
        }))
        .unwrap();
        let vouched = rezip(&outsider_sealed, &keep, Some(("plugin/countersignatures.json", good_countersig)));
        let vouched = verify_plugin_package(&vouched, None, &with_reviewer).expect("counter-signed build verifies");
        assert_eq!(vouched.trusted_by.as_deref(), Some(reviewer_did.as_str()));

        let installed = store.install(&sealed, Some("org.example.fleet-sim"), &trust, &never_revoked).unwrap();
        assert_eq!(installed.trusted_by.as_deref(), Some(publisher_did.as_str()));
        assert_eq!(installed.component_digest, fe_format::integrity::digest_hex(&component));
        assert_eq!(
            store.list().unwrap(),
            vec![("org.example.fleet-sim".to_string(), installed.content_hash.clone())]
        );
        assert_eq!(store.get("org.example.fleet-sim", &installed.content_hash).unwrap().unwrap(), sealed);
        let _ = std::fs::remove_dir_all(&store_dir);
    }

    /// A package whose `assets/bomb` inflates past the 64 MiB per-entry cap from a
    /// sub-MiB archive must be refused by `open`.
    #[test]
    fn test_open_refuses_zip_bomb_asset() {
        let keypair = test_signing_key();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let plain = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            (
                "manifest.json",
                serde_json::to_vec(&test_manifest(&keypair)).unwrap(),
            ),
            ("entries.json", br#"{"entries":[]}"#.to_vec()),
            ("license.json", serde_json::to_vec(&License::default()).unwrap()),
        ] {
            zip.start_file(name, plain).unwrap();
            zip.write_all(&body).unwrap();
        }
        let deflate = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1));
        zip.start_file("assets/bomb", deflate).unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..(64 + 8) {
            zip.write_all(&chunk).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        assert!(bytes.len() < (1 << 20), "fixture must be a real bomb");

        let err = HexonPackage::open(&bytes).expect_err("bomb must be refused");
        assert!(
            format!("{err:#}").contains(fe_format::limits::DECOMPRESSION_LIMIT_ERROR),
            "unexpected error: {err:#}"
        );
    }
}
