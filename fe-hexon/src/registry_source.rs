//! `RegistrySource`: one trait for finding and fetching extension packages (Local, static signed index, protocol v1).
//! Rationale and trust model: `docs/extensions/extensions-ui.md` section Registry sources; spec R29/R32.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::package::{verify_plugin_package, PluginPackage, PluginPackageError, PluginTrust};

/// Index document schema this build reads; a newer one is refused.
pub const INDEX_SCHEMA_VERSION: u64 = 1;
/// Protocol version a registry must advertise (`config.json` for a static index, the header or `/health` for v1).
pub const PROTOCOL_V1: &str = "1";
/// Domain line prefixed (with LF) to the canonical index bytes before signing (registry contract v1).
pub const INDEX_SIGNING_DOMAIN: &str = "fractalengine/catalog-index/v1";
/// Package kind the Extensions panel installs; every other kind in an index is skipped.
pub const EXTENSION_KIND: &str = "extension";
/// Largest index document accepted.
pub const INDEX_MAX_BYTES: u64 = 1024 * 1024;
/// Largest `config.json` or package detail document accepted.
pub const DOC_MAX_BYTES: u64 = 256 * 1024;
/// Largest package accepted from any source.
pub const PACKAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest attestation document (manifest plus base64 header members) accepted.
pub const HEADER_MAX_BYTES: u64 = 1024 * 1024;

const CONFIG_PATH: &str = "config.json";
const INDEX_PATH: &str = "index.json";
const DETAIL_DIR: &str = "packages";

/// One installable package a source offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Reverse-DNS package id.
    pub package_id: String,
    /// Package version.
    pub version: String,
    /// Content hash of the sealed manifest (`PluginPackage::content_hash`).
    pub content_hash: String,
    /// Where the sealed bytes live (path or URL, absolute or relative to the index).
    pub url: String,
    /// Sealed package size in bytes.
    pub size: u64,
    /// Display name for Browse; the verified manifest stays authoritative.
    #[serde(default)]
    pub name: String,
    /// Where `plugin/extension.json` can be read before the download, when the registry offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_url: Option<String>,
}

/// A package a listing skipped, with the reason (shown in Browse instead of silently dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// File name or package id.
    pub what: String,
    /// Typed reason text.
    pub reason: String,
}

/// What `list` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Offered packages, in source order.
    pub entries: Vec<RegistryEntry>,
    /// Items the source could not vouch for.
    pub refused: Vec<Refusal>,
    /// True when the last verified cached index was used (registry unreachable, or the live index was older).
    pub from_cache: bool,
    /// True when the index's `expires_at` has passed: shown as stale, never blocks an install.
    pub stale: bool,
}

/// Sealed bytes that passed local verification.
#[derive(Debug, Clone)]
pub struct FetchedPackage {
    /// The sealed `.hexon` bytes, ready for the install path.
    pub bytes: Vec<u8>,
    /// Verification result (identity, content hash, signer).
    pub package: PluginPackage,
}

/// Distinct refusal reasons (the Browse view shows each verbatim; spec R29 AC).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("registry unreachable: {0}")]
    Transport(String),
    #[error("registry speaks protocol {found:?}, this client requires 1")]
    ProtocolMismatch { found: Option<String> },
    #[error("index is malformed: {0}")]
    IndexMalformed(String),
    #[error("index schema {0} is not supported")]
    IndexSchema(u64),
    #[error("index signer '{0}' is not a key pinned for this source")]
    IndexSignerUnknown(String),
    #[error("index key '{0}' also signs packages; an index key is pinned for one source only")]
    IndexKeyMisused(String),
    #[error("index signature does not verify: the index was altered or signed by another key")]
    IndexSignature,
    #[error("package detail does not match the signed index: {0}")]
    DetailMismatch(String),
    #[error("package '{0}' is not offered by this source")]
    NotFound(String),
    #[error("package '{0}' version {1} is yanked")]
    Yanked(String, String),
    #[error("{what} exceeds {max} bytes")]
    TooLarge { what: String, max: u64 },
    #[error("package failed verification: {0}")]
    PackageInvalid(String),
    #[error("package signer is not trusted: {0}")]
    PackageUntrusted(String),
    #[error("package targets an unsupported plugin contract")]
    PackageContract,
    #[error("package is version {served} but the listing said {listed}")]
    VersionMismatch { listed: String, served: String },
    #[error("package content hash {served} differs from the listed {listed}")]
    ContentHashMismatch { listed: String, served: String },
    #[error("this source has no header document; review the grants after download")]
    HeaderUnavailable,
}

/// Find and fetch packages; `fetch` always verifies locally, whatever the source claims.
pub trait RegistrySource: Send + Sync {
    /// Short name shown in Browse, e.g. `Local folder` or the index host.
    fn label(&self) -> String;
    /// Entries, optionally narrowed by a case-insensitive substring of id or name.
    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError>;
    /// The `plugin/extension.json` bytes, so consent can render before the download.
    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError>;
    /// The sealed bytes, verified against the trust roots and the listed hash.
    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError>;
}

