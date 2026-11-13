//! `RegistrySource`: one trait for finding and fetching extension packages (Local, store-client contract v1 catalogue,
//! protocol v1). Rationale and trust model: `docs/spec/store-client/contract-v1.md`, `docs/extensions/extensions-ui.md`
//! section Registry sources; spec R29/R32.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::package::{verify_plugin_package, PluginPackage, PluginPackageError, PluginTrust};

/// Index document schema this build reads; a newer one is refused.
pub const INDEX_SCHEMA_VERSION: u64 = 1;
/// Newest `config.json` schema this build reads.
pub const CONFIG_SCHEMA_VERSION: u64 = 1;
/// Newest package detail schema this build reads.
pub const DETAIL_SCHEMA_VERSION: u64 = 1;
/// Newest attestation schema this build reads.
pub const ATTESTATION_SCHEMA_VERSION: u64 = 1;
/// Protocol version a registry must advertise (`config.json` `registry_protocol`; the header or `/health` for v1).
pub const PROTOCOL_V1: &str = "1";
/// Domain line prefixed (with LF) to the canonical index bytes before signing (registry contract v1).
pub const INDEX_SIGNING_DOMAIN: &str = "fractalengine/catalog-index/v1";
/// Store family of every code-bearing package that is not a web bundle.
pub const EXTENSION_KIND: &str = "extension";
/// Store family of a package whose manifest v2 `kind` is `web-bundle`.
pub const WEB_BUNDLE_KIND: &str = "web-bundle";
/// Store families this build reads; index entries of any other kind are skipped.
pub const KNOWN_KINDS: [&str; 2] = [EXTENSION_KIND, WEB_BUNDLE_KIND];
/// `access` of a package anyone may install.
pub const ACCESS_FREE: &str = "free";
/// `access` of a package whose non-free entries need an entitlement.
pub const ACCESS_LICENSED: &str = "licensed";
/// Optional feature: hosted search.
pub const FEATURE_SEARCH: &str = "search";
/// Optional feature: kill list at `revocations.json`.
pub const FEATURE_REVOCATIONS: &str = "revocations";
/// Optional feature: the entitlement token endpoint (and `acquire_url`).
pub const FEATURE_ENTITLEMENTS: &str = "entitlements";
/// Interface id an extension version `requires`.
pub const PLUGIN_INTERFACE: &str = "fractalengine:plugin";
/// Interface id a web-bundle version `requires`.
pub const PORTAL_INTERFACE: &str = "fractalengine:portal-bundle";
/// Path prefix of every catalogue document under the endpoint base.
pub const CATALOG_PREFIX: &str = "api/v1/catalog";
/// Largest index document accepted.
pub const INDEX_MAX_BYTES: u64 = 1024 * 1024;
/// Most entries one index may carry.
pub const INDEX_MAX_ENTRIES: usize = 10_000;
/// Largest `config.json` or package detail document accepted.
pub const DOC_MAX_BYTES: u64 = 256 * 1024;
/// Largest package accepted from any source.
pub const PACKAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest attestation document (manifest plus base64 header members) accepted.
pub const HEADER_MAX_BYTES: u64 = 1024 * 1024;
/// Largest hosted search page accepted.
pub const SEARCH_MAX_BYTES: u64 = 1024 * 1024;
/// Longest search parameter accepted, in characters.
pub const SEARCH_PARAM_MAX_CHARS: usize = 256;
/// Most further signatures one attestation may carry.
pub const ATTESTATION_MAX_SIGNATURES: usize = 16;

const CONFIG_PATH: &str = "config.json";
const INDEX_PATH: &str = "index.json";
const SEARCH_PATH: &str = "search";
const DETAIL_DIR: &str = "packages";
const LICENSE_PATH: &str = "license.json";
/// Interface versions this host implements (`requires` filters versions against these).
const HOST_PLUGIN_INTERFACE: (u64, u64, u64) = (2, 0, 0);
const HOST_PORTAL_INTERFACE: (u64, u64, u64) = (1, 0, 0);

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
    /// Items the source could not vouch for, or that a kill list revokes (shown unavailable).
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
    /// Verification result (identity, content hash, signer); a web bundle has no component.
    pub package: PluginPackage,
}

impl FetchedPackage {
    /// Store family read from the verified `plugin/extension.json`.
    pub fn kind(&self) -> &'static str {
        store_family(&self.package.extension_json)
    }
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
    #[error("{document} schema {found} is newer than this client reads")]
    SchemaTooNew { document: String, found: u64 },
    #[error("index signer '{0}' is not a key pinned for this source")]
    IndexSignerUnknown(String),
    #[error("index key '{0}' also signs packages; an index key is pinned for one source only")]
    IndexKeyMisused(String),
    #[error("index signature does not verify: the index was altered or signed by another key")]
    IndexSignature,
    #[error("index issued {served} is older than the verified {known}; refusing a rollback")]
    IndexRollback { known: String, served: String },
    #[error("package detail does not match the signed index: {0}")]
    DetailMismatch(String),
    #[error("catalogue location refused: {0}")]
    LocationRefused(String),
    #[error("search query refused: {0}")]
    InvalidQuery(String),
    #[error("package '{0}' is not offered by this source")]
    NotFound(String),
    #[error("package '{0}' version {1} is yanked")]
    Yanked(String, String),
    #[error("package '{package_id}' is not installable from this listing (access {access:?})")]
    AccessUnknown { package_id: String, access: String },
    #[error("package '{package_id}' build {content_hash} is revoked by a kill list")]
    Revoked { package_id: String, content_hash: String },
    #[error("{what} exceeds {max} bytes")]
    TooLarge { what: String, max: u64 },
    #[error("package failed verification: {0}")]
    PackageInvalid(String),
    #[error("package signer is not trusted: {0}")]
    PackageUntrusted(String),
    #[error("package targets an unsupported plugin contract")]
    PackageContract,
    #[error("package publisher {served} differs from the listed {listed}")]
    PublisherMismatch { listed: String, served: String },
    #[error("package is a {served} but the listing says {listed}")]
    KindMismatch { listed: String, served: String },
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

/// True when a `did:key` is a configured package trust root.
pub type KeyPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Pinned index key by key id.
pub type KeyLookup = Arc<dyn Fn(&str) -> Option<VerifyingKey> + Send + Sync>;
/// True when a verified kill list revokes `(package_id, hash)`; the host matches by hash under any id.
pub type KillPredicate = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// A kill-list predicate that revokes nothing (tools and tests; the binary injects the real list).
pub fn no_kill_list() -> KillPredicate {
    Arc::new(|_, _| false)
}

/// Index-key lookup over an endpoint's pinned `(key_id, key)` set (current and next key during a rotation).
pub fn index_pin(pins: Vec<(String, VerifyingKey)>) -> KeyLookup {
    Arc::new(move |key_id| pins.iter().find(|(id, _)| id == key_id).map(|(_, key)| *key))
}

/// Who a source trusts; closures keep fe-hexon free of fe-plugin's trust-root file and kill list.
#[derive(Clone)]
pub struct SourceTrust {
    /// True when a `did:key` is a configured package trust root.
    pub is_trust_root: KeyPredicate,
    /// This source's pinned index keys by key id (built per endpoint; never a package or revocation signer).
    pub index_signer: KeyLookup,
    /// Kill list, checked at offer, header and fetch; the audited Owner override lives in the predicate.
    pub is_killed: KillPredicate,
    /// Dev escape hatch for unsigned packages; never set in release builds.
    pub allow_unsigned_dev: bool,
}

impl SourceTrust {
    /// Trust over `is_trust_root` with no pinned index key and no kill list.
    pub fn new(is_trust_root: KeyPredicate) -> Self {
        Self { is_trust_root, index_signer: Arc::new(|_| None), is_killed: no_kill_list(), allow_unsigned_dev: false }
    }

    /// The same package trust with this endpoint's own index-key pin.
    pub fn with_index_pin(mut self, pins: Vec<(String, VerifyingKey)>) -> Self {
        self.index_signer = index_pin(pins);
        self
    }

    /// The same trust with a kill-list predicate.
    pub fn with_kill_list(mut self, is_killed: KillPredicate) -> Self {
        self.is_killed = is_killed;
        self
    }

    fn plugin_trust(&self) -> PluginTrust<'_> {
        PluginTrust { is_trust_root: &*self.is_trust_root, allow_unsigned_dev: self.allow_unsigned_dev }
    }

    /// `Err(Revoked)` when the kill list names `hash` (under any id).
    fn refuse_killed(&self, package_id: &str, hash: &str) -> Result<(), RegistryError> {
        if (self.is_killed)(package_id, hash) {
            return Err(RegistryError::Revoked { package_id: package_id.to_owned(), content_hash: hash.to_owned() });
        }
        Ok(())
    }
}

fn matches_query(entry: &RegistryEntry, query: Option<&str>) -> bool {
    let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else { return true };
    let needle = query.to_lowercase();
    entry.package_id.to_lowercase().contains(&needle) || entry.name.to_lowercase().contains(&needle)
}

/// Store family of a manifest v2 document: `web-bundle` when its `kind` says so, otherwise `extension`.
pub fn store_family(extension_json: &[u8]) -> &'static str {
    let kind = serde_json::from_slice::<Value>(extension_json)
        .ok()
        .and_then(|doc| doc.get("kind").and_then(Value::as_str).map(str::to_owned));
    if kind.as_deref() == Some(WEB_BUNDLE_KIND) {
        WEB_BUNDLE_KIND
    } else {
        EXTENSION_KIND
    }
}

/// `access` values a listing may install from.
pub fn installable_access(access: &str) -> bool {
    access == ACCESS_FREE || access == ACCESS_LICENSED
}

fn plugin_refusal(error: PluginPackageError) -> RegistryError {
    match error {
        PluginPackageError::Invalid(reason) => RegistryError::PackageInvalid(reason),
        PluginPackageError::UntrustedSigner(reason) => RegistryError::PackageUntrusted(reason),
        PluginPackageError::UnsupportedContract => RegistryError::PackageContract,
    }
}

/// Verify sealed bytes against an entry: seal and trust, listed version, listed content hash, kill list.
pub fn verify_fetched(bytes: Vec<u8>, entry: &RegistryEntry, trust: &SourceTrust) -> Result<FetchedPackage, RegistryError> {
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "package".into(), max: PACKAGE_MAX_BYTES });
    }
    let package = verify_plugin_package(&bytes, Some(&entry.package_id), &trust.plugin_trust()).map_err(plugin_refusal)?;
    if package.version != entry.version {
        return Err(RegistryError::VersionMismatch { listed: entry.version.clone(), served: package.version });
    }
    if !package.content_hash.eq_ignore_ascii_case(&entry.content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: entry.content_hash.clone(), served: package.content_hash });
    }
    for hash in package.revocation_keys() {
        trust.refuse_killed(&package.package_id, hash)?;
    }
    Ok(FetchedPackage { bytes, package })
}

