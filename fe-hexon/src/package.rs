use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use crate::manifest::{AssetEntry, HexonManifest, License};
use crate::signature;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

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

/// In-memory representation of a fully parsed .fecrate package.
#[derive(Debug, Clone)]
pub struct HexonPackageData {
    pub manifest: HexonManifest,
    pub entries: Vec<AssetEntry>,
    pub license: License,
    pub icon: Option<Vec<u8>>,
    pub previews: HashMap<String, Vec<u8>>,
    pub assets: HashMap<String, Vec<u8>>, // keyed by asset_hash
    pub readme: Option<String>,
    pub schema: Option<String>,
    /// Whether the manifest signature was verified successfully on open.
    pub signature_verified: bool,
}

impl HexonPackageData {
    /// Returns true if the manifest signature is valid.
    pub fn signature_valid(&self) -> bool {
        self.signature_verified
    }

    /// Compute the hexon URI for this package.
    pub fn hexon_uri(&self) -> String {
        hexon_uri(&self.manifest)
    }
}

/// Builds or opens .fecrate ZIP packages.
pub struct HexonPackage;

impl HexonPackage {
    /// Build a .fecrate ZIP from a manifest, entries, assets directory, and signing key.
    ///
    /// Returns the raw ZIP bytes.
    #[allow(clippy::too_many_arguments)] // package builder — params mirror archive sections
    pub fn build(
        manifest: HexonManifest,
        entries: Vec<AssetEntry>,
        license: License,
        assets: HashMap<String, Vec<u8>>, // asset_hash -> bytes
        icon: Option<Vec<u8>>,
        previews: HashMap<String, Vec<u8>>,
        readme: Option<String>,
        schema: Option<String>,
        signing_key: &SigningKey,
    ) -> Result<Vec<u8>, anyhow::Error> {
        // Serialize JSON components
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| anyhow::anyhow!("manifest serialization failed: {}", e))?;
        let entries_json = serde_json::to_vec_pretty(&entries)
            .map_err(|e| anyhow::anyhow!("entries serialization failed: {}", e))?;
        let license_json = serde_json::to_vec_pretty(&license)
            .map_err(|e| anyhow::anyhow!("license serialization failed: {}", e))?;

        // Sign the manifest
        let sig = signature::sign_manifest(&manifest_json, signing_key)?;

        // Build ZIP in memory
        let mut zip_writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        // manifest.json
        zip_writer.start_file("manifest.json", options)?;
        zip_writer.write_all(&manifest_json)?;

        // entries.json
        zip_writer.start_file("entries.json", options)?;
        zip_writer.write_all(&entries_json)?;

        // license.json
        zip_writer.start_file("license.json", options)?;
        zip_writer.write_all(&license_json)?;

        // signature.txt
        zip_writer.start_file("signature.txt", options)?;
        zip_writer.write_all(sig.as_bytes())?;

        // icon.png (optional)
        if let Some(icon_bytes) = icon {
            zip_writer.start_file("icon.png", options)?;
            zip_writer.write_all(&icon_bytes)?;
        }

        // preview/* (optional)
        for (name, bytes) in &previews {
            zip_writer.start_file(format!("preview/{}", name), options)?;
            zip_writer.write_all(bytes)?;
        }

        // assets/{hash}
        for (hash, bytes) in &assets {
            zip_writer.start_file(format!("assets/{}", hash), options)?;
            zip_writer.write_all(bytes)?;
        }

        // README.md (optional)
        if let Some(readme_text) = readme {
            zip_writer.start_file("README.md", options)?;
            zip_writer.write_all(readme_text.as_bytes())?;
        }

        // schema.json (optional)
        if let Some(schema_text) = schema {
            zip_writer.start_file("schema.json", options)?;
            zip_writer.write_all(schema_text.as_bytes())?;
        }