/// Who a source trusts; closures keep fe-hexon free of fe-plugin's trust-root file.
#[derive(Clone)]
pub struct SourceTrust {
    /// True when a `did:key` is a configured package trust root.
    pub is_trust_root: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// This source's pinned index keys by key id (built per endpoint; never a package or revocation signer).
    pub index_signer: Arc<dyn Fn(&str) -> Option<VerifyingKey> + Send + Sync>,
    /// Dev escape hatch for unsigned packages; never set in release builds.
    pub allow_unsigned_dev: bool,
}

impl SourceTrust {
    fn plugin_trust(&self) -> PluginTrust<'_> {
        PluginTrust { is_trust_root: &*self.is_trust_root, allow_unsigned_dev: self.allow_unsigned_dev }
    }
}

fn matches_query(entry: &RegistryEntry, query: Option<&str>) -> bool {
    let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else { return true };
    let needle = query.to_lowercase();
    entry.package_id.to_lowercase().contains(&needle) || entry.name.to_lowercase().contains(&needle)
}

/// Verify sealed bytes against an entry: seal and trust, listed version, listed content hash.
pub fn verify_fetched(bytes: Vec<u8>, entry: &RegistryEntry, trust: &SourceTrust) -> Result<FetchedPackage, RegistryError> {
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "package".into(), max: PACKAGE_MAX_BYTES });
    }
    let package = verify_plugin_package(&bytes, Some(&entry.package_id), &trust.plugin_trust()).map_err(|error| match error {
        PluginPackageError::Invalid(reason) => RegistryError::PackageInvalid(reason),
        PluginPackageError::UntrustedSigner(reason) => RegistryError::PackageUntrusted(reason),
        PluginPackageError::UnsupportedContract => RegistryError::PackageContract,
    })?;
    if package.version != entry.version {
        return Err(RegistryError::VersionMismatch { listed: entry.version.clone(), served: package.version });
    }
    if !package.content_hash.eq_ignore_ascii_case(&entry.content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: entry.content_hash.clone(), served: package.content_hash });
    }
    Ok(FetchedPackage { bytes, package })
}

fn entry_for(package: &PluginPackage, url: String, size: u64) -> RegistryEntry {
    let name = serde_json::from_slice::<serde_json::Value>(&package.extension_json)
        .ok()
        .and_then(|doc| doc.get("name").and_then(|v| v.as_str()).map(str::to_owned))
        .unwrap_or_else(|| package.package_id.clone());
    RegistryEntry {
        package_id: package.package_id.clone(),
        version: package.version.clone(),
        content_hash: package.content_hash.clone(),
        url,
        size,
        name,
        header_url: None,
    }
}

// ---- Local ------------------------------------------------------------------------------------

/// A `.hexon` file, or a folder of them.
pub struct LocalSource {
    root: PathBuf,
    trust: SourceTrust,
}

impl LocalSource {
    /// Source over `root` (file or folder).
    pub fn new(root: impl Into<PathBuf>, trust: SourceTrust) -> Self {
        Self { root: root.into(), trust }
    }

    fn files(&self) -> Result<Vec<PathBuf>, RegistryError> {
        if self.root.is_file() {
            return Ok(vec![self.root.clone()]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.root)
            .map_err(|e| RegistryError::Transport(format!("{}: {e}", self.root.display())))?
            .filter_map(|item| item.ok().map(|item| item.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("hexon")))
            .collect();
        files.sort();
        Ok(files)
    }

    fn read(path: &Path) -> Result<Vec<u8>, RegistryError> {
        read_file_capped(path, PACKAGE_MAX_BYTES, "package")
    }
}

fn read_file_capped(path: &Path, max: u64, what: &str) -> Result<Vec<u8>, RegistryError> {
    let meta = std::fs::metadata(path).map_err(|e| RegistryError::Transport(format!("{}: {e}", path.display())))?;
    if meta.len() > max {
        return Err(RegistryError::TooLarge { what: what.into(), max });
    }
    std::fs::read(path).map_err(|e| RegistryError::Transport(format!("{}: {e}", path.display())))
}

impl RegistrySource for LocalSource {
    fn label(&self) -> String {
        format!("Local: {}", self.root.display())
    }

    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        let mut listing = Listing::default();
        for path in self.files()? {
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let outcome = Self::read(&path).and_then(|bytes| {
                let size = bytes.len() as u64;
                verify_plugin_package(&bytes, None, &self.trust.plugin_trust())
                    .map(|package| entry_for(&package, path.to_string_lossy().into_owned(), size))
                    .map_err(|error| match error {
                        PluginPackageError::Invalid(reason) => RegistryError::PackageInvalid(reason),
                        PluginPackageError::UntrustedSigner(reason) => RegistryError::PackageUntrusted(reason),
                        PluginPackageError::UnsupportedContract => RegistryError::PackageContract,
                    })
            });
            match outcome {
                Ok(entry) if matches_query(&entry, query) => listing.entries.push(entry),
                Ok(_) => {}
                Err(error) => listing.refused.push(Refusal { what: file_name, reason: error.to_string() }),
            }
        }
        Ok(listing)
    }

    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        Ok(self.fetch(entry)?.package.extension_json)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        let path = PathBuf::from(&entry.url);
        let climbs = path.components().any(|part| matches!(part, std::path::Component::ParentDir));
        let inside = !climbs && (path == self.root || path.starts_with(&self.root));
        if !inside {
            return Err(RegistryError::NotFound(entry.package_id.clone()));
        }
        verify_fetched(Self::read(&path)?, entry, &self.trust)
    }
}