/// Verify a sealed web bundle against an entry and its listed publisher: seal by that publisher (a trust root),
/// id, version, content hash, manifest v2 kind and kill list. The portal verifier runs at install (P-STORE).
pub fn verify_web_bundle(
    bytes: Vec<u8>,
    entry: &RegistryEntry,
    publisher_pin: &str,
    trust: &SourceTrust,
) -> Result<FetchedPackage, RegistryError> {
    use fe_format::integrity::PLUGIN_EXTENSION_PATH;
    use fe_format::limits::MAX_JSON_ENTRY_BYTES;
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "package".into(), max: PACKAGE_MAX_BYTES });
    }
    let manifest_raw = read_manifest_raw(&bytes)?;
    let manifest: fe_format::HexonManifest =
        serde_json::from_slice(&manifest_raw).map_err(|e| RegistryError::PackageInvalid(format!("manifest.json: {e}")))?;
    // The id check comes first: a mirror cannot serve a revoked build under a new id.
    if manifest.hexon_id != entry.package_id {
        return Err(RegistryError::PackageInvalid("id-mismatch".into()));
    }
    if manifest.publisher_did != publisher_pin {
        return Err(RegistryError::PublisherMismatch { listed: publisher_pin.to_owned(), served: manifest.publisher_did });
    }
    let policy = fe_format::IntegrityPolicy::RequireSealed { pin: Some(publisher_pin.to_owned()) };
    let publisher_did = match fe_format::verify_archive(&bytes, &policy) {
        Ok(fe_format::IntegrityStatus::Sealed { publisher_did }) => publisher_did,
        Ok(fe_format::IntegrityStatus::Unsealed) => return Err(RegistryError::PackageUntrusted("unsigned".into())),
        Err(error) => return Err(RegistryError::PackageInvalid(error.to_string())),
    };
    if !(trust.is_trust_root)(&publisher_did) {
        return Err(RegistryError::PackageUntrusted(format!("publisher {publisher_did} is not a trust root")));
    }
    if manifest.version != entry.version {
        return Err(RegistryError::VersionMismatch { listed: entry.version.clone(), served: manifest.version });
    }
    let content_hash =
        fe_format::integrity::content_hash(&manifest_raw).map_err(|e| RegistryError::PackageInvalid(format!("{e:#}")))?;
    if !content_hash.eq_ignore_ascii_case(&entry.content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: entry.content_hash.clone(), served: content_hash });
    }
    trust.refuse_killed(&manifest.hexon_id, &content_hash)?;
    let read = |path: &str| {
        fe_format::HexonArchive::read_member(&bytes, path, MAX_JSON_ENTRY_BYTES)
            .map_err(|e| RegistryError::PackageInvalid(format!("{path}: {e:#}")))
    };
    let extension_json =
        read(PLUGIN_EXTENSION_PATH)?.ok_or_else(|| RegistryError::PackageInvalid(format!("missing-member {PLUGIN_EXTENSION_PATH}")))?;
    let served_kind = store_family(&extension_json);
    if served_kind != WEB_BUNDLE_KIND {
        return Err(RegistryError::KindMismatch { listed: WEB_BUNDLE_KIND.into(), served: served_kind.into() });
    }
    let extension: Value =
        serde_json::from_slice(&extension_json).map_err(|e| RegistryError::PackageInvalid(format!("extension.json: {e}")))?;
    let field = |key: &str| extension.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    if field("id") != manifest.hexon_id || field("version") != manifest.version {
        return Err(RegistryError::PackageInvalid("extension.json names another build".into()));
    }
    let license_json = read(LICENSE_PATH)?;
    let package = PluginPackage {
        package_id: manifest.hexon_id,
        version: manifest.version,
        publisher_did: publisher_did.clone(),
        // A web bundle has no component; its only revocation key is the content hash.
        component_digest: content_hash.clone(),
        content_hash,
        extension_json,
        license_json,
        component: Vec::new(),
        trusted_by: Some(publisher_did),
    };
    Ok(FetchedPackage { bytes, package })
}

/// A verified non-plugin hexon (tileset, terrain, ...): sealed bytes plus the identity its manifest vouched for.
#[derive(Debug, Clone)]
pub struct VerifiedContent {
    /// The `.hexon` bytes, ready for the install path.
    pub bytes: Vec<u8>,
    /// Manifest `hexon_id`, equal to the requested id.
    pub hexon_id: String,
    /// Manifest version, equal to the listed one.
    pub version: String,
    /// Manifest `hexon_type` as its serde name (e.g. `terrain_tileset`).
    pub hexon_type: String,
    /// BLAKE3 of the manifest with the signature blanked, equal to the listed hash.
    pub content_hash: String,
    /// Sealing publisher (a trust root); `None` for a dev-unsigned archive.
    pub publisher_did: Option<String>,
}

/// Verified bytes of either hexon family; plugins keep their full package verdict.
#[derive(Debug, Clone)]
pub enum FetchedHexon {
    Plugin(FetchedPackage),
    Content(VerifiedContent),
}

/// Raw `manifest.json` of a `.hexon`, read by the zip reader `verify_archive` uses (no parser differential).
fn read_manifest_raw(bytes: &[u8]) -> Result<Vec<u8>, RegistryError> {
    fe_format::HexonArchive::read_member(bytes, fe_format::integrity::MANIFEST_PATH, fe_format::limits::MAX_JSON_ENTRY_BYTES)
        .map_err(|e| RegistryError::PackageInvalid(format!("manifest.json: {e:#}")))?
        .ok_or_else(|| RegistryError::PackageInvalid("missing-member manifest.json".into()))
}

/// Verify sealed bytes of any hexon type: plugins through [`verify_fetched`], content hexons by seal, id, version and listed hash.
pub fn verify_fetched_hexon(bytes: Vec<u8>, entry: &RegistryEntry, trust: &SourceTrust) -> Result<FetchedHexon, RegistryError> {
    if bytes.len() as u64 > PACKAGE_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "package".into(), max: PACKAGE_MAX_BYTES });
    }
    let manifest_raw = read_manifest_raw(&bytes)?;
    let manifest: fe_format::HexonManifest =
        serde_json::from_slice(&manifest_raw).map_err(|e| RegistryError::PackageInvalid(format!("manifest.json: {e}")))?;
    if matches!(manifest.hexon_type, fe_format::HexonType::Plugin) {
        return verify_fetched(bytes, entry, trust).map(FetchedHexon::Plugin);
    }
    // The id check comes first: a mirror cannot serve a revoked build under a new id.
    if manifest.hexon_id != entry.package_id {
        return Err(RegistryError::PackageInvalid("id-mismatch".into()));
    }
    let publisher_did = match fe_format::verify_archive(&bytes, &fe_format::IntegrityPolicy::VerifyIfSealed) {
        Ok(fe_format::IntegrityStatus::Sealed { publisher_did }) => {
            if !(trust.is_trust_root)(&publisher_did) {
                return Err(RegistryError::PackageUntrusted(format!("publisher {publisher_did} is not a trust root")));
            }
            Some(publisher_did)
        }
        Ok(fe_format::IntegrityStatus::Unsealed) if trust.allow_unsigned_dev => None,
        Ok(fe_format::IntegrityStatus::Unsealed) => return Err(RegistryError::PackageUntrusted("unsigned".into())),
        Err(error) => return Err(RegistryError::PackageInvalid(error.to_string())),
    };
    if manifest.version != entry.version {
        return Err(RegistryError::VersionMismatch { listed: entry.version.clone(), served: manifest.version });
    }
    let content_hash =
        fe_format::integrity::content_hash(&manifest_raw).map_err(|e| RegistryError::PackageInvalid(format!("{e:#}")))?;
    if !content_hash.eq_ignore_ascii_case(&entry.content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: entry.content_hash.clone(), served: content_hash });
    }
    trust.refuse_killed(&manifest.hexon_id, &content_hash)?;
    let hexon_type = serde_json::to_value(&manifest.hexon_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(FetchedHexon::Content(VerifiedContent {
        bytes,
        hexon_id: manifest.hexon_id,
        version: manifest.version,
        hexon_type,
        content_hash,
        publisher_did,
    }))
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

/// Move entries a kill list revokes into `refused` (shown unavailable, never offered for install).
fn split_killed(entries: Vec<RegistryEntry>, trust: &SourceTrust, refused: &mut Vec<Refusal>) -> Vec<RegistryEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            let killed = (trust.is_killed)(&entry.package_id, &entry.content_hash);
            if killed {
                refused.push(Refusal { what: entry.package_id.clone(), reason: "revoked by a kill list".into() });
            }
            !killed
        })
        .collect()
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
                let package = verify_plugin_package(&bytes, None, &self.trust.plugin_trust()).map_err(plugin_refusal)?;
                for hash in package.revocation_keys() {
                    self.trust.refuse_killed(&package.package_id, hash)?;
                }
                Ok(entry_for(&package, path.to_string_lossy().into_owned(), size))
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

// ---- Catalogue documents (store-client contract v1) ---------------------------------------------

/// `config.json`: an unsigned bootstrap; it names features and keys but never confers trust.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogConfig {
    /// Document schema (refused when newer than [`CONFIG_SCHEMA_VERSION`]).
    pub schema_version: u64,
    /// Store families the endpoint serves.
    pub kinds: Vec<String>,
    /// Advertised features; a missing one is never probed.
    pub features: BTreeSet<String>,
    /// Advisory: the key id signing `index.json` now (never selects or widens the pin).
    pub index_key_id: Option<String>,
    /// Advisory: where the endpoint publishes its kill list (never configures one).
    pub revocation_url: Option<String>,
}

impl CatalogConfig {
    /// True when `features` advertises `feature`.
    pub fn has(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }
}

/// `schema_version` of a document (absent means 1), refused when newer than `known`.
fn check_schema(doc: &Value, known: u64, document: &str) -> Result<u64, RegistryError> {
    let found = match doc.get("schema_version") {
        None | Some(Value::Null) => 1,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| RegistryError::IndexMalformed(format!("{document}: schema_version is not an integer")))?,
    };
    if found > known {
        return Err(RegistryError::SchemaTooNew { document: document.into(), found });
    }
    Ok(found)
}

/// Parse `config.json`: protocol 1 (integer, or the string `"1"`) and a schema this client reads.
pub fn parse_config(bytes: &[u8]) -> Result<CatalogConfig, RegistryError> {
    if bytes.len() as u64 > DOC_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: CONFIG_PATH.into(), max: DOC_MAX_BYTES });
    }
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| RegistryError::IndexMalformed(format!("{CONFIG_PATH}: {e}")))?;
    if !doc.is_object() {
        return Err(RegistryError::IndexMalformed(format!("{CONFIG_PATH}: not a JSON object")));
    }
    let found = doc.get("registry_protocol").map(|value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    });
    if found.as_deref() != Some(PROTOCOL_V1) {
        return Err(RegistryError::ProtocolMismatch { found });
    }
    let schema_version = check_schema(&doc, CONFIG_SCHEMA_VERSION, CONFIG_PATH)?;
    let strings = |key: &str| -> Vec<String> {
        doc.get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    };
    let text = |key: &str| doc.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(CatalogConfig {
        schema_version,
        kinds: strings("kinds"),
        features: strings("features").into_iter().collect(),
        index_key_id: text("index_key_id"),
        revocation_url: text("revocation_url"),
    })
}