        let cursor = zip_writer.finish()?;
        Ok(cursor.into_inner())
    }

    /// Open and parse a .fecrate ZIP from raw bytes.
    pub fn open(bytes: &[u8]) -> Result<HexonPackageData, anyhow::Error> {
        use fe_format::limits::{
            check_entry_count, MAX_ARCHIVE_UNCOMPRESSED_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES,
            MAX_JSON_ENTRY_BYTES,
        };
        let reader = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| anyhow::anyhow!("invalid ZIP archive: {}", e))?;
        check_entry_count(archive.len())?;
        // One decompression budget for the whole package (zip-bomb guard,
        // fe-format AGENTS.md §decompression-limits).
        let mut budget = MAX_ARCHIVE_UNCOMPRESSED_BYTES;
        let (json, binary) = (MAX_JSON_ENTRY_BYTES, MAX_ENTRY_UNCOMPRESSED_BYTES);

        // Read manifest.json (keep raw bytes for signature verification)
        let (manifest, manifest_raw): (HexonManifest, Vec<u8>) = {
            let buf = read_named(&mut archive, "manifest.json", json, &mut budget)?
                .ok_or_else(|| anyhow::anyhow!("missing manifest.json"))?;
            let parsed: HexonManifest = serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("manifest.json parse error: {}", e))?;
            (parsed, buf)
        };

        // Read entries.json
        let entries: Vec<AssetEntry> = {
            let buf = read_named(&mut archive, "entries.json", json, &mut budget)?
                .ok_or_else(|| anyhow::anyhow!("missing entries.json"))?;
            serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("entries.json parse error: {}", e))?
        };

        // Read license.json
        let license: License = {
            let buf = read_named(&mut archive, "license.json", json, &mut budget)?
                .ok_or_else(|| anyhow::anyhow!("missing license.json"))?;
            serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("license.json parse error: {}", e))?
        };

        // Read signature.txt
        let signature = {
            let buf = read_named(&mut archive, "signature.txt", json, &mut budget)?
                .ok_or_else(|| anyhow::anyhow!("missing signature.txt"))?;
            String::from_utf8(buf)?.trim().to_string()
        };

        // Read optional icon.png
        let icon = read_named(&mut archive, "icon.png", binary, &mut budget)?;

        // Read preview/* and assets/{hash} files
        let mut previews = HashMap::new();
        let mut assets = HashMap::new();
        for i in 0..archive.len() {
            let name = {
                let file = archive.by_index(i)?;
                file.enclosed_name()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string())
            };
            let Some(name) = name else { continue };
            let target = if let Some(file_name) = name.strip_prefix("preview/") {
                (&mut previews, file_name)
            } else if let Some(hash) = name.strip_prefix("assets/") {
                (&mut assets, hash)
            } else {
                continue;
            };
            if target.1.is_empty() {
                continue;
            }
            let mut f = archive.by_index(i)?;
            let declared = f.size();
            let buf = fe_format::read_entry_capped(&mut f, declared, binary, &mut budget)
                .map_err(|e| e.context(format!("reading {name}")))?;
            target.0.insert(target.1.to_string(), buf);
        }

        // Read optional README.md / schema.json (non-UTF-8 is treated as absent, as before)
        let readme = read_named(&mut archive, "README.md", json, &mut budget)?
            .and_then(|b| String::from_utf8(b).ok());
        let schema = read_named(&mut archive, "schema.json", json, &mut budget)?
            .and_then(|b| String::from_utf8(b).ok());

        // Verify signature against raw manifest bytes (not re-serialized)
        let sig_valid =
            signature::verify_manifest(&manifest_raw, &signature, &manifest.publisher_did)?;

        let mut signed_manifest = manifest.clone();
        signed_manifest.signature = signature;

        Ok(HexonPackageData {
            manifest: signed_manifest,
            entries,
            license,
            icon,
            previews,
            assets,
            readme,
            schema,
            signature_verified: sig_valid,
        })
    }
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