// ---- Static signed index (registry contract v1) -------------------------------------------------

/// One `index.json` entry (registry contract v1 `IndexEntry`); other fields are signed but not read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IndexEntry {
    /// Package id (`^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`).
    pub package_id: String,
    /// `extension`, `web-bundle`, or a kind this build skips.
    pub kind: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Newest offered version (SemVer without build metadata).
    pub latest_version: String,
    /// Content hash of that version's sealed build.
    pub latest_content_hash: String,
    /// BLAKE3 hex of `packages/{package_id}.json` exactly as served.
    pub detail_hash: String,
}

/// An index whose signature, schema and entries passed verification.
#[derive(Debug, Clone)]
pub struct VerifiedIndex {
    /// Signed issue time; an index older than the cached one is ignored (rollback guard).
    pub issued_at: DateTime<Utc>,
    /// Signed expiry, if any.
    pub expires_at: Option<DateTime<Utc>>,
    /// Extension entries with a valid id, version and digests, in index order.
    pub entries: Vec<IndexEntry>,
    /// Entries skipped for a malformed shape, id, version or a case-only id collision.
    pub refused: Vec<Refusal>,
}

impl VerifiedIndex {
    /// True once `expires_at` has passed.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expiry| expiry <= now)
    }
}

/// Bytes an index signature covers: the domain line, then canonical JSON of the document without `signature`.
pub fn index_signed_bytes(doc: &Value) -> Result<Vec<u8>, RegistryError> {
    let mut object = doc.as_object().cloned().ok_or_else(|| RegistryError::IndexMalformed("not a JSON object".into()))?;
    object.remove("signature");
    let unsigned = Value::Object(object);
    if !integers_only(&unsigned) {
        return Err(RegistryError::IndexMalformed("signed documents carry integers only".into()));
    }
    let mut out = format!("{INDEX_SIGNING_DOMAIN}\n");
    write_canonical(&unsigned, &mut out);
    Ok(out.into_bytes())
}

/// Sign an index document as `key_id` (publisher tooling and tests); the signature is hex ed25519.
pub fn sign_index(mut doc: Value, key_id: &str, key: &SigningKey) -> Result<Value, RegistryError> {
    let Some(object) = doc.as_object_mut() else {
        return Err(RegistryError::IndexMalformed("not a JSON object".into()));
    };
    object.insert("signer_key_id".into(), Value::String(key_id.to_owned()));
    let signature: Signature = key.sign(&index_signed_bytes(&doc)?);
    if let Some(object) = doc.as_object_mut() {
        object.insert("signature".into(), Value::String(encode_hex(&signature.to_bytes())));
    }
    Ok(doc)
}

/// Verify index bytes: size, schema, pinned signer, signature over the generic document, then each entry.
pub fn verify_index(bytes: &[u8], trust: &SourceTrust) -> Result<VerifiedIndex, RegistryError> {
    if bytes.len() as u64 > INDEX_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "index".into(), max: INDEX_MAX_BYTES });
    }
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| RegistryError::IndexMalformed(e.to_string()))?;
    let schema = doc
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| RegistryError::IndexMalformed("missing schema_version".into()))?;
    if schema != INDEX_SCHEMA_VERSION {
        return Err(RegistryError::IndexSchema(schema));
    }
    let signer = doc
        .get("signer_key_id")
        .and_then(Value::as_str)
        .ok_or_else(|| RegistryError::IndexMalformed("missing signer_key_id".into()))?;
    let key = (trust.index_signer)(signer).ok_or_else(|| RegistryError::IndexSignerUnknown(signer.to_owned()))?;
    if (trust.is_trust_root)(&fe_format::verifying_key_to_did(&key)) {
        return Err(RegistryError::IndexKeyMisused(signer.to_owned()));
    }
    let signature = doc
        .get("signature")
        .and_then(Value::as_str)
        .and_then(decode_signature)
        .ok_or(RegistryError::IndexSignature)?;
    key.verify(&index_signed_bytes(&doc)?, &signature).map_err(|_| RegistryError::IndexSignature)?;

    let time = |field: &str| -> Result<Option<DateTime<Utc>>, RegistryError> {
        match doc.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_str()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|at| Some(at.with_timezone(&Utc)))
                .ok_or_else(|| RegistryError::IndexMalformed(format!("{field} is not RFC 3339"))),
        }
    };
    let issued_at = time("issued_at")?.ok_or_else(|| RegistryError::IndexMalformed("missing issued_at".into()))?;
    let expires_at = time("expires_at")?;
    let raw_entries = doc
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| RegistryError::IndexMalformed("missing entries".into()))?;

    let mut index = VerifiedIndex { issued_at, expires_at, entries: Vec::new(), refused: Vec::new() };
    // One package-id namespace across kinds: collisions are checked before kinds are filtered.
    let mut seen_ids = HashSet::new();
    for raw in raw_entries {
        let what = raw.get("package_id").and_then(Value::as_str).unwrap_or("index entry").to_owned();
        let entry: IndexEntry = match serde_json::from_value(raw.clone()) {
            Ok(entry) => entry,
            Err(error) => {
                index.refused.push(Refusal { what, reason: format!("malformed index entry: {error}") });
                continue;
            }
        };
        let refusal = if !valid_package_id(&entry.package_id) {
            Some("package id does not match the id grammar")
        } else if !seen_ids.insert(entry.package_id.to_ascii_lowercase()) {
            Some("package id collides with an earlier entry")
        } else if entry.kind != EXTENSION_KIND {
            continue;
        } else if !valid_version(&entry.latest_version) {
            Some("latest_version is not SemVer without build metadata")
        } else if !is_digest_hex(&entry.latest_content_hash) || !is_digest_hex(&entry.detail_hash) {
            Some("content or detail hash is not BLAKE3 hex")
        } else {
            None
        };
        match refusal {
            Some(reason) => index.refused.push(Refusal { what, reason: reason.into() }),
            None => index.entries.push(entry),
        }
    }
    Ok(index)
}