/// `publisher` of an index entry or detail document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publisher {
    /// `did:key` that must equal the verified manifest's `publisher_did`.
    #[serde(default)]
    pub did: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Signed through the index: the operator verified this publisher (a claim, not a badge).
    #[serde(default)]
    pub verified: bool,
}

/// One `index.json` entry (registry contract v1 `IndexEntry`); unknown members are signed but not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Package id (`^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`).
    pub package_id: String,
    /// `extension` or `web-bundle` (other kinds are skipped before this type is built).
    pub kind: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// One-line summary.
    #[serde(default)]
    pub summary: String,
    /// Listed publisher.
    #[serde(default)]
    pub publisher: Publisher,
    /// Newest offered version (SemVer without build metadata).
    pub latest_version: String,
    /// Content hash of that version's sealed build.
    pub latest_content_hash: String,
    /// BLAKE3 hex of `packages/{package_id}.json` exactly as served.
    pub detail_hash: String,
    /// `free` or `licensed`; anything else is not installable from this listing.
    #[serde(default)]
    pub access: String,
    /// Exact-match categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Exact, case-sensitive tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Icon location (display only).
    #[serde(default)]
    pub icon_url: Option<String>,
    /// RFC 3339 time of the newest change; drives the `updated` sort.
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl IndexEntry {
    /// `updated_at` parsed; `None` sorts last.
    pub fn updated(&self) -> Option<DateTime<Utc>> {
        self.updated_at
            .as_deref()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|at| at.with_timezone(&Utc))
    }

    /// The listing's install view of this entry (latest version).
    pub fn to_registry_entry(&self) -> RegistryEntry {
        RegistryEntry {
            package_id: self.package_id.clone(),
            version: self.latest_version.clone(),
            content_hash: self.latest_content_hash.clone(),
            url: format!("{DETAIL_DIR}/{}.json", self.package_id),
            size: 0,
            name: self.name.clone(),
            header_url: None,
        }
    }
}

/// An index whose signature, schema and entries passed verification.
#[derive(Debug, Clone)]
pub struct VerifiedIndex {
    /// Signed issue time; an index older than the verified floor is ignored (rollback guard).
    pub issued_at: DateTime<Utc>,
    /// Signed expiry, if any.
    pub expires_at: Option<DateTime<Utc>>,
    /// Entries of a known kind with a valid id, version and digests, in index order.
    pub entries: Vec<IndexEntry>,
    /// Entries skipped for a malformed shape, id, version or a case-only id collision.
    pub refused: Vec<Refusal>,
}

impl VerifiedIndex {
    /// True once `expires_at` has passed.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expiry| expiry <= now)
    }

    /// The verified entry for `package_id`.
    pub fn entry(&self, package_id: &str) -> Option<&IndexEntry> {
        self.entries.iter().find(|entry| entry.package_id == package_id)
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
    // Strict, as for revocation lists: no small-order or non-canonical signature components.
    key.verify_strict(&index_signed_bytes(&doc)?, &signature).map_err(|_| RegistryError::IndexSignature)?;

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
    if raw_entries.len() > INDEX_MAX_ENTRIES {
        return Err(RegistryError::IndexMalformed(format!("more than {INDEX_MAX_ENTRIES} entries")));
    }

    let mut index = VerifiedIndex { issued_at, expires_at, entries: Vec::new(), refused: Vec::new() };
    // One package-id namespace across kinds: collisions are checked before kinds are filtered.
    let mut seen_ids = HashSet::new();
    for raw in raw_entries {
        let what = raw.get("package_id").and_then(Value::as_str).unwrap_or("index entry").to_owned();
        let kind = raw.get("kind").and_then(Value::as_str).unwrap_or_default();
        if !valid_package_id(&what) {
            index.refused.push(Refusal { what, reason: "package id does not match the id grammar".into() });
            continue;
        }
        if !seen_ids.insert(what.to_ascii_lowercase()) {
            index.refused.push(Refusal { what, reason: "package id collides with an earlier entry".into() });
            continue;
        }
        if !KNOWN_KINDS.contains(&kind) {
            continue;
        }
        let entry: IndexEntry = match serde_json::from_value(raw.clone()) {
            Ok(entry) => entry,
            Err(error) => {
                index.refused.push(Refusal { what, reason: format!("malformed index entry: {error}") });
                continue;
            }
        };
        let refusal = if !valid_version(&entry.latest_version) {
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

/// Cargo caret requirement (`"2"`, `"2.0"`, `"^2.0.1"`) against `version`; any other syntax is unsatisfiable.
pub fn caret_matches(requirement: &str, version: (u64, u64, u64)) -> bool {
    let text = requirement.trim();
    let text = text.strip_prefix('^').unwrap_or(text).trim();
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 3 || !parts.iter().all(|part| semver_number(part)) {
        return false;
    }
    let Ok(numbers) = parts.iter().map(|part| part.parse::<u64>()).collect::<Result<Vec<u64>, _>>() else {
        return false;
    };
    let lower = (numbers[0], numbers.get(1).copied().unwrap_or(0), numbers.get(2).copied().unwrap_or(0));
    let upper = match numbers.as_slice() {
        [major, ..] if *major > 0 => (major + 1, 0, 0),
        [_] => (1, 0, 0),
        [_, minor] | [_, minor, _] if *minor > 0 => (0, minor + 1, 0),
        [_, minor] => (0, minor + 1, 0),
        [_, _, patch] => (0, 0, patch + 1),
        _ => return false,
    };
    lower <= version && version < upper
}

/// Every `requires` interface is one this host implements, at a version the requirement admits.
pub fn requires_satisfied(requires: &BTreeMap<String, String>) -> bool {
    requires.iter().all(|(interface, requirement)| {
        let host = match interface.as_str() {
            PLUGIN_INTERFACE => HOST_PLUGIN_INTERFACE,
            PORTAL_INTERFACE => HOST_PORTAL_INTERFACE,
            _ => return false,
        };
        caret_matches(requirement, host)
    })
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
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok()).collect()
}

/// Hex (the revocation list's encoding) or standard base64.
fn decode_signature(text: &str) -> Option<Signature> {
    let raw = decode_hex(text).or_else(|| base64::engine::general_purpose::STANDARD.decode(text).ok())?;
    Signature::from_slice(&raw).ok()
}

/// RFC 3986 percent-encoding of everything but the unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `(scheme://authority, path segments)` of an http(s) URL; `None` for anything else or a URL carrying userinfo.
fn split_http(url: &str) -> Option<(String, Vec<String>)> {
    let lower = url.to_ascii_lowercase();
    let scheme_len = if lower.starts_with("https://") {
        8
    } else if lower.starts_with("http://") {
        7
    } else {
        return None;
    };
    let rest = &url[scheme_len..];
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Userinfo would send a credential to the host a document chose.
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let segments = path.split('/').filter(|part| !part.is_empty()).map(str::to_owned).collect();
    Some((format!("{}{authority}", &url[..scheme_len]), segments))
}

/// Resolve `location` from a document in `document_dir` (segments under the endpoint) against the endpoint `base`.
/// Absolute http(s) URLs pass (CDN downloads) unless they carry userinfo; relative ones must stay under `base`
/// (contract rule 1.6). Backslashes, drive or scheme prefixes, network paths, encoded dots or slashes, query and
/// fragment are refused, so Windows never authenticates to a host a document chose.
pub fn resolve_location(base: &str, document_dir: &str, location: &str) -> Result<String, RegistryError> {
    let refuse = |why: &str| Err(RegistryError::LocationRefused(format!("{location:?}: {why}")));
    if location.is_empty() || location.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return refuse("empty or contains whitespace");
    }
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return match split_http(location) {
            Some(_) => Ok(location.to_owned()),
            None => refuse("not a plain http(s) URL"),
        };
    }
    if location.contains('\\') || location.contains(':') || location.starts_with("//") {
        return refuse("not a path inside the source");
    }
    if location.contains('?') || location.contains('#') {
        return refuse("query or fragment in a relative location");
    }
    if ["%2e", "%2f", "%5c"].iter().any(|encoded| lower.contains(encoded)) {
        return refuse("encoded dot or separator");
    }
    let (origin, base_segments) = match split_http(base) {
        Some((origin, segments)) => (Some(origin), segments),
        None => (None, Vec::new()),
    };
    let mut segments: Vec<String> = if location.starts_with('/') {
        if origin.is_none() {
            return refuse("root-relative path on a folder endpoint");
        }
        Vec::new()
    } else {
        base_segments.iter().cloned().chain(document_dir.split('/').filter(|part| !part.is_empty()).map(str::to_owned)).collect()
    };
    for part in location.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return refuse("climbs out of the source");
                }
            }
            other => segments.push(other.to_owned()),
        }
    }
    if segments.len() <= base_segments.len() || segments[..base_segments.len()] != base_segments[..] {
        return refuse("climbs out of the source");
    }
    Ok(match origin {
        Some(origin) => format!("{origin}/{}", segments.join("/")),
        None => format!("{}/{}", base.trim_end_matches(['/', '\\']), segments.join("/")),
    })
}

/// `acquire_url` with `licensee_did` and `device_x25519_did` appended as percent-encoded query parameters (https only).
pub fn acquire_link(acquire_url: &str, licensee_did: &str, device_x25519_did: &str) -> Result<String, RegistryError> {
    if !acquire_url.to_ascii_lowercase().starts_with("https://") || split_http(acquire_url).is_none() {
        return Err(RegistryError::LocationRefused(format!("{acquire_url:?}: acquire_url must be a plain https URL")));
    }
    let (head, fragment) = match acquire_url.split_once('#') {
        Some((head, fragment)) => (head, Some(fragment)),
        None => (acquire_url, None),
    };
    let separator = match head.find('?') {
        None => "?",
        Some(_) if head.ends_with('?') || head.ends_with('&') => "",
        Some(_) => "&",
    };
    let mut link = format!(
        "{head}{separator}licensee_did={}&device_x25519_did={}",
        percent_encode(licensee_did),
        percent_encode(device_x25519_did)
    );
    if let Some(fragment) = fragment {
        link.push('#');
        link.push_str(fragment);
    }
    Ok(link)
}

/// `acquire_url` a detail may offer: present only for a `licensed` package on an endpoint advertising
/// `entitlements`, and only when it is a plain https URL.
pub fn checked_acquire_url(detail: &PackageDetail, config: Option<&CatalogConfig>) -> Option<String> {
    let url = detail.acquire_url.as_deref()?;
    let offered = detail.access == ACCESS_LICENSED && config.is_some_and(|config| config.has(FEATURE_ENTITLEMENTS));
    (offered && url.to_ascii_lowercase().starts_with("https://") && split_http(url).is_some()).then(|| url.to_owned())
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDetail {
    /// Document schema.
    #[serde(default)]
    pub schema_version: u64,
    /// Equal to the index entry's id.
    pub package_id: String,
    /// Equal to the index entry's kind.
    pub kind: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Long description (display only).
    #[serde(default)]
    pub description: String,
    /// Listed publisher; must equal the index entry's.
    #[serde(default)]
    pub publisher: Publisher,
    /// `free` or `licensed`.
    #[serde(default)]
    pub access: String,
    /// Raw acquire link; read it through [`checked_acquire_url`].
    #[serde(default)]
    pub acquire_url: Option<String>,
    /// Categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// `homepage`, `source`, `issues` (display only).
    #[serde(default)]
    pub links: Value,
    /// Versions this host can satisfy, newest first (invalid or unsatisfiable ones are refusal lines).
    #[serde(default)]
    pub versions: Vec<DetailVersion>,
}

/// One version of a package detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailVersion {
    /// SemVer without build metadata.
    pub version: String,
    /// Content hash of the sealed build.
    pub content_hash: String,
    /// Exact archive size; a download past it is aborted.
    pub size_bytes: u64,
    /// RFC 3339 publish time (display only).
    #[serde(default)]
    pub published_at: Option<String>,
    /// Interface id to Cargo caret requirement.
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
    /// Absolute, or relative to the detail document.
    pub download_url: String,
    /// Hidden from new installs.
    #[serde(default)]
    pub yanked: bool,
}