/// Extract a hexon URI from a manifest.
/// Format: `{crate_id}@{version}`
pub fn hexon_uri(manifest: &HexonManifest) -> String {
    format!("{}@{}", manifest.crate_id, manifest.version)
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
            patch.parse::<u64>().is_ok() && minor.parse::<u64>().is_ok_and(|minor| minor <= HOST_CONTRACT_MINOR)
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

    let unsigned = manifest.signature.as_deref().map_or(true, str::is_empty);
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

    // The bytes returned are the ones read here (zip 0.6), so they are re-checked against the root
    // `verify_archive` (zip 2) already verified: one source of truth, no parser differential.
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
            // fe-format's verifier: canonical `signing_bytes` (`"signature": ""`), as `fe-ext countersign`
            // signs; fe-hexon's `signature::verify_manifest` checks raw bytes and refuses every one (G3-C4).
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

    use crate::manifest::{AssetEntry, EntryKind, HexonKind, HexonManifest, License, LicenseType};

    fn test_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn test_manifest(keypair: &SigningKey) -> HexonManifest {
        let pub_key = keypair.verifying_key();
        let did = {
            use multibase::Base;
            let mut bytes = vec![0xed, 0x01];
            bytes.extend_from_slice(&pub_key.to_bytes());
            format!("did:key:{}", multibase::encode(Base::Base58Btc, &bytes))
        };

        HexonManifest {
            schema_version: 1,
            crate_id: "test-model-v1".to_string(),
            publisher_did: did,
            publisher_name: "Test Publisher".to_string(),
            version: "1.0.0".to_string(),
            build_id: "build-001".to_string(),
            name: "Test Model".to_string(),
            description: "A test model".to_string(),
            tags: vec!["test".to_string()],
            hexon_type: HexonKind::Model,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            approx_size_bytes: 1024,
            min_engine_version: "0.1.0".to_string(),
            homepage_url: None,
            dependencies: vec![],
            signature: String::new(),
        }
    }

    fn test_entries() -> Vec<AssetEntry> {
        vec![AssetEntry {
            entry_id: "model_001".to_string(),
            kind: EntryKind::Model,
            asset_hash: "abc123".to_string(),
            format: "gltf".to_string(),
            label: "Test Model".to_string(),
            tags: vec![],
            description: "A test model entry".to_string(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: None,
        }]
    }

    fn test_license() -> License {
        License {
            license_type: LicenseType::Free,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec![],
            encrypted_key: None,
        }
    }

    #[test]
    fn test_build_and_open_roundtrip() {
        let keypair = test_signing_key();
        let manifest = test_manifest(&keypair);
        let entries = test_entries();
        let license = test_license();

        let mut assets = HashMap::new();
        assets.insert("abc123".to_string(), b"fake gltf data".to_vec());

        let zip_bytes = HexonPackage::build(
            manifest.clone(),
            entries.clone(),
            license.clone(),
            assets.clone(),
            None,
            HashMap::new(),
            Some("# Test README".to_string()),
            None,
            &keypair,
        )
        .expect("build should succeed");

        let package = HexonPackage::open(&zip_bytes).expect("open should succeed");
        assert_eq!(package.manifest.crate_id, manifest.crate_id);
        assert_eq!(package.entries.len(), entries.len());
        assert_eq!(package.assets.len(), 1);
        assert!(package.readme.is_some());
        assert!(package.signature_valid(), "signature should be valid");
    }

    #[test]
    fn test_hexon_uri_format() {
        let keypair = test_signing_key();
        let manifest = test_manifest(&keypair);
        let uri = hexon_uri(&manifest);
        assert_eq!(uri, "test-model-v1@1.0.0");
    }

    #[test]
    fn test_asset_hashes_match_blake3() {
        let keypair = test_signing_key();
        let manifest = test_manifest(&keypair);
        let entries = vec![
            AssetEntry {
                entry_id: "model_001".to_string(),
                kind: EntryKind::Model,
                asset_hash: crate::signature::asset_hash(b"fake gltf data"),
                format: "gltf".to_string(),
                label: "Test Model".to_string(),
                tags: vec![],
                description: String::new(),
                is_placeable: true,
                is_private: false,
                preview_image: None,
                metadata: None,
                sub_assets: None,
            },
            AssetEntry {
                entry_id: "texture_001".to_string(),
                kind: EntryKind::Texture,
                asset_hash: crate::signature::asset_hash(b"fake png bytes"),
                format: "png".to_string(),
                label: "Diffuse".to_string(),
                tags: vec![],
                description: String::new(),
                is_placeable: false,
                is_private: false,
                preview_image: None,
                metadata: None,
                sub_assets: None,
            },
        ];
        let license = License {
            license_type: LicenseType::Free,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec![],
            encrypted_key: None,
        };

        let mut assets = HashMap::new();
        assets.insert(entries[0].asset_hash.clone(), b"fake gltf data".to_vec());
        assets.insert(entries[1].asset_hash.clone(), b"fake png bytes".to_vec());

        let zip_bytes = HexonPackage::build(
            manifest,
            entries.clone(),
            license,
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();

        // Verify every entry's asset_hash matches BLAKE3 of the actual blob
        for entry in &package.entries {
            let blob = package
                .assets
                .get(&entry.asset_hash)
                .expect("asset blob must exist for entry");
            let computed = crate::signature::asset_hash(blob);
            assert_eq!(
                computed, entry.asset_hash,
                "BLAKE3 hash mismatch for entry {}",
                entry.entry_id
            );
        }
    }

    #[test]
    fn test_multi_format_entries() {
        let keypair = test_signing_key();
        let manifest = HexonManifest {
            hexon_type: HexonKind::Material,
            ..test_manifest(&keypair)
        };

        let albedo_data = b"albedo texture bytes";
        let normal_data = b"normal map bytes";
        let roughness_data = b"roughness bytes";

        let entries = vec![
            AssetEntry {
                entry_id: "mat_pbr".to_string(),
                kind: EntryKind::Material,
                asset_hash: crate::signature::asset_hash(albedo_data),
                format: "pbr_bundle".to_string(),
                label: "PBR Material".to_string(),
                tags: vec!["pbr".to_string()],
                description: "A PBR material with sub-assets".to_string(),
                is_placeable: true,
                is_private: false,
                preview_image: None,
                metadata: Some(serde_json::json!({"shader": "standard"})),
                sub_assets: Some(HashMap::from([
                    (
                        "normal".to_string(),
                        crate::signature::asset_hash(normal_data),
                    ),
                    (
                        "roughness".to_string(),
                        crate::signature::asset_hash(roughness_data),
                    ),
                ])),
            },
            AssetEntry {
                entry_id: "tex_normal".to_string(),
                kind: EntryKind::Texture,
                asset_hash: crate::signature::asset_hash(normal_data),
                format: "png".to_string(),
                label: "Normal Map".to_string(),
                tags: vec![],
                description: String::new(),
                is_placeable: false,
                is_private: false,
                preview_image: None,
                metadata: None,
                sub_assets: None,
            },
            AssetEntry {
                entry_id: "tex_roughness".to_string(),
                kind: EntryKind::Texture,
                asset_hash: crate::signature::asset_hash(roughness_data),
                format: "png".to_string(),
                label: "Roughness".to_string(),
                tags: vec![],
                description: String::new(),
                is_placeable: false,
                is_private: false,
                preview_image: None,
                metadata: None,
                sub_assets: None,
            },
        ];

        let license = License {
            license_type: LicenseType::Attribution,
            attribution: Some("CC-BY 4.0".to_string()),
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec!["mat_pbr".to_string()],
            encrypted_key: None,
        };

        let mut assets = HashMap::new();
        assets.insert(
            crate::signature::asset_hash(albedo_data),
            albedo_data.to_vec(),
        );
        assets.insert(
            crate::signature::asset_hash(normal_data),
            normal_data.to_vec(),
        );
        assets.insert(
            crate::signature::asset_hash(roughness_data),
            roughness_data.to_vec(),
        );

        let zip_bytes = HexonPackage::build(
            manifest,
            entries.clone(),
            license.clone(),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();

        // Verify structure
        assert_eq!(package.entries.len(), 3);
        assert_eq!(package.assets.len(), 3);
        assert_eq!(package.license.license_type, LicenseType::Attribution);
        assert_eq!(package.license.free_entries, vec!["mat_pbr"]);

        // Verify sub_assets are preserved
        let mat = package
            .entries
            .iter()
            .find(|e| e.entry_id == "mat_pbr")
            .unwrap();
        let sub = mat.sub_assets.as_ref().unwrap();
        assert!(sub.contains_key("normal"));
        assert!(sub.contains_key("roughness"));

        // Verify all hashes
        for entry in &package.entries {
            let blob = package.assets.get(&entry.asset_hash).unwrap();
            assert_eq!(crate::signature::asset_hash(blob), entry.asset_hash);
        }
    }

    #[test]
    fn test_terrain_and_skybox_entries() {
        let keypair = test_signing_key();

        // Terrain package
        let terrain_manifest = HexonManifest {
            crate_id: "terrain-alps-v1".to_string(),
            hexon_type: HexonKind::Terrain,
            name: "Alps Terrain".to_string(),
            ..test_manifest(&keypair)
        };

        let tile_data = b"fake terrain tile rgb bytes";
        let gpx_data = b"<?xml version='1.0'?><gpx></gpx>";

        let entries = vec![
            AssetEntry {
                entry_id: "tileset_01".to_string(),
                kind: EntryKind::TerrainTileset,
                asset_hash: crate::signature::asset_hash(tile_data),
                format: "terrain_rgb".to_string(),
                label: "Alps Elevation".to_string(),
                tags: vec!["terrain".to_string(), "elevation".to_string()],
                description: "Terrain RGB tileset".to_string(),
                is_placeable: true,
                is_private: false,
                preview_image: None,
                metadata: Some(
                    serde_json::json!({"zoom_min": 5, "zoom_max": 14, "bounds": [5.9, 45.8, 10.5, 47.8]}),
                ),
                sub_assets: None,
            },
            AssetEntry {
                entry_id: "gpx_trail".to_string(),
                kind: EntryKind::GpxFile,
                asset_hash: crate::signature::asset_hash(gpx_data),
                format: "gpx".to_string(),
                label: "Alpine Trail".to_string(),
                tags: vec!["gpx".to_string(), "hiking".to_string()],
                description: "A GPX trail file".to_string(),
                is_placeable: false,
                is_private: false,
                preview_image: None,
                metadata: None,
                sub_assets: None,
            },
        ];

        let license = License {
            license_type: LicenseType::Free,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec![],
            encrypted_key: None,
        };

        let mut assets = HashMap::new();
        assets.insert(crate::signature::asset_hash(tile_data), tile_data.to_vec());
        assets.insert(crate::signature::asset_hash(gpx_data), gpx_data.to_vec());

        let zip_bytes = HexonPackage::build(
            terrain_manifest,
            entries.clone(),
            license,
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert_eq!(package.manifest.hexon_type, HexonKind::Terrain);
        assert_eq!(package.entries.len(), 2);
        assert!(package.signature_valid());

        // Skybox package
        let skybox_manifest = HexonManifest {
            crate_id: "skybox-sunset-v1".to_string(),
            hexon_type: HexonKind::Skybox,
            name: "Sunset Skybox".to_string(),
            ..test_manifest(&keypair)
        };

        let hdr_data = b"fake HDR image data";
        let skybox_entries = vec![AssetEntry {
            entry_id: "sky_hdr".to_string(),
            kind: EntryKind::Skybox,
            asset_hash: crate::signature::asset_hash(hdr_data),
            format: "hdr".to_string(),
            label: "Sunset HDR".to_string(),
            tags: vec!["skybox".to_string()],
            description: "HDR skybox".to_string(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: Some(serde_json::json!({"resolution": [4096, 2048]})),
            sub_assets: None,
        }];

        let mut skybox_assets = HashMap::new();
        skybox_assets.insert(crate::signature::asset_hash(hdr_data), hdr_data.to_vec());

        let license2 = License {
            license_type: LicenseType::Paid,
            attribution: None,
            payment_provider: Some("stripe".to_string()),
            payment_verification_url: Some("https://example.com/verify".to_string()),
            free_entries: vec![],
            encrypted_key: None,
        };

        let zip2 = HexonPackage::build(
            skybox_manifest,
            skybox_entries,
            license2,
            skybox_assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .unwrap();

        let pkg2 = HexonPackage::open(&zip2).unwrap();
        assert_eq!(pkg2.manifest.hexon_type, HexonKind::Skybox);
        assert_eq!(pkg2.license.license_type, LicenseType::Paid);
        assert!(pkg2.license.payment_provider.is_some());
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
        let manifest = test_manifest(&keypair);
        let entries = test_entries();
        let license = test_license();

        let mut assets = HashMap::new();
        assets.insert("abc123".to_string(), b"fake gltf data".to_vec());

        let zip_bytes = HexonPackage::build(
            manifest.clone(),
            entries.clone(),
            license.clone(),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            &keypair,
        )
        .expect("build should succeed");

        // Tamper with the ZIP bytes (corrupt a byte in the middle)
        let mut tampered = zip_bytes.clone();
        if tampered.len() > 100 {
            tampered[50] = tampered[50].wrapping_add(1);
        }

        // Open should fail or signature should be invalid
        let result = HexonPackage::open(&tampered);
        // Either the ZIP is corrupt and open fails, or the signature is invalid
        if let Ok(pkg) = result {
            assert!(
                !pkg.signature_valid(),
                "tampered package should fail signature"
            );
        }
        // If open fails due to corruption, that's also acceptable

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
        let rezip = |bytes: &[u8],
                     edit: &dyn Fn(&str, Vec<u8>) -> Option<Vec<u8>>,
                     extra: Option<(&str, Vec<u8>)>| {
            let mut source = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::FileOptions::default();
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
        let rows: Vec<(&str, Vec<u8>, Option<&str>, &PluginTrust<'_>, Refusal)> = vec![
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

    /// A signed-shape package whose `assets/bomb` inflates past the 64 MiB
    /// per-entry cap from a sub-MiB archive must be refused by `open`.
    #[test]
    fn test_open_refuses_zip_bomb_asset() {
        let keypair = test_signing_key();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let plain = zip::write::FileOptions::default();
        for (name, body) in [
            (
                "manifest.json",
                serde_json::to_vec(&test_manifest(&keypair)).unwrap(),
            ),
            ("entries.json", b"[]".to_vec()),
            ("license.json", serde_json::to_vec(&test_license()).unwrap()),
            ("signature.txt", b"unsigned".to_vec()),
        ] {
            zip.start_file(name, plain).unwrap();
            zip.write_all(&body).unwrap();
        }
        let deflate = zip::write::FileOptions::default()
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