fn integers_only(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.is_i64() || number.is_u64(),
        Value::Array(items) => items.iter().all(integers_only),
        Value::Object(map) => map.values().all(integers_only),
        _ => true,
    }
}

/// Package id grammar of registry contract v1: `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`.
pub fn valid_package_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        && !id.to_ascii_lowercase().ends_with(".json")
}

fn semver_number(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()) && (part == "0" || !part.starts_with('0'))
}

fn semver_prerelease_part(part: &str) -> bool {
    !part.is_empty()
        && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && (!part.bytes().all(|byte| byte.is_ascii_digit()) || semver_number(part))
}

/// SemVer 2.0 without build metadata (`+...` is refused).
pub fn valid_version(version: &str) -> bool {
    let (core, prerelease) = match version.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (version, None),
    };
    core.split('.').count() == 3
        && core.split('.').all(semver_number)
        && prerelease.is_none_or(|prerelease| prerelease.split('.').all(semver_prerelease_part))
}

fn is_digest_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (position, key) in keys.into_iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok()).collect()
}

/// Hex (the revocation list's encoding) or standard base64.
fn decode_signature(text: &str) -> Option<Signature> {
    let raw = decode_hex(text).or_else(|| base64::engine::general_purpose::STANDARD.decode(text).ok())?;
    Signature::from_slice(&raw).ok()
}

/// Reads bytes from a location; the binary supplies an http(s) implementation, [`FileFetcher`] serves paths.
/// Implementations are anonymous: no credential is ever sent to an endpoint or to a url a document names.
pub trait BytesFetcher: Send + Sync {
    /// Read at most `max_bytes` from `location`.
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError>;
}

/// [`BytesFetcher`] over the local filesystem (also how loopback fixtures are served in tests).
pub struct FileFetcher;

impl BytesFetcher for FileFetcher {
    fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
        read_file_capped(Path::new(location), max_bytes, "document")
    }
}

/// `packages/{id}.json`: every offered version of one package, pinned by the index's `detail_hash`.
#[derive(Debug, Clone, Deserialize)]
struct PackageDetail {
    package_id: String,
    kind: String,
    #[serde(default)]
    versions: Vec<DetailVersion>,
}

#[derive(Debug, Clone, Deserialize)]
struct DetailVersion {
    version: String,
    content_hash: String,
    size_bytes: u64,
    download_url: String,
    #[serde(default)]
    yanked: bool,
}

/// `packages/{id}/{version}/attestation.json`: the signed manifest and header members, for consent before download.
#[derive(Debug, Clone, Deserialize)]
struct Attestation {
    package_id: String,
    version: String,
    content_hash: String,
    manifest_b64: String,
    #[serde(default)]
    header_members_b64: BTreeMap<String, String>,
}