/// A detail document checked against the verified index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDetail {
    /// The verified index entry the detail is pinned by.
    pub listed: IndexEntry,
    /// The detail, with only the versions this host can use.
    pub detail: PackageDetail,
    /// Versions skipped, with the reason.
    pub refused: Vec<Refusal>,
    /// The acquire link the UI may open (see [`checked_acquire_url`]).
    pub acquire_url: Option<String>,
    /// True when the latest listed build is on the kill list (shown unavailable).
    pub revoked: bool,
}

/// Check detail bytes against the verified index entry: hash pin, then id, kind and publisher, then versions.
pub fn verify_detail(bytes: &[u8], listed: &IndexEntry, config: Option<&CatalogConfig>) -> Result<VerifiedDetail, RegistryError> {
    if bytes.len() as u64 > DOC_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "package detail".into(), max: DOC_MAX_BYTES });
    }
    if !fe_format::integrity::digest_hex(bytes).eq_ignore_ascii_case(&listed.detail_hash) {
        return Err(RegistryError::DetailMismatch("detail document hash differs from the index".into()));
    }
    let mut doc: Value = serde_json::from_slice(bytes).map_err(|e| RegistryError::DetailMismatch(e.to_string()))?;
    let member = |key: &str| doc.get(key).and_then(Value::as_str).map(str::to_owned);
    if member("package_id").as_deref() != Some(listed.package_id.as_str()) || member("kind").as_deref() != Some(listed.kind.as_str()) {
        return Err(RegistryError::DetailMismatch("package id or kind differs from the index".into()));
    }
    check_schema(&doc, DETAIL_SCHEMA_VERSION, "package detail")?;
    let raw_versions = match doc.as_object_mut().and_then(|object| object.remove("versions")) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    let mut detail: PackageDetail = serde_json::from_value(doc).map_err(|e| RegistryError::DetailMismatch(e.to_string()))?;
    if detail.publisher.did != listed.publisher.did {
        return Err(RegistryError::DetailMismatch("publisher differs from the index".into()));
    }
    let mut refused = Vec::new();
    for raw in raw_versions {
        let what = format!("{}@{}", listed.package_id, raw.get("version").and_then(Value::as_str).unwrap_or("?"));
        let version: DetailVersion = match serde_json::from_value(raw) {
            Ok(version) => version,
            Err(error) => {
                refused.push(Refusal { what, reason: format!("malformed version: {error}") });
                continue;
            }
        };
        let reason = if !valid_version(&version.version) {
            Some("version is not SemVer without build metadata")
        } else if !is_digest_hex(&version.content_hash) {
            Some("content hash is not BLAKE3 hex")
        } else if version.size_bytes == 0 || version.size_bytes > PACKAGE_MAX_BYTES {
            Some("size_bytes is zero or exceeds the package cap")
        } else if !requires_satisfied(&version.requires) {
            Some("requires an interface version this host does not implement")
        } else {
            None
        };
        if version.version == listed.latest_version && !version.content_hash.eq_ignore_ascii_case(&listed.latest_content_hash) {
            return Err(RegistryError::DetailMismatch("latest version hash differs from the index".into()));
        }
        match reason {
            Some(reason) => refused.push(Refusal { what, reason: reason.into() }),
            None => detail.versions.push(version),
        }
    }
    let acquire_url = checked_acquire_url(&detail, config);
    Ok(VerifiedDetail { listed: listed.clone(), detail, refused, acquire_url, revoked: false })
}

/// `packages/{id}/{version}/attestation.json`: the header phase served as a file.
#[derive(Debug, Clone, Deserialize)]
struct Attestation {
    package_id: String,
    version: String,
    content_hash: String,
    manifest_b64: String,
    #[serde(default)]
    header_members_b64: BTreeMap<String, String>,
    #[serde(default)]
    signatures: Vec<AttestationSignature>,
}

#[derive(Debug, Clone, Deserialize)]
struct AttestationSignature {
    key_id: String,
    signature: String,
}

/// The header phase of one build, verified before its body is downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAttestation {
    /// Manifest `hexon_id`.
    pub package_id: String,
    /// Manifest version.
    pub version: String,
    /// Recomputed from the manifest, never taken from the document.
    pub content_hash: String,
    /// The sealing publisher.
    pub publisher_did: String,
    /// Store family from the verified `plugin/extension.json`.
    pub kind: String,
    /// Committed `plugin/extension.json` bytes (consent renders from these).
    pub extension_json: Vec<u8>,
    /// Committed `license.json`, when the build carries one.
    pub license_json: Option<Vec<u8>>,
    /// Trust roots (other than the publisher) whose counter-signature over the same bytes verified.
    pub reviewed_by: Vec<String>,
}

impl VerifiedAttestation {
    /// "Reviewed" badge: a counter-signature verified against a trust root (never a document field).
    pub fn is_reviewed(&self) -> bool {
        !self.reviewed_by.is_empty()
    }
}

/// What an attestation must name: the requested build, its listed publisher and store family when known.
#[derive(Debug, Clone, Copy)]
pub struct ExpectedBuild<'a> {
    pub package_id: &'a str,
    pub version: &'a str,
    pub content_hash: &'a str,
    /// Listing's `publisher.did`, the `verify_sealed_header` pin (`None` for protocol-v1, which lists none).
    pub publisher_pin: Option<&'a str>,
    /// Listing's `kind`, compared with the verified manifest's family.
    pub kind: Option<&'a str>,
}

/// Verify an attestation: names the build, seals under the pinned publisher, recomputes to the content hash,
/// commits `plugin/extension.json`, is not kill-listed and is vouched for by a trust root (publisher or reviewer).
pub fn verify_attestation(bytes: &[u8], expected: &ExpectedBuild<'_>, trust: &SourceTrust) -> Result<VerifiedAttestation, RegistryError> {
    use fe_format::integrity::PLUGIN_EXTENSION_PATH;
    if bytes.len() as u64 > HEADER_MAX_BYTES {
        return Err(RegistryError::TooLarge { what: "attestation".into(), max: HEADER_MAX_BYTES });
    }
    let invalid = |reason: String| RegistryError::PackageInvalid(format!("attestation: {reason}"));
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
    check_schema(&doc, ATTESTATION_SCHEMA_VERSION, "attestation")?;
    let attestation: Attestation = serde_json::from_value(doc).map_err(|e| invalid(e.to_string()))?;
    if attestation.package_id != expected.package_id
        || attestation.version != expected.version
        || !attestation.content_hash.eq_ignore_ascii_case(expected.content_hash)
    {
        return Err(invalid("names another build".into()));
    }
    if attestation.signatures.len() > ATTESTATION_MAX_SIGNATURES {
        return Err(invalid(format!("more than {ATTESTATION_MAX_SIGNATURES} signatures")));
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let manifest = engine.decode(&attestation.manifest_b64).map_err(|e| invalid(e.to_string()))?;
    let mut members = BTreeMap::new();
    for (path, encoded) in &attestation.header_members_b64 {
        members.insert(path.clone(), engine.decode(encoded).map_err(|e| invalid(format!("{path}: {e}")))?);
    }
    let parsed: fe_format::HexonManifest = serde_json::from_slice(&manifest).map_err(|e| invalid(e.to_string()))?;
    if parsed.hexon_id != expected.package_id || parsed.version != expected.version {
        return Err(invalid("manifest names another package".into()));
    }
    if let Some(pin) = expected.publisher_pin {
        if parsed.publisher_did != pin {
            return Err(RegistryError::PublisherMismatch { listed: pin.to_owned(), served: parsed.publisher_did });
        }
    }
    let policy = fe_format::IntegrityPolicy::RequireSealed { pin: expected.publisher_pin.map(str::to_owned) };
    fe_format::verify_sealed_header(&manifest, &members, &policy).map_err(|e| invalid(e.to_string()))?;
    let committed = parsed.integrity.as_ref().is_some_and(|root| root.header.contains_key(PLUGIN_EXTENSION_PATH));
    if !committed {
        return Err(invalid("plugin/extension.json is not a committed header member".into()));
    }
    let content_hash = fe_format::integrity::content_hash(&manifest).map_err(|e| invalid(format!("{e:#}")))?;
    if !content_hash.eq_ignore_ascii_case(expected.content_hash) {
        return Err(RegistryError::ContentHashMismatch { listed: expected.content_hash.to_owned(), served: content_hash });
    }
    trust.refuse_killed(&parsed.hexon_id, &content_hash)?;
    // Counter-signatures cover the publisher-signed bytes; only trust roots count, and a bad one earns no badge.
    let mut reviewed_by = Vec::new();
    for entry in &attestation.signatures {
        if entry.key_id == parsed.publisher_did || !(trust.is_trust_root)(&entry.key_id) || reviewed_by.contains(&entry.key_id) {
            continue;
        }
        let Some(signature) = decode_signature(&entry.signature) else { continue };
        let encoded = engine.encode(signature.to_bytes());
        if fe_format::verify_manifest(&manifest, &encoded, &entry.key_id).unwrap_or(false) {
            reviewed_by.push(entry.key_id.clone());
        }
    }
    if !(trust.is_trust_root)(&parsed.publisher_did) && reviewed_by.is_empty() {
        return Err(RegistryError::PackageUntrusted(format!("publisher {} is not a trust root", parsed.publisher_did)));
    }
    let extension_json = members.remove(PLUGIN_EXTENSION_PATH).ok_or(RegistryError::HeaderUnavailable)?;
    let kind = store_family(&extension_json);
    if let Some(listed) = expected.kind {
        if listed != kind {
            return Err(RegistryError::KindMismatch { listed: listed.to_owned(), served: kind.to_owned() });
        }
    }
    Ok(VerifiedAttestation {
        package_id: parsed.hexon_id,
        version: parsed.version,
        content_hash,
        publisher_did: parsed.publisher_did,
        kind: kind.to_owned(),
        extension_json,
        license_json: members.remove(LICENSE_PATH),
        reviewed_by,
    })
}

// ---- Search (store-client contract v1 §3) ---------------------------------------------------------

/// `sort` of a search: newest `updated_at` first (default) or by name; ties by `package_id`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchSort {
    #[default]
    Updated,
    Name,
}

impl SearchSort {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Name => "name",
        }
    }

    /// Parse a wire name; an unknown one falls back to `updated` locally.
    pub fn parse(text: &str) -> Self {
        if text == "name" {
            Self::Name
        } else {
            Self::Updated
        }
    }
}

/// One search request; every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Case-insensitive substring over id, name and summary.
    pub q: Option<String>,
    /// Exact store family.
    pub kind: Option<String>,
    /// Exact category.
    pub category: Option<String>,
    /// Comma-separated, all-of, exact and case-sensitive tags.
    pub tag: Option<String>,
    /// Result order.
    pub sort: SearchSort,
    /// Opaque hosted cursor (ignored by the local fallback).
    pub cursor: Option<String>,
}

impl SearchQuery {
    fn params(&self) -> [(&'static str, Option<&str>); 5] {
        [
            ("q", self.q.as_deref()),
            ("kind", self.kind.as_deref()),
            ("category", self.category.as_deref()),
            ("tag", self.tag.as_deref()),
            ("cursor", self.cursor.as_deref()),
        ]
    }

    fn check(&self) -> Result<(), RegistryError> {
        for (name, value) in self.params() {
            if value.is_some_and(|value| value.chars().count() > SEARCH_PARAM_MAX_CHARS) {
                return Err(RegistryError::InvalidQuery(format!("{name} is longer than {SEARCH_PARAM_MAX_CHARS} characters")));
            }
        }
        Ok(())
    }

    /// `?sort=…&q=…` with every present parameter percent-encoded.
    pub fn query_string(&self) -> String {
        let mut query = format!("?sort={}", self.sort.as_str());
        for (name, value) in self.params() {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                query.push_str(&format!("&{name}={}", percent_encode(value)));
            }
        }
        query
    }
}

/// True when `entry` satisfies the contract's search rules (server and local fallback alike).
pub fn matches_search(entry: &IndexEntry, query: &SearchQuery) -> bool {
    let present = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
    let text_ok = present(&query.q).is_none_or(|q| {
        let needle = q.to_lowercase();
        [&entry.package_id, &entry.name, &entry.summary].iter().any(|field| field.to_lowercase().contains(&needle))
    });
    let kind_ok = present(&query.kind).is_none_or(|kind| entry.kind == kind);
    let category_ok = present(&query.category).is_none_or(|category| entry.categories.contains(&category));
    let tags_ok = present(&query.tag)
        .is_none_or(|tags| tags.split(',').filter(|tag| !tag.is_empty()).all(|tag| entry.tags.iter().any(|known| known == tag)));
    text_ok && kind_ok && category_ok && tags_ok
}

/// Order entries by `sort`, ties broken by `package_id` (byte order).
pub fn sort_entries(entries: &mut [IndexEntry], sort: SearchSort) {
    match sort {
        SearchSort::Name => entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.package_id.cmp(&b.package_id))),
        SearchSort::Updated => entries.sort_by(|a, b| b.updated().cmp(&a.updated()).then_with(|| a.package_id.cmp(&b.package_id))),
    }
}

/// The local fallback: filter and order verified index entries by the contract's rules.
pub fn search_entries(entries: &[IndexEntry], query: &SearchQuery) -> Vec<IndexEntry> {
    let mut hits: Vec<IndexEntry> = entries.iter().filter(|entry| matches_search(entry, query)).cloned().collect();
    sort_entries(&mut hits, query.sort);
    hits
}

/// One search result; actions only on a verified, unrevoked, installable hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The verified index entry, or the unsigned hosted item when the index has none.
    pub entry: IndexEntry,
    /// True when `entry` came from the verified index.
    pub verified: bool,
    /// True when a kill list revokes the listed build.
    pub revoked: bool,
}

impl SearchHit {
    /// Badges, acquire links and install actions are shown only for this.
    pub fn actionable(&self) -> bool {
        self.verified && !self.revoked && installable_access(&self.entry.access)
    }
}

/// What a search returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// Hits in result order.
    pub hits: Vec<SearchHit>,
    /// Hosted cursor for the next page.
    pub next_cursor: Option<String>,
    /// True when the endpoint's hosted search answered.
    pub hosted: bool,
    /// True when the last verified index was used.
    pub from_cache: bool,
    /// True when the index's `expires_at` has passed.
    pub stale: bool,
    /// Index entries or hosted items skipped.
    pub refused: Vec<Refusal>,
}

// ---- Catalogue endpoint (static tree or hosted registry) --------------------------------------------

/// A store-client contract v1 endpoint (static tree or hosted) with an offline cache and a persisted rollback floor.
pub struct StaticIndexSource {
    base: String,
    fetcher: Arc<dyn BytesFetcher>,
    trust: SourceTrust,
    cache_path: Option<PathBuf>,
    /// Newest index verified in this process: the rollback floor even when no cache file is configured.
    newest: std::sync::Mutex<Option<VerifiedIndex>>,
}

/// One load: the config (absent when served from cache) and the verified index.
struct Loaded {
    config: Option<CatalogConfig>,
    index: VerifiedIndex,
    from_cache: bool,
}

impl StaticIndexSource {
    /// Source over the endpoint `base` (a folder, or the http(s) prefix under which `api/v1/catalog/` is served).
    pub fn new(base: impl Into<String>, fetcher: Arc<dyn BytesFetcher>, trust: SourceTrust) -> Self {
        Self { base: base.into(), fetcher, trust, cache_path: None, newest: std::sync::Mutex::new(None) }
    }

    /// Keep the last verified index at `path` (and its `issued_at` beside it) and fall back to it when unreachable.
    pub fn with_cache(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = Some(path.into());
        self
    }

    /// A catalogue document under `{base}/api/v1/catalog/`.
    fn catalog(&self, path: &str) -> Result<String, RegistryError> {
        resolve_location(&self.base, CATALOG_PREFIX, path)
    }

    /// `config.json`, fetched and checked (protocol, schema); never probed for features.
    pub fn config(&self) -> Result<CatalogConfig, RegistryError> {
        parse_config(&self.fetcher.get(&self.catalog(CONFIG_PATH)?, DOC_MAX_BYTES)?)
    }

    /// `{cache}.issued_at`: the persisted rollback floor, kept even if the cache file is lost or replaced.
    fn floor_path(&self) -> Option<PathBuf> {
        let cache = self.cache_path.as_ref()?;
        let mut name = cache.file_name()?.to_os_string();
        name.push(".issued_at");
        Some(cache.with_file_name(name))
    }

    fn persisted_floor(&self) -> Option<DateTime<Utc>> {
        let text = std::fs::read_to_string(self.floor_path()?).ok()?;
        DateTime::parse_from_rfc3339(text.trim()).ok().map(|at| at.with_timezone(&Utc))
    }

    fn cached_index(&self) -> Option<VerifiedIndex> {
        let bytes = std::fs::read(self.cache_path.as_ref()?).ok()?;
        // The cache is re-verified: a tampered cache file is ignored like a tampered fetch.
        match verify_index(&bytes, &self.trust) {
            Ok(index) => Some(index),
            Err(error) => {
                tracing::warn!(base = %self.base, %error, "registry index cache does not verify; ignored");
                None
            }
        }
    }

    fn remembered(&self) -> Option<VerifiedIndex> {
        self.newest.lock().ok().and_then(|newest| newest.clone())
    }

    fn remember(&self, index: &VerifiedIndex) {
        if let Ok(mut newest) = self.newest.lock() {
            if newest.as_ref().is_none_or(|known| known.issued_at <= index.issued_at) {
                *newest = Some(index.clone());
            }
        }
    }

    /// Newest verified copy (cache file or in process) at or above the floor.
    fn best_known(&self, floor: Option<DateTime<Utc>>) -> Option<VerifiedIndex> {
        [self.cached_index(), self.remembered()]
            .into_iter()
            .flatten()
            .max_by_key(|index| index.issued_at)
            .filter(|index| floor.is_none_or(|floor| index.issued_at >= floor))
    }

    /// Highest `issued_at` this client has verified: persisted floor, cache file or this process.
    fn floor(&self) -> Option<DateTime<Utc>> {
        [self.persisted_floor(), self.cached_index().map(|index| index.issued_at), self.remembered().map(|index| index.issued_at)]
            .into_iter()
            .flatten()
            .max()
    }