/// `plugin/extension.json` from an attestation, bound to `content_hash` by the manifest signature and header digests.
fn verified_header(attestation: &Attestation, content_hash: &str) -> Result<Vec<u8>, RegistryError> {
    let engine = base64::engine::general_purpose::STANDARD;
    let invalid = |reason: String| RegistryError::PackageInvalid(format!("attestation: {reason}"));
    let manifest = engine.decode(&attestation.manifest_b64).map_err(|e| invalid(e.to_string()))?;
    let mut members = BTreeMap::new();
    for (path, encoded) in &attestation.header_members_b64 {
        members.insert(path.clone(), engine.decode(encoded).map_err(|e| invalid(format!("{path}: {e}")))?);
    }
    fe_format::verify_sealed_header(&manifest, &members, &fe_format::IntegrityPolicy::RequireSealed { pin: None })
        .map_err(|e| invalid(e.to_string()))?;
    let parsed: fe_format::HexonManifest = serde_json::from_slice(&manifest).map_err(|e| invalid(e.to_string()))?;
    if parsed.hexon_id != attestation.package_id || parsed.version != attestation.version {
        return Err(invalid("manifest names another package".into()));
    }
    let committed = parsed
        .integrity
        .as_ref()
        .is_some_and(|root| root.header.contains_key(fe_format::integrity::PLUGIN_EXTENSION_PATH));
    if !committed {
        return Err(invalid("plugin/extension.json is not a committed header member".into()));
    }
    let served = fe_format::integrity::content_hash(&manifest).map_err(|e| invalid(format!("{e:#}")))?;
    if !served.eq_ignore_ascii_case(content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: content_hash.to_owned(), served });
    }
    members.remove(fe_format::integrity::PLUGIN_EXTENSION_PATH).ok_or(RegistryError::HeaderUnavailable)
}

fn is_separator(character: char) -> bool {
    character == '/' || character == '\\'
}

/// A static signed index endpoint (`config.json`, `index.json`, `packages/...`) with an offline cache.
pub struct StaticIndexSource {
    base: String,
    fetcher: Arc<dyn BytesFetcher>,
    trust: SourceTrust,
    cache_path: Option<PathBuf>,
}

impl StaticIndexSource {
    /// Source over the endpoint `base` (a folder, or the http(s) prefix that serves `config.json`), read through `fetcher`.
    pub fn new(base: impl Into<String>, fetcher: Arc<dyn BytesFetcher>, trust: SourceTrust) -> Self {
        Self { base: base.into(), fetcher, trust, cache_path: None }
    }

    /// Keep the last verified index at `path` and fall back to it when the endpoint is unreachable.
    pub fn with_cache(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = Some(path.into());
        self
    }

    /// `base/location`; a relative location that climbs is refused so a document cannot point outside the endpoint.
    fn resolve(&self, location: &str) -> Result<String, RegistryError> {
        if location.contains("://") || Path::new(location).is_absolute() {
            return Ok(location.to_owned());
        }
        if location.split(is_separator).any(|part| part == "..") {
            return Err(RegistryError::DetailMismatch(format!("location {location:?} climbs out of the source")));
        }
        Ok(format!("{}/{}", self.base.trim_end_matches(is_separator), location.trim_start_matches(is_separator)))
    }

    /// `config.json` must advertise `registry_protocol: 1`; its unsigned `index_key_id` never widens trust.
    fn check_protocol(&self) -> Result<(), RegistryError> {
        let bytes = self.fetcher.get(&self.resolve(CONFIG_PATH)?, DOC_MAX_BYTES)?;
        let config: Value =
            serde_json::from_slice(&bytes).map_err(|e| RegistryError::IndexMalformed(format!("{CONFIG_PATH}: {e}")))?;
        let found = config.get("registry_protocol").map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
        if found.as_deref() == Some(PROTOCOL_V1) {
            Ok(())
        } else {
            Err(RegistryError::ProtocolMismatch { found })
        }
    }

    fn cached_index(&self) -> Option<Result<VerifiedIndex, RegistryError>> {
        let bytes = std::fs::read(self.cache_path.as_ref()?).ok()?;
        Some(verify_index(&bytes, &self.trust))
    }

    /// The live index; the cache when the endpoint is unreachable or the live index is older (rollback).
    fn load_index(&self) -> Result<(VerifiedIndex, bool), RegistryError> {
        let live = self
            .check_protocol()
            .and_then(|()| self.resolve(INDEX_PATH))
            .and_then(|location| self.fetcher.get(&location, INDEX_MAX_BYTES));
        match live {
            Ok(bytes) => {
                let index = verify_index(&bytes, &self.trust)?;
                if let Some(Ok(cached)) = self.cached_index() {
                    if index.issued_at < cached.issued_at {
                        tracing::warn!(base = %self.base, "registry index older than the cached copy ignored");
                        return Ok((cached, true));
                    }
                }
                if let Some(cache) = &self.cache_path {
                    if let Err(error) = std::fs::write(cache, &bytes) {
                        tracing::warn!(%error, "registry index cache not written");
                    }
                }
                Ok((index, false))
            }
            // The cache is re-verified: a tampered cache file is refused like a tampered fetch.
            Err(RegistryError::Transport(reason)) => match self.cached_index() {
                Some(cached) => Ok((cached?, true)),
                None => Err(RegistryError::Transport(reason)),
            },
            Err(other) => Err(other),
        }
    }