    fn persist(&self, bytes: &[u8], index: &VerifiedIndex) {
        if let Some(cache) = &self.cache_path {
            let written = cache.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(cache, bytes));
            if let Err(error) = written {
                tracing::warn!(%error, "registry index cache not written");
            }
        }
        if let Some(floor) = self.floor_path() {
            if let Err(error) = std::fs::write(&floor, index.issued_at.to_rfc3339()) {
                tracing::warn!(%error, "registry rollback floor not written");
            }
        }
        self.remember(index);
    }

    /// The live index; the newest verified copy when the endpoint is unreachable or the live index is older than
    /// the verified floor (rollback guard), and a refusal when no copy at the floor survives.
    fn load(&self) -> Result<Loaded, RegistryError> {
        let live = self.config().and_then(|config| {
            let bytes = self.fetcher.get(&self.catalog(INDEX_PATH)?, INDEX_MAX_BYTES)?;
            Ok((config, bytes))
        });
        match live {
            Ok((config, bytes)) => {
                let index = verify_index(&bytes, &self.trust)?;
                let floor = self.floor();
                if let Some(floor) = floor.filter(|floor| index.issued_at < *floor) {
                    tracing::warn!(base = %self.base, "registry index older than the last verified copy ignored");
                    return match self.best_known(Some(floor)) {
                        Some(known) => Ok(Loaded { config: Some(config), index: known, from_cache: true }),
                        None => Err(RegistryError::IndexRollback { known: floor.to_rfc3339(), served: index.issued_at.to_rfc3339() }),
                    };
                }
                self.persist(&bytes, &index);
                Ok(Loaded { config: Some(config), index, from_cache: false })
            }
            Err(RegistryError::Transport(reason)) => match self.best_known(self.floor()) {
                Some(known) => Ok(Loaded { config: None, index: known, from_cache: true }),
                None => Err(RegistryError::Transport(reason)),
            },
            Err(other) => Err(other),
        }
    }

    /// The index entry for `package_id` and its hash-pinned, verified detail document.
    fn vouched_detail(&self, package_id: &str) -> Result<VerifiedDetail, RegistryError> {
        let loaded = self.load()?;
        let listed = loaded.index.entry(package_id).cloned().ok_or_else(|| RegistryError::NotFound(package_id.to_owned()))?;
        let bytes = self.fetcher.get(&self.catalog(&format!("{DETAIL_DIR}/{}.json", listed.package_id))?, DOC_MAX_BYTES)?;
        let mut verified = verify_detail(&bytes, &listed, loaded.config.as_ref())?;
        verified.revoked = (self.trust.is_killed)(&listed.package_id, &listed.latest_content_hash);
        Ok(verified)
    }

    /// Details for one package: versions this host can use, publisher, access and the checked acquire link.
    pub fn detail(&self, package_id: &str) -> Result<VerifiedDetail, RegistryError> {
        self.vouched_detail(package_id)
    }

    /// The version `entry` names, as the signed index and its hash-pinned detail document vouch for it.
    fn vouched_version(&self, entry: &RegistryEntry) -> Result<(IndexEntry, DetailVersion), RegistryError> {
        let verified = self.vouched_detail(&entry.package_id)?;
        if !installable_access(&verified.listed.access) {
            return Err(RegistryError::AccessUnknown { package_id: entry.package_id.clone(), access: verified.listed.access });
        }
        let version = verified
            .detail
            .versions
            .into_iter()
            .find(|version| version.version == entry.version && version.content_hash.eq_ignore_ascii_case(&entry.content_hash))
            .ok_or_else(|| RegistryError::NotFound(format!("{}@{}", entry.package_id, entry.version)))?;
        if version.yanked {
            return Err(RegistryError::Yanked(entry.package_id.clone(), version.version));
        }
        self.trust.refuse_killed(&entry.package_id, &version.content_hash)?;
        Ok((verified.listed, version))
    }

    /// Header phase of `entry`: the verified attestation (consent, license, "Reviewed" badge) before any download.
    pub fn attest(&self, entry: &RegistryEntry) -> Result<VerifiedAttestation, RegistryError> {
        let (listed, version) = self.vouched_version(entry)?;
        let location = self.catalog(&format!("{DETAIL_DIR}/{}/{}/attestation.json", entry.package_id, version.version))?;
        let bytes = match self.fetcher.get(&location, HEADER_MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(RegistryError::Transport(_) | RegistryError::NotFound(_)) => return Err(RegistryError::HeaderUnavailable),
            Err(other) => return Err(other),
        };
        let expected = ExpectedBuild {
            package_id: &entry.package_id,
            version: &version.version,
            content_hash: &version.content_hash,
            publisher_pin: Some(listed.publisher.did.as_str()),
            kind: Some(listed.kind.as_str()),
        };
        verify_attestation(&bytes, &expected, &self.trust)
    }

    /// Search: the endpoint's hosted search when `config.json` advertises it, otherwise the verified index filtered
    /// locally by the same rules. Every hosted hit is switched to its verified index entry before it is actionable.
    pub fn search(&self, query: &SearchQuery) -> Result<SearchResults, RegistryError> {
        query.check()?;
        let loaded = self.load()?;
        let stale = loaded.index.is_stale(Utc::now());
        let revoked = |entry: &IndexEntry| (self.trust.is_killed)(&entry.package_id, &entry.latest_content_hash);
        let hosted = loaded.config.as_ref().is_some_and(|config| config.has(FEATURE_SEARCH)) && !loaded.from_cache;
        if hosted {
            match self.hosted_search(query, &loaded.index) {
                Ok(mut results) => {
                    results.from_cache = loaded.from_cache;
                    results.stale = stale;
                    return Ok(results);
                }
                Err(error) => tracing::warn!(base = %self.base, %error, "hosted search failed; filtering the verified index"),
            }
        }
        let hits = search_entries(&loaded.index.entries, query)
            .into_iter()
            .map(|entry| SearchHit { revoked: revoked(&entry), entry, verified: true })
            .collect();
        Ok(SearchResults { hits, next_cursor: None, hosted: false, from_cache: loaded.from_cache, stale, refused: loaded.index.refused })
    }

    fn hosted_search(&self, query: &SearchQuery, index: &VerifiedIndex) -> Result<SearchResults, RegistryError> {
        let location = format!("{}{}", self.catalog(SEARCH_PATH)?, query.query_string());
        let bytes = self.fetcher.get(&location, SEARCH_MAX_BYTES)?;
        let page: Value = serde_json::from_slice(&bytes).map_err(|e| RegistryError::IndexMalformed(format!("search page: {e}")))?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| RegistryError::IndexMalformed("search page has no items".into()))?;
        let mut results = SearchResults {
            next_cursor: page.get("next_cursor").and_then(Value::as_str).map(str::to_owned),
            hosted: true,
            ..SearchResults::default()
        };
        for item in items.iter().take(INDEX_MAX_ENTRIES) {
            let id = item.get("package_id").and_then(Value::as_str).unwrap_or("search hit").to_owned();
            // A hit is unsigned: the verified index entry replaces it whenever one exists.
            if let Some(listed) = index.entry(&id) {
                let revoked = (self.trust.is_killed)(&listed.package_id, &listed.latest_content_hash);
                results.hits.push(SearchHit { entry: listed.clone(), verified: true, revoked });
                continue;
            }
            match serde_json::from_value::<IndexEntry>(item.clone()) {
                Ok(entry) if valid_package_id(&entry.package_id) && KNOWN_KINDS.contains(&entry.kind.as_str()) => {
                    results.hits.push(SearchHit { entry, verified: false, revoked: false });
                }
                Ok(_) => {}
                Err(error) => results.refused.push(Refusal { what: id, reason: format!("malformed search hit: {error}") }),
            }
        }
        Ok(results)
    }

    /// Verified bytes of `entry`, dispatched on the listed family; a web bundle goes to the portal install path.
    pub fn fetch_listed(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        // Only what the signed index and its pinned detail document vouch for is fetched, never a caller-supplied url.
        let (listed, version) = self.vouched_version(entry)?;
        let location = resolve_location(&self.base, &format!("{CATALOG_PREFIX}/{DETAIL_DIR}"), &version.download_url)?;
        let bytes = self.fetcher.get(&location, version.size_bytes)?;
        if bytes.len() as u64 > version.size_bytes {
            return Err(RegistryError::TooLarge { what: "package".into(), max: version.size_bytes });
        }
        let vouched = RegistryEntry {
            version: version.version,
            content_hash: version.content_hash,
            url: version.download_url,
            size: version.size_bytes,
            ..entry.clone()
        };
        if listed.kind == WEB_BUNDLE_KIND {
            return verify_web_bundle(bytes, &vouched, &listed.publisher.did, &self.trust);
        }
        let fetched = verify_fetched(bytes, &vouched, &self.trust)?;
        if fetched.package.publisher_did != listed.publisher.did {
            return Err(RegistryError::PublisherMismatch { listed: listed.publisher.did, served: fetched.package.publisher_did });
        }
        if fetched.kind() != listed.kind {
            return Err(RegistryError::KindMismatch { listed: listed.kind, served: fetched.kind().to_owned() });
        }
        Ok(fetched)
    }
}

impl RegistrySource for StaticIndexSource {
    fn label(&self) -> String {
        format!("Index: {}", self.base)
    }

    /// Extensions only (web bundles are reached through [`StaticIndexSource::search`]); revoked and
    /// not-installable entries are refusal lines.
    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        let loaded = self.load()?;
        let stale = loaded.index.is_stale(Utc::now());
        let mut refused = loaded.index.refused;
        let mut entries = Vec::new();
        for listed in loaded.index.entries.iter().filter(|listed| listed.kind == EXTENSION_KIND) {
            if !installable_access(&listed.access) {
                refused.push(Refusal {
                    what: listed.package_id.clone(),
                    reason: format!("not installable from this listing (access {:?})", listed.access),
                });
                continue;
            }
            let entry = listed.to_registry_entry();
            if matches_query(&entry, query) {
                entries.push(entry);
            }
        }
        let entries = split_killed(entries, &self.trust, &mut refused);
        Ok(Listing { entries, refused, from_cache: loaded.from_cache, stale })
    }

    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        Ok(self.attest(entry)?.extension_json)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        self.fetch_listed(entry)
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
    /// The attestation document (`package_id`, `version`, `content_hash`, `manifest_b64`, `header_members_b64`) for an entry; `None` when the registry has none.
    fn attestation(&self, _entry: &RegistryEntry) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
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

    /// Header phase over the registry's `/meta`: sealed, trusted, kill-list checked, before any download.
    pub fn attest(&self, entry: &RegistryEntry) -> Result<VerifiedAttestation, RegistryError> {
        requested_uri_matches(entry)?;
        self.check_protocol()?;
        self.trust.refuse_killed(&entry.package_id, &entry.content_hash)?;
        let Some(bytes) = self.transport.attestation(entry).map_err(RegistryError::Transport)? else {
            return Err(RegistryError::HeaderUnavailable);
        };
        let expected = ExpectedBuild {
            package_id: &entry.package_id,
            version: &entry.version,
            content_hash: &entry.content_hash,
            publisher_pin: None,
            kind: None,
        };
        verify_attestation(&bytes, &expected, &self.trust)
    }
}

/// The registry uri (`id` or `id@version`) must name the entry's own id and version (revocation-by-renaming steer).
fn requested_uri_matches(entry: &RegistryEntry) -> Result<(), RegistryError> {
    let (id, version) = match entry.url.split_once('@') {
        Some((id, version)) => (id, Some(version)),
        None => (entry.url.as_str(), None),
    };
    if id != entry.package_id || version.is_some_and(|version| version != entry.version) {
        return Err(RegistryError::PackageInvalid("id-mismatch: the registry uri names another build".into()));
    }
    Ok(())
}

impl RegistrySource for ProtocolV1Source {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        self.check_protocol()?;
        let entries = self.transport.search(query).map_err(RegistryError::Transport)?;
        let mut refused = Vec::new();
        let entries = split_killed(entries, &self.trust, &mut refused);
        Ok(Listing { entries, refused, ..Listing::default() })
    }

    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        Ok(self.attest(entry)?.extension_json)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        requested_uri_matches(entry)?;
        self.check_protocol()?;
        self.trust.refuse_killed(&entry.package_id, &entry.content_hash)?;
        let bytes = self.transport.download(entry).map_err(RegistryError::Transport)?;
        verify_fetched(bytes, entry, &self.trust)
    }
}

impl ProtocolV1Source {
    /// Download and verify any hexon type (plugin or content such as a tileset), dispatching on the manifest's type.
    /// Whole-archive and capped at [`PACKAGE_MAX_BYTES`]: large chunked tilesets keep the partial-fetch path.
    pub fn fetch_hexon(&self, entry: &RegistryEntry) -> Result<FetchedHexon, RegistryError> {
        requested_uri_matches(entry)?;
        self.check_protocol()?;
        self.trust.refuse_killed(&entry.package_id, &entry.content_hash)?;
        let bytes = self.transport.download(entry).map_err(RegistryError::Transport)?;
        verify_fetched_hexon(bytes, entry, &self.trust)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::publisher::PluginHexonBuilder;

    const WASM_COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
    const PACKAGE_ID: &str = "org.example.fleet-sim";
    const CATALOG: &str = "api/v1/catalog";

    fn did_of(key: &SigningKey) -> String {
        fe_format::verifying_key_to_did(&key.verifying_key())
    }

    fn sealed_package(id: &str, version: &str, key: &SigningKey) -> Vec<u8> {
        let extension = json!({
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
        json!({
            "schema_version": 1, "package_id": package.package_id, "version": package.version,
            "content_hash": package.content_hash, "manifest_b64": engine.encode(&manifest),
            "header_members_b64": members, "signatures": [],
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
        attestation: Option<Vec<u8>>,
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
        fn attestation(&self, _entry: &RegistryEntry) -> Result<Option<Vec<u8>>, String> {
            Ok(self.attestation.clone())
        }
    }

    /// A hosted endpoint: files from disk, plus one canned search page (the hosted-only operation).
    struct HostedStub {
        page: Vec<u8>,
    }

    impl BytesFetcher for HostedStub {
        fn get(&self, location: &str, max_bytes: u64) -> Result<Vec<u8>, RegistryError> {
            if location.contains("/search?") {
                return Ok(self.page.clone());
            }
            FileFetcher.get(location, max_bytes)
        }
    }

    /// Store-client journey over a contract v1 endpoint tree (`api/v1/catalog/…`, blobs outside it): list, search
    /// (local and hosted, hosted hits re-resolved to the verified index), header phase, fetch; offline cache, rollback
    /// (in process, cached, and against the persisted floor once the cache is gone), kill list at offer, header and
    /// fetch, tampered index and detail, refused download locations, publisher pin, misused index key, newer config
    /// schema, bad package signature and protocol mismatch each refuse distinctly.
    #[test]
    fn registry_source_journey() {
        let publisher = SigningKey::from_bytes(&[11u8; 32]);
        let index_key = SigningKey::from_bytes(&[12u8; 32]);
        let stranger = SigningKey::from_bytes(&[13u8; 32]);
        let publisher_did = did_of(&publisher);
        let root_did = publisher_did.clone();
        let killed: Arc<Mutex<HashSet<String>>> = Arc::default();
        let kill_view = killed.clone();
        let trust = SourceTrust::new(Arc::new(move |did| did == root_did))
            .with_index_pin(vec![("index-root".into(), index_key.verifying_key())])
            .with_kill_list(Arc::new(move |_, hash| kill_view.lock().unwrap().contains(hash)));
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let base = root.to_string_lossy().into_owned();

        // The endpoint: config, sealed build under blobs/, hash-pinned detail, attestation, signed index.
        let good = sealed_package(PACKAGE_ID, "1.0.0", &publisher);
        let facts = verify_plugin_package(&good, None, &trust.plugin_trust()).expect("fixture verifies");
        let config = |protocol: u64, schema: u64, features: Value| {
            serde_json::to_vec(&json!({
                "schema_version": schema, "registry_protocol": protocol, "kinds": ["extension", "web-bundle"],
                "features": features, "index_key_id": "index-root",
            }))
            .unwrap()
        };
        let base_features = json!(["index", "detail", "attestations"]);
        put(root, &format!("{CATALOG}/config.json"), &config(1, 1, base_features.clone()));
        put(root, "blobs/fleet-sim-1.0.0.hexon", &good);
        let publisher_doc = |did: &str| json!({"did": did, "name": "Example", "verified": false});
        let detail_doc = |did: &str, download_url: &str| {
            serde_json::to_vec(&json!({
                "schema_version": 1, "package_id": PACKAGE_ID, "kind": "extension", "access": "free",
                "publisher": publisher_doc(did),
                "versions": [{"version": "1.0.0", "content_hash": facts.content_hash, "size_bytes": good.len(),
                              "requires": {"fractalengine:plugin": "2.0"},
                              "download_url": download_url, "yanked": false}],
            }))
            .unwrap()
        };
        let detail = detail_doc(&publisher_did, "../../../../blobs/fleet-sim-1.0.0.hexon");
        let detail_path = format!("{CATALOG}/packages/{PACKAGE_ID}.json");
        put(root, &detail_path, &detail);
        let attested = serde_json::to_vec(&attestation(&good, &facts)).unwrap();
        put(root, &format!("{CATALOG}/packages/{PACKAGE_ID}/1.0.0/attestation.json"), &attested);
        let index_doc = |issued_at: &str, detail: &[u8], did: &str| {
            json!({
                "schema_version": 1, "issued_at": issued_at, "expires_at": "2026-11-08T00:00:01Z",
                "entries": [{"package_id": PACKAGE_ID, "kind": "extension", "name": "Fleet Sim",
                             "summary": "Simulated vehicle fleet", "publisher": publisher_doc(did),
                             "latest_version": "1.0.0", "latest_content_hash": facts.content_hash,
                             "detail_hash": fe_format::integrity::digest_hex(detail), "access": "free",
                             "categories": ["simulation"], "tags": ["fleet", "sim"],
                             "updated_at": "2026-11-07T00:00:00Z"},
                            {"package_id": "org.example.future", "kind": "theme-pack", "latest_version": "1.0.0"}],
            })
        };
        let index_path = format!("{CATALOG}/index.json");
        let signed = |doc: Value, key: &SigningKey| serde_json::to_vec(&sign_index(doc, "index-root", key).unwrap()).unwrap();
        put(root, &index_path, &signed(index_doc("2026-11-08T00:00:00Z", &detail, &publisher_did), &index_key));

        // Good endpoint: listed (stale once expires_at passed, still installable; the unknown kind is skipped),
        // previewed before download, fetched.
        let cache_path = root.join("cache/index.cache.json");
        let source = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), trust.clone()).with_cache(&cache_path);
        let listing = source.list(None).expect("good index lists");
        assert_eq!((listing.entries.len(), listing.from_cache, listing.stale), (1, false, true));
        assert!(source.list(Some("nomatch")).unwrap().entries.is_empty());
        let entry = listing.entries[0].clone();
        let header = source.attest(&entry).expect("header before download");
        assert_eq!((header.extension_json.as_slice(), header.is_reviewed()), (facts.extension_json.as_slice(), false));
        assert_eq!(source.fetch(&entry).expect("good package fetches").package.package_id, PACKAGE_ID);
        assert_eq!(source.detail(PACKAGE_ID).expect("detail verifies").acquire_url, None);
        let offline = StaticIndexSource::new(root.join("gone").to_string_lossy(), Arc::new(FileFetcher), trust.clone())
            .with_cache(&cache_path);
        assert!(offline.list(None).expect("offline falls back to the verified cache").from_cache);
        let local = LocalSource::new(root.join("blobs"), trust.clone());
        assert_eq!(local.list(None).unwrap().entries[0].content_hash, facts.content_hash);

        // Search without the hosted feature filters the verified index locally (case-insensitive q, exact tags).
        let search = |q: Option<&str>, tag: Option<&str>| {
            let query = SearchQuery { q: q.map(str::to_owned), tag: tag.map(str::to_owned), ..SearchQuery::default() };
            source.search(&query).expect("local search").hits
        };
        let hits = search(Some("VEHICLE"), Some("sim,fleet"));
        assert!(hits.len() == 1 && hits[0].actionable());
        assert!(search(None, Some("Sim")).is_empty(), "tags are case-sensitive");

        // Hosted search (advertised, never probed): a known hit becomes the verified entry, an unknown one stays inert.
        put(root, &format!("{CATALOG}/config.json"), &config(1, 1, json!(["index", "detail", "attestations", "search"])));
        let page = serde_json::to_vec(&json!({"items": [
            {"package_id": PACKAGE_ID, "kind": "extension", "name": "Spoofed", "latest_version": "9.9.9",
             "latest_content_hash": "0".repeat(64), "detail_hash": "0".repeat(64), "access": "free"},
            {"package_id": "org.example.unlisted", "kind": "extension", "name": "Unlisted", "latest_version": "1.0.0",
             "latest_content_hash": "0".repeat(64), "detail_hash": "0".repeat(64), "access": "free"}],
            "next_cursor": "c2"}))
        .unwrap();
        let hosted = StaticIndexSource::new(base.clone(), Arc::new(HostedStub { page }), trust.clone());
        let results = hosted.search(&SearchQuery { q: Some("fleet".into()), ..SearchQuery::default() }).expect("hosted search");
        assert!(results.hosted && results.next_cursor.as_deref() == Some("c2"));
        assert_eq!((results.hits[0].entry.name.as_str(), results.hits[0].actionable()), ("Fleet Sim", true));
        assert_eq!((results.hits[1].verified, results.hits[1].actionable()), (false, false));
        put(root, &format!("{CATALOG}/config.json"), &config(1, 1, base_features.clone()));

        // Kill list: the build is unavailable at offer, header and fetch; dropping the entry restores it.
        killed.lock().unwrap().insert(facts.content_hash.clone());
        let revoked = source.list(None).unwrap();
        assert!(revoked.entries.is_empty() && revoked.refused.iter().any(|refusal| refusal.reason.contains("kill list")));
        assert!(matches!(source.attest(&entry), Err(RegistryError::Revoked { .. })));
        assert!(matches!(source.fetch(&entry), Err(RegistryError::Revoked { .. })));
        assert!(!search(Some("fleet"), None)[0].actionable());
        killed.lock().unwrap().clear();

        // Rollback: an older, validly signed index is ignored in favour of the newer cached copy and, with no
        // cache file configured, of the newest copy this process verified; once the cache is gone, the persisted
        // issued_at still refuses it.
        let uncached = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), trust.clone());
        assert!(!uncached.list(None).unwrap().from_cache);
        put(root, &index_path, &signed(index_doc("2026-11-01T00:00:00Z", &detail, &publisher_did), &index_key));
        assert!(source.list(None).unwrap().from_cache, "older index ignored");
        assert!(uncached.list(None).unwrap().from_cache, "older index ignored without a cache file");
        std::fs::remove_file(&cache_path).unwrap();
        let restarted = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), trust.clone()).with_cache(&cache_path);
        assert!(matches!(restarted.list(None), Err(RegistryError::IndexRollback { .. })));

        // Tampered index: an entry altered after signing.
        let mut tampered = sign_index(index_doc("2026-11-09T00:00:00Z", &detail, &publisher_did), "index-root", &index_key).unwrap();
        tampered["entries"][0]["latest_version"] = Value::String("1.0.1".into());
        put(root, &index_path, &serde_json::to_vec(&tampered).unwrap());
        assert_eq!(source.list(None).unwrap_err(), RegistryError::IndexSignature);

        // Tampered detail document: the signed index pins its hash.
        put(root, &index_path, &signed(index_doc("2026-11-09T00:00:00Z", &detail, &publisher_did), &index_key));
        put(root, &detail_path, &[detail.as_slice(), b" ".as_slice()].concat());
        assert!(matches!(source.fetch(&entry), Err(RegistryError::DetailMismatch(_))));
        put(root, &detail_path, &detail);

        // Validly pinned details naming a UNC path, or a path climbing out of the endpoint, are refused before any read.
        for (stamp, download_url) in
            [("2026-11-09T12:00:00Z", r"\\attacker\share\fleet-sim-1.0.0.hexon"), ("2026-11-09T13:00:00Z", "../../../../../outside.hexon")]
        {
            let bad_detail = detail_doc(&publisher_did, download_url);
            put(root, &detail_path, &bad_detail);
            put(root, &index_path, &signed(index_doc(stamp, &bad_detail, &publisher_did), &index_key));
            assert!(matches!(source.fetch(&entry), Err(RegistryError::LocationRefused(_))), "{download_url}");
        }

        // A listing naming another publisher: the header phase and the download both refuse the build.
        let stranger_detail = detail_doc(&did_of(&stranger), "../../../../blobs/fleet-sim-1.0.0.hexon");
        put(root, &detail_path, &stranger_detail);
        put(root, &index_path, &signed(index_doc("2026-11-09T14:00:00Z", &stranger_detail, &did_of(&stranger)), &index_key));
        assert!(matches!(source.attest(&entry), Err(RegistryError::PublisherMismatch { .. })));
        assert!(matches!(source.fetch(&entry), Err(RegistryError::PublisherMismatch { .. })));
        put(root, &detail_path, &detail);