    /// The version `entry` names, as the signed index and its hash-pinned detail document vouch for it.
    fn vouched_version(&self, entry: &RegistryEntry) -> Result<DetailVersion, RegistryError> {
        let (index, _) = self.load_index()?;
        let listed = index
            .entries
            .iter()
            .find(|listed| listed.package_id == entry.package_id)
            .ok_or_else(|| RegistryError::NotFound(entry.package_id.clone()))?;
        let bytes = self.fetcher.get(&self.resolve(&format!("{DETAIL_DIR}/{}.json", listed.package_id))?, DOC_MAX_BYTES)?;
        if !fe_format::integrity::digest_hex(&bytes).eq_ignore_ascii_case(&listed.detail_hash) {
            return Err(RegistryError::DetailMismatch("detail document hash differs from the index".into()));
        }
        let detail: PackageDetail = serde_json::from_slice(&bytes).map_err(|e| RegistryError::DetailMismatch(e.to_string()))?;
        if detail.package_id != listed.package_id || detail.kind != listed.kind {
            return Err(RegistryError::DetailMismatch("package id or kind differs from the index".into()));
        }
        let version = detail
            .versions
            .into_iter()
            .find(|version| version.version == entry.version && version.content_hash.eq_ignore_ascii_case(&entry.content_hash))
            .ok_or_else(|| RegistryError::NotFound(format!("{}@{}", entry.package_id, entry.version)))?;
        if version.version == listed.latest_version && !version.content_hash.eq_ignore_ascii_case(&listed.latest_content_hash) {
            return Err(RegistryError::DetailMismatch("latest version hash differs from the index".into()));
        }
        if version.yanked {
            return Err(RegistryError::Yanked(entry.package_id.clone(), version.version));
        }
        Ok(version)
    }
}

impl RegistrySource for StaticIndexSource {
    fn label(&self) -> String {
        format!("Index: {}", self.base)
    }

    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        let (index, from_cache) = self.load_index()?;
        let stale = index.is_stale(Utc::now());
        let entries = index
            .entries
            .iter()
            .map(|listed| RegistryEntry {
                package_id: listed.package_id.clone(),
                version: listed.latest_version.clone(),
                content_hash: listed.latest_content_hash.clone(),
                url: format!("{DETAIL_DIR}/{}.json", listed.package_id),
                size: 0,
                name: listed.name.clone(),
                header_url: None,
            })
            .filter(|entry| matches_query(entry, query))
            .collect();
        Ok(Listing { entries, refused: index.refused, from_cache, stale })
    }

    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        let version = self.vouched_version(entry)?;
        let location = self.resolve(&format!("{DETAIL_DIR}/{}/{}/attestation.json", entry.package_id, version.version))?;
        let bytes = match self.fetcher.get(&location, HEADER_MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(RegistryError::Transport(_)) => return Err(RegistryError::HeaderUnavailable),
            Err(other) => return Err(other),
        };
        let attestation: Attestation =
            serde_json::from_slice(&bytes).map_err(|e| RegistryError::PackageInvalid(format!("attestation: {e}")))?;
        if attestation.package_id != entry.package_id
            || attestation.version != version.version
            || !attestation.content_hash.eq_ignore_ascii_case(&version.content_hash)
        {
            return Err(RegistryError::PackageInvalid("attestation: names another build".into()));
        }
        verified_header(&attestation, &version.content_hash)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        // Only what the signed index and its pinned detail document vouch for is fetched, never a caller-supplied url.
        let version = self.vouched_version(entry)?;
        if version.size_bytes > PACKAGE_MAX_BYTES {
            return Err(RegistryError::TooLarge { what: "package".into(), max: PACKAGE_MAX_BYTES });
        }
        let bytes = self.fetcher.get(&self.resolve(&version.download_url)?, version.size_bytes)?;
        let vouched = RegistryEntry {
            version: version.version,
            content_hash: version.content_hash,
            url: version.download_url,
            size: version.size_bytes,
            ..entry.clone()
        };
        verify_fetched(bytes, &vouched, &self.trust)
    }
}

// ---- Protocol v1 ------------------------------------------------------------------------------

/// Transport over a protocol-v1 registry; the binary adapts `fe-plugin`'s `RegistryClient` and `RemoteRegistryClient`.
pub trait ProtocolV1Transport: Send + Sync {
    /// Value of the `X-Hexon-Registry-Protocol` header from a health probe, `None` when absent.
    fn protocol(&self) -> Result<Option<String>, String>;
    /// Search results mapped to entries (`url` carries the registry uri used by `download`).
    fn search(&self, query: Option<&str>) -> Result<Vec<RegistryEntry>, String>;
    /// Download sealed bytes for an entry.
    fn download(&self, entry: &RegistryEntry) -> Result<Vec<u8>, String>;
}

/// A protocol-v1 registry endpoint the operator configured.
pub struct ProtocolV1Source {
    label: String,
    transport: Arc<dyn ProtocolV1Transport>,
    trust: SourceTrust,
}

impl ProtocolV1Source {
    /// Source over `transport`, labelled for Browse.
    pub fn new(label: impl Into<String>, transport: Arc<dyn ProtocolV1Transport>, trust: SourceTrust) -> Self {
        Self { label: label.into(), transport, trust }
    }