        // An index key that also signs packages is refused, even when pinned.
        let misused = trust.clone().with_index_pin(vec![("index-root".into(), publisher.verifying_key())]);
        put(root, &index_path, &signed(index_doc("2026-11-10T00:00:00Z", &detail, &publisher_did), &publisher));
        let misused_source = StaticIndexSource::new(base.clone(), Arc::new(FileFetcher), misused);
        assert_eq!(misused_source.list(None).unwrap_err(), RegistryError::IndexKeyMisused("index-root".into()));
        put(root, &index_path, &signed(index_doc("2026-11-10T00:00:00Z", &detail, &publisher_did), &index_key));

        // A config schema newer than this client reads is refused before the index.
        put(root, &format!("{CATALOG}/config.json"), &config(1, 2, base_features.clone()));
        assert_eq!(source.list(None).unwrap_err(), RegistryError::SchemaTooNew { document: "config.json".into(), found: 2 });
        put(root, &format!("{CATALOG}/config.json"), &config(1, 1, base_features.clone()));

        // Bad package signature: a byte flipped inside the sealed build the index vouches for.
        let mut corrupt = good.clone();
        let middle = corrupt.len() / 2;
        corrupt[middle] ^= 0xff;
        put(root, "blobs/fleet-sim-1.0.0.hexon", &corrupt);
        assert!(matches!(source.fetch(&entry), Err(RegistryError::PackageInvalid(_))));

        // Protocol mismatch: refused before the index is read, by the endpoint and by a protocol-v1 registry.
        put(root, &format!("{CATALOG}/config.json"), &config(2, 1, base_features));
        assert_eq!(source.list(None).unwrap_err(), RegistryError::ProtocolMismatch { found: Some("2".into()) });
        let v1 = |protocol: &str| FakeV1 { protocol: Some(protocol.into()), bytes: good.clone(), attestation: Some(attested.clone()) };
        let mismatched = ProtocolV1Source::new("v2 registry", Arc::new(v1("2")), trust.clone());
        assert_eq!(mismatched.list(None).unwrap_err(), RegistryError::ProtocolMismatch { found: Some("2".into()) });
        let matched = ProtocolV1Source::new("v1 registry", Arc::new(v1("1")), trust);
        let v1_entry = entry_for(&facts, format!("{PACKAGE_ID}@1.0.0"), 0);
        // G4-C14: the protocol-v1 header phase renders consent before the download.
        assert_eq!(matched.header(&v1_entry).expect("v1 header"), facts.extension_json);
        assert_eq!(matched.fetch(&v1_entry).unwrap().package.version, "1.0.0");
        // A request uri naming another build is refused before any download (revocation-by-renaming steer).
        let renamed = RegistryEntry { url: "org.example.other@1.0.0".into(), ..v1_entry };
        assert!(matches!(matched.fetch(&renamed), Err(RegistryError::PackageInvalid(_))));
    }

    fn listed(id: &str, name: &str, summary: &str, tags: &[&str], category: &str, updated_at: Option<&str>) -> IndexEntry {
        IndexEntry {
            package_id: id.into(),
            kind: if category == "portal" { WEB_BUNDLE_KIND.into() } else { EXTENSION_KIND.into() },
            name: name.into(),
            summary: summary.into(),
            publisher: Publisher::default(),
            latest_version: "1.0.0".into(),
            latest_content_hash: "0".repeat(64),
            detail_hash: "0".repeat(64),
            access: ACCESS_FREE.into(),
            categories: vec![category.into()],
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            icon_url: None,
            updated_at: updated_at.map(str::to_owned),
        }
    }

    /// Pure store-client rules, one table each: location resolution, caret requirements, search filter and order,
    /// config parsing, acquire links.
    #[test]
    fn store_client_rules_table() {
        let docs = "api/v1/catalog/packages";
        let resolutions: &[(&str, &str, Option<&str>)] = &[
            ("C:/ep", "x.json", Some("C:/ep/api/v1/catalog/packages/x.json")),
            ("C:/ep", "../../../../blobs/a.hexon", Some("C:/ep/blobs/a.hexon")),
            ("C:/ep", "../../../../../a.hexon", None),
            ("C:/ep", "/blobs/a.hexon", None),
            ("C:/ep", r"\\host\share\a.hexon", None),
            ("C:/ep", "C:/elsewhere/a.hexon", None),
            ("C:/ep", "%2e%2e/a.hexon", None),
            ("C:/ep", "a.hexon?x=1", None),
            ("https://cdn.example/store", "../../../../blobs/a.hexon", Some("https://cdn.example/store/blobs/a.hexon")),
            ("https://cdn.example/store", "/store/blobs/a.hexon", Some("https://cdn.example/store/blobs/a.hexon")),
            ("https://cdn.example/store", "/other/a.hexon", None),
            ("https://cdn.example/store", "../../../../../a.hexon", None),
            ("https://cdn.example/store", "https://mirror.example/a.hexon", Some("https://mirror.example/a.hexon")),
            ("https://cdn.example/store", "https://user:pw@mirror.example/a.hexon", None),
            ("https://cdn.example/store", "ftp://mirror.example/a.hexon", None),
        ];
        for (base, location, expected) in resolutions {
            assert_eq!(resolve_location(base, docs, location).ok().as_deref(), *expected, "{base} + {location}");
        }

        let carets: &[(&str, (u64, u64, u64), bool)] = &[
            ("2.0", (2, 0, 0), true),
            ("^2", (2, 5, 1), true),
            ("2.1", (2, 0, 0), false),
            ("1", (2, 0, 0), false),
            ("0.3", (0, 3, 9), true),
            ("0.3", (0, 4, 0), false),
            ("0.0.4", (0, 0, 4), true),
            ("0.0.4", (0, 0, 5), false),
            (">=2", (2, 0, 0), false),
            ("02.0", (2, 0, 0), false),
        ];
        for (requirement, version, expected) in carets {
            assert_eq!(caret_matches(requirement, *version), *expected, "{requirement} vs {version:?}");
        }
        assert!(!requires_satisfied(&BTreeMap::from([("fractalengine:unknown".to_owned(), "1".to_owned())])));

        let entries = [
            listed("b.fleet", "Fleet", "Vehicles", &["sim", "fleet"], "simulation", Some("2026-11-02T00:00:00Z")),
            listed("a.fleet", "Fleet", "Trucks", &["sim"], "simulation", Some("2026-11-02T00:00:00Z")),
            listed("c.portal", "Atlas", "Map viewer", &["Map"], "portal", Some("2026-11-05T00:00:00Z")),
            listed("d.undated", "Zeta", "Ünïcode Fleet", &[], "simulation", None),
        ];
        let query = |q: Option<&str>, kind: Option<&str>, tag: Option<&str>, sort: SearchSort| SearchQuery {
            q: q.map(str::to_owned),
            kind: kind.map(str::to_owned),
            tag: tag.map(str::to_owned),
            sort,
            ..SearchQuery::default()
        };
        let searches: &[(SearchQuery, &[&str])] = &[
            (query(None, None, None, SearchSort::Updated), &["c.portal", "a.fleet", "b.fleet", "d.undated"]),
            (query(None, None, None, SearchSort::Name), &["c.portal", "a.fleet", "b.fleet", "d.undated"]),
            (query(Some("FLEET"), None, None, SearchSort::Updated), &["a.fleet", "b.fleet", "d.undated"]),
            (query(Some("ünïcode"), None, None, SearchSort::Updated), &["d.undated"]),
            (query(None, Some(WEB_BUNDLE_KIND), None, SearchSort::Updated), &["c.portal"]),
            (query(None, None, Some("fleet,sim"), SearchSort::Updated), &["b.fleet"]),
            (query(None, None, Some("map"), SearchSort::Updated), &[]),
        ];
        for (search, expected) in searches {
            let ids: Vec<String> = search_entries(&entries, search).into_iter().map(|entry| entry.package_id).collect();
            assert_eq!(ids, *expected, "{search:?}");
        }
        assert_eq!(query(Some("a b"), None, None, SearchSort::Name).query_string(), "?sort=name&q=a%20b");
        let too_long = SearchQuery { q: Some("x".repeat(SEARCH_PARAM_MAX_CHARS + 1)), ..SearchQuery::default() };
        assert!(matches!(too_long.check(), Err(RegistryError::InvalidQuery(_))));

        let configs: &[(&str, Result<bool, RegistryError>)] = &[
            (r#"{"schema_version":1,"registry_protocol":1,"features":["index","search"]}"#, Ok(true)),
            (r#"{"registry_protocol":"1","features":["index"]}"#, Ok(false)),
            (r#"{"schema_version":1,"registry_protocol":2}"#, Err(RegistryError::ProtocolMismatch { found: Some("2".into()) })),
            (
                r#"{"schema_version":3,"registry_protocol":1}"#,
                Err(RegistryError::SchemaTooNew { document: "config.json".into(), found: 3 }),
            ),
        ];
        for (text, expected) in configs {
            assert_eq!(&parse_config(text.as_bytes()).map(|config| config.has(FEATURE_SEARCH)), expected, "{text}");
        }

        let (licensee, device) = ("did:key:z6MkA", "did:key:z6LSb");
        let links: &[(&str, Option<&str>)] = &[
            ("https://shop.example/get", Some("https://shop.example/get?licensee_did=did%3Akey%3Az6MkA&device_x25519_did=did%3Akey%3Az6LSb")),
            ("https://shop.example/get?p=1#t", Some("https://shop.example/get?p=1&licensee_did=did%3Akey%3Az6MkA&device_x25519_did=did%3Akey%3Az6LSb#t")),
            ("http://shop.example/get", None),
            ("https://user@shop.example/get", None),
        ];
        for (url, expected) in links {
            assert_eq!(acquire_link(url, licensee, device).ok().as_deref(), *expected, "{url}");
        }
        let entitled = CatalogConfig { features: [FEATURE_ENTITLEMENTS.to_owned()].into(), ..CatalogConfig::default() };
        let detail = |access: &str, url: &str| PackageDetail {
            schema_version: 1,
            package_id: "a.fleet".into(),
            kind: EXTENSION_KIND.into(),
            name: String::new(),
            description: String::new(),
            publisher: Publisher::default(),
            access: access.into(),
            acquire_url: Some(url.into()),
            categories: Vec::new(),
            tags: Vec::new(),
            links: Value::Null,
            versions: Vec::new(),
        };
        let acquire: &[(PackageDetail, Option<&CatalogConfig>, bool)] = &[
            (detail(ACCESS_LICENSED, "https://shop.example/get"), Some(&entitled), true),
            (detail(ACCESS_LICENSED, "https://shop.example/get"), None, false),
            (detail(ACCESS_LICENSED, "http://shop.example/get"), Some(&entitled), false),
            (detail(ACCESS_FREE, "https://shop.example/get"), Some(&entitled), false),
        ];
        for (doc, config, offered) in acquire {
            assert_eq!(checked_acquire_url(doc, *config).is_some(), *offered, "{doc:?}");
        }
    }
}