    /// The protocol check runs before every operation, as the registry client does.
    fn check_protocol(&self) -> Result<(), RegistryError> {
        let found = self.transport.protocol().map_err(RegistryError::Transport)?;
        if found.as_deref() == Some(PROTOCOL_V1) {
            Ok(())
        } else {
            Err(RegistryError::ProtocolMismatch { found })
        }
    }
}

impl RegistrySource for ProtocolV1Source {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        self.check_protocol()?;
        let entries = self.transport.search(query).map_err(RegistryError::Transport)?;
        Ok(Listing { entries, ..Listing::default() })
    }

    fn header(&self, _entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        self.check_protocol()?;
        Err(RegistryError::HeaderUnavailable)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        self.check_protocol()?;
        let bytes = self.transport.download(entry).map_err(RegistryError::Transport)?;
        verify_fetched(bytes, entry, &self.trust)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use base64::Engine as _;

    use super::*;
    use crate::publisher::PluginHexonBuilder;

    const WASM_COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
    const PACKAGE_ID: &str = "org.example.fleet-sim";

    fn did_of(key: &SigningKey) -> String {
        fe_format::verifying_key_to_did(&key.verifying_key())
    }

    fn sealed_package(id: &str, version: &str, key: &SigningKey) -> Vec<u8> {
        let extension = serde_json::json!({
            "id": id, "version": version, "name": "Fleet Sim",
            "contract": "fractalengine:plugin@2.0.0", "kind": "simulation",
        });
        let mut component = WASM_COMPONENT_PREAMBLE.to_vec();
        component.extend_from_slice(&[7u8; 64]);
        PluginHexonBuilder::new(serde_json::to_vec(&extension).unwrap(), component).build(key).expect("seal")
    }

    /// The manifest and committed header members of a sealed package, as an attestation document carries them.
    fn attestation(sealed: &[u8], package: &PluginPackage) -> Value {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(sealed)).expect("zip");
        let mut read = |name: &str| {
            let mut out = Vec::new();
            archive.by_name(name).expect("member").read_to_end(&mut out).expect("read member");
            out
        };
        let manifest = read("manifest.json");
        let parsed: fe_format::HexonManifest = serde_json::from_slice(&manifest).unwrap();
        let members: serde_json::Map<String, Value> = parsed
            .integrity
            .expect("sealed")
            .header
            .keys()
            .map(|path| (path.clone(), Value::String(engine.encode(read(path.as_str())))))
            .collect();
        serde_json::json!({
            "package_id": package.package_id, "version": package.version, "content_hash": package.content_hash,
            "manifest_b64": engine.encode(&manifest), "header_members_b64": members, "signatures": [],
        })
    }

    fn put(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    struct FakeV1 {
        protocol: Option<String>,
        bytes: Vec<u8>,
    }

    impl ProtocolV1Transport for FakeV1 {
        fn protocol(&self) -> Result<Option<String>, String> {
            Ok(self.protocol.clone())
        }
        fn search(&self, _query: Option<&str>) -> Result<Vec<RegistryEntry>, String> {
            Ok(Vec::new())
        }
        fn download(&self, _entry: &RegistryEntry) -> Result<Vec<u8>, String> {
            Ok(self.bytes.clone())
        }
    }

    /// Browse journey over a static signed index endpoint: list, preview the header, fetch; offline cache, rollback,
    /// tampered index, tampered detail, misused index key, bad package signature and protocol mismatch each refuse distinctly.
    #[test]
    fn registry_source_journey() {
        let publisher = SigningKey::from_bytes(&[11u8; 32]);
        let index_key = SigningKey::from_bytes(&[12u8; 32]);
        let publisher_did = did_of(&publisher);
        let index_verifying = index_key.verifying_key();
        let trust = SourceTrust {
            is_trust_root: Arc::new(move |did| did == publisher_did),
            index_signer: Arc::new(move |id| (id == "index-root").then_some(index_verifying)),
            allow_unsigned_dev: false,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let base = root.to_string_lossy().into_owned();

        // The endpoint: config, sealed build, hash-pinned detail document, attestation, signed index.
        let good = sealed_package(PACKAGE_ID, "1.0.0", &publisher);
        let facts = verify_plugin_package(&good, None, &trust.plugin_trust()).expect("fixture verifies");
        let config = |protocol: u64| {
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "registry_protocol": protocol, "kinds": ["extension", "web-bundle"],
                "features": [], "index_key_id": "index-root",
            }))
            .unwrap()
        };
        put(root, "config.json", &config(1));
        put(root, "fleet-sim-1.0.0.hexon", &good);
        let detail = serde_json::to_vec(&serde_json::json!({
            "package_id": PACKAGE_ID, "kind": "extension", "access": "free",
            "versions": [{"version": "1.0.0", "content_hash": facts.content_hash, "size_bytes": good.len(),
                          "download_url": "fleet-sim-1.0.0.hexon", "yanked": false}],
        }))
        .unwrap();
        let detail_path = format!("packages/{PACKAGE_ID}.json");
        put(root, &detail_path, &detail);
        let attested = serde_json::to_vec(&attestation(&good, &facts)).unwrap();
        put(root, &format!("packages/{PACKAGE_ID}/1.0.0/attestation.json"), &attested);
        let detail_hash = fe_format::integrity::digest_hex(&detail);
        let index_doc = |issued_at: &str| {
            serde_json::json!({
                "schema_version": 1, "issued_at": issued_at, "expires_at": "2026-11-08T00:00:01Z",
                "entries": [{"package_id": PACKAGE_ID, "kind": "extension", "name": "Fleet Sim", "latest_version": "1.0.0",
                             "latest_content_hash": facts.content_hash, "detail_hash": detail_hash, "access": "free"}],
            })
        };
        let signed = |doc: Value, key: &SigningKey| serde_json::to_vec(&sign_index(doc, "index-root", key).unwrap()).unwrap();
        put(root, "index.json", &signed(index_doc("2026-11-08T00:00:00Z"), &index_key));

        // Good index: listed (stale once expires_at passed, still installable), previewed before download, fetched.
        let cache_path = root.join("index.cache.json");
        let source = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), trust.clone()).with_cache(&cache_path);
        let listing = source.list(None).expect("good index lists");
        assert_eq!((listing.entries.len(), listing.from_cache, listing.stale), (1, false, true));
        assert!(source.list(Some("nomatch")).unwrap().entries.is_empty());
        let entry = listing.entries[0].clone();
        assert_eq!(source.header(&entry).expect("header before download"), facts.extension_json);
        assert_eq!(source.fetch(&entry).expect("good package fetches").package.package_id, PACKAGE_ID);
        let offline = StaticIndexSource::new(root.join("gone").to_string_lossy(), Arc::new(FileFetcher), trust.clone())
            .with_cache(&cache_path);
        assert!(offline.list(None).expect("offline falls back to the verified cache").from_cache);
        let local = LocalSource::new(root, trust.clone());
        assert_eq!(local.list(None).unwrap().entries[0].content_hash, facts.content_hash);

        // Rollback: an older, validly signed index is ignored in favour of the newer cached copy.
        put(root, "index.json", &signed(index_doc("2026-11-01T00:00:00Z"), &index_key));
        assert!(source.list(None).unwrap().from_cache, "older index ignored");

        // Tampered index: an entry altered after signing.
        let mut tampered = sign_index(index_doc("2026-11-09T00:00:00Z"), "index-root", &index_key).unwrap();
        tampered["entries"][0]["latest_version"] = Value::String("1.0.1".into());
        put(root, "index.json", &serde_json::to_vec(&tampered).unwrap());
        assert_eq!(source.list(None).unwrap_err(), RegistryError::IndexSignature);

        // Tampered detail document: the signed index pins its hash.
        put(root, "index.json", &signed(index_doc("2026-11-09T00:00:00Z"), &index_key));
        put(root, &detail_path, &[detail.as_slice(), b" ".as_slice()].concat());
        assert!(matches!(source.fetch(&entry), Err(RegistryError::DetailMismatch(_))));
        put(root, &detail_path, &detail);

        // An index key that also signs packages is refused, even when pinned.
        let publisher_verifying = publisher.verifying_key();
        let misused = SourceTrust { index_signer: Arc::new(move |id| (id == "index-root").then_some(publisher_verifying)), ..trust.clone() };
        put(root, "index.json", &signed(index_doc("2026-11-10T00:00:00Z"), &publisher));
        let misused_source = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), misused);
        assert_eq!(misused_source.list(None).unwrap_err(), RegistryError::IndexKeyMisused("index-root".into()));
        put(root, "index.json", &signed(index_doc("2026-11-10T00:00:00Z"), &index_key));

        // Bad package signature: a byte flipped inside the sealed build the index vouches for.
        let mut corrupt = good.clone();
        let middle = corrupt.len() / 2;
        corrupt[middle] ^= 0xff;
        put(root, "fleet-sim-1.0.0.hexon", &corrupt);
        assert!(matches!(source.fetch(&entry), Err(RegistryError::PackageInvalid(_))));

        // Protocol mismatch: refused before the index is read, by the static endpoint and by a protocol-v1 registry.
        put(root, "config.json", &config(2));
        assert_eq!(source.list(None).unwrap_err(), RegistryError::ProtocolMismatch { found: Some("2".into()) });
        let mismatched = ProtocolV1Source::new("v2 registry", Arc::new(FakeV1 { protocol: Some("2".into()), bytes: good.clone() }), trust.clone());
        assert_eq!(mismatched.list(None).unwrap_err(), RegistryError::ProtocolMismatch { found: Some("2".into()) });
        let matched = ProtocolV1Source::new("v1 registry", Arc::new(FakeV1 { protocol: Some("1".into()), bytes: good }), trust);
        let v1_entry = entry_for(&facts, format!("{PACKAGE_ID}@1.0.0"), 0);
        assert_eq!(matched.fetch(&v1_entry).unwrap().package.version, "1.0.0");
    }
}
