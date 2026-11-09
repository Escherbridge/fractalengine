//! `RegistrySource`: one trait for finding and fetching extension packages (Local, static signed index, protocol v1).
//! Rationale and trust model: `docs/extensions/extensions-ui.md` section Registry sources; spec R29/R32.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::package::{verify_plugin_package, PluginPackage, PluginPackageError, PluginTrust};

/// Index document schema this build reads.
pub const INDEX_SCHEMA_VERSION: u32 = 1;
/// Protocol version string a protocol-v1 registry must advertise.
pub const PROTOCOL_V1: &str = "1";
/// Largest index document accepted.
pub const INDEX_MAX_BYTES: u64 = 1024 * 1024;
/// Largest package accepted from any source.
pub const PACKAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest `plugin/extension.json` header read from a header document.
pub const HEADER_MAX_BYTES: u64 = 256 * 1024;

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
    /// True when the live fetch failed and the last verified cached index was used.
    pub from_cache: bool,
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
    IndexSchema(u32),
    #[error("index signer '{0}' is not a trusted key")]
    IndexSignerUnknown(String),
    #[error("index signature does not verify: the index was altered or signed by another key")]
    IndexSignature,
    #[error("package '{0}' is not offered by this source")]
    NotFound(String),
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
    /// Verifying key of an index-signing trust root, by key id.
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

// ---- Static signed index ----------------------------------------------------------------------

/// The `index.json` document: entries signed by an index-signing trust root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedIndex {
    /// Always [`INDEX_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// RFC 3339 UTC issue time (shown in Browse as the index age).
    pub issued_at: String,
    /// Offered packages.
    pub entries: Vec<RegistryEntry>,
    /// Trust-root key id that signed.
    pub signer_key_id: String,
    /// Hex ed25519 signature over [`SignedIndex::signed_bytes`].
    pub signature: String,
}

impl SignedIndex {
    /// Canonical (sorted-key, compact) JSON of `{entries, issued_at, schema_version}`, the revocation list's rule.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let value = serde_json::json!({
            "entries": self.entries,
            "issued_at": self.issued_at,
            "schema_version": self.schema_version,
        });
        let mut out = String::new();
        write_canonical(&value, &mut out);
        out.into_bytes()
    }

    /// Build and sign an index (publisher tooling and tests).
    pub fn sign(entries: Vec<RegistryEntry>, issued_at: impl Into<String>, signer_key_id: impl Into<String>, key: &SigningKey) -> Self {
        let mut index = Self {
            schema_version: INDEX_SCHEMA_VERSION,
            issued_at: issued_at.into(),
            entries,
            signer_key_id: signer_key_id.into(),
            signature: String::new(),
        };
        let signature: Signature = key.sign(&index.signed_bytes());
        index.signature = encode_hex(&signature.to_bytes());
        index
    }

    /// Parse and verify index bytes against the trusted index signers.
    pub fn parse_verified(bytes: &[u8], trust: &SourceTrust) -> Result<Self, RegistryError> {
        if bytes.len() as u64 > INDEX_MAX_BYTES {
            return Err(RegistryError::TooLarge { what: "index".into(), max: INDEX_MAX_BYTES });
        }
        let index: SignedIndex = serde_json::from_slice(bytes).map_err(|e| RegistryError::IndexMalformed(e.to_string()))?;
        if index.schema_version != INDEX_SCHEMA_VERSION {
            return Err(RegistryError::IndexSchema(index.schema_version));
        }
        let key = (trust.index_signer)(&index.signer_key_id).ok_or_else(|| RegistryError::IndexSignerUnknown(index.signer_key_id.clone()))?;
        let signature = decode_hex(&index.signature)
            .and_then(|raw| Signature::from_slice(&raw).ok())
            .ok_or(RegistryError::IndexSignature)?;
        key.verify(&index.signed_bytes(), &signature).map_err(|_| RegistryError::IndexSignature)?;
        Ok(index)
    }
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (position, key) in keys.into_iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
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
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok()).collect()
}

/// Reads bytes from a location; the binary supplies an http(s) implementation, [`FileFetcher`] serves paths.
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

/// An `index.json` signed by a trust root, with an offline cache of the last verified copy.
pub struct StaticIndexSource {
    index_location: String,
    fetcher: Arc<dyn BytesFetcher>,
    trust: SourceTrust,
    cache_path: Option<PathBuf>,
}

impl StaticIndexSource {
    /// Source reading `index_location` through `fetcher`.
    pub fn new(index_location: impl Into<String>, fetcher: Arc<dyn BytesFetcher>, trust: SourceTrust) -> Self {
        Self { index_location: index_location.into(), fetcher, trust, cache_path: None }
    }

    /// Keep the last verified index at `path` and fall back to it when the registry is unreachable.
    pub fn with_cache(mut self, path: impl Into<PathBuf>) -> Self {
        self.cache_path = Some(path.into());
        self
    }

    fn resolve(&self, url: &str) -> String {
        if url.contains("://") || Path::new(url).is_absolute() {
            return url.to_owned();
        }
        match self.index_location.rfind(['/', '\\']) {
            Some(cut) => format!("{}/{}", &self.index_location[..cut], url),
            None => url.to_owned(),
        }
    }

    fn load_index(&self) -> Result<(SignedIndex, bool), RegistryError> {
        match self.fetcher.get(&self.index_location, INDEX_MAX_BYTES) {
            Ok(bytes) => {
                let index = SignedIndex::parse_verified(&bytes, &self.trust)?;
                if let Some(cache) = &self.cache_path {
                    if let Err(error) = std::fs::write(cache, &bytes) {
                        tracing::warn!(%error, "registry index cache not written");
                    }
                }
                Ok((index, false))
            }
            Err(RegistryError::Transport(reason)) => {
                let cached = self.cache_path.as_ref().and_then(|path| std::fs::read(path).ok());
                match cached {
                    // The cache is re-verified: a tampered cache file is refused like a tampered fetch.
                    Some(bytes) => Ok((SignedIndex::parse_verified(&bytes, &self.trust)?, true)),
                    None => Err(RegistryError::Transport(reason)),
                }
            }
            Err(other) => Err(other),
        }
    }
}

impl RegistrySource for StaticIndexSource {
    fn label(&self) -> String {
        format!("Index: {}", self.index_location)
    }

    fn list(&self, query: Option<&str>) -> Result<Listing, RegistryError> {
        let (index, from_cache) = self.load_index()?;
        let entries = index.entries.into_iter().filter(|entry| matches_query(entry, query)).collect();
        Ok(Listing { entries, refused: Vec::new(), from_cache })
    }

    fn header(&self, entry: &RegistryEntry) -> Result<Vec<u8>, RegistryError> {
        let url = entry.header_url.as_deref().ok_or(RegistryError::HeaderUnavailable)?;
        self.fetcher.get(&self.resolve(url), HEADER_MAX_BYTES)
    }

    fn fetch(&self, entry: &RegistryEntry) -> Result<FetchedPackage, RegistryError> {
        // Only entries the verified index lists may be fetched, never an arbitrary caller-supplied url.
        let (index, _) = self.load_index()?;
        let listed = index
            .entries
            .iter()
            .find(|listed| listed.package_id == entry.package_id && listed.version == entry.version && listed.content_hash == entry.content_hash)
            .ok_or_else(|| RegistryError::NotFound(entry.package_id.clone()))?;
        let bytes = self.fetcher.get(&self.resolve(&listed.url), PACKAGE_MAX_BYTES)?;
        verify_fetched(bytes, listed, &self.trust)
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
        Ok(Listing { entries, refused: Vec::new(), from_cache: false })
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
    use super::*;
    use crate::publisher::PluginHexonBuilder;

    const WASM_COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    fn did_of(key: &SigningKey) -> String {
        fe_identity::did_key::did_key_from_public_key(&key.verifying_key())
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

    /// Browse journey: good index lists and fetches; tampered index, bad package signature and protocol mismatch each refuse distinctly.
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
        let good = sealed_package("org.example.fleet-sim", "1.0.0", &publisher);
        std::fs::write(dir.path().join("fleet-sim.hexon"), &good).unwrap();
        let facts = verify_plugin_package(&good, None, &trust.plugin_trust()).expect("fixture verifies");
        let entry = entry_for(&facts, "fleet-sim.hexon".into(), good.len() as u64);

        // Good index: listed, searched, fetched and verified; cache serves an offline listing.
        let index = SignedIndex::sign(vec![entry.clone()], "2026-11-08T00:00:00Z", "index-root", &index_key);
        let index_path = dir.path().join("index.json");
        std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
        let cache_path = dir.path().join("index.cache.json");
        let source = StaticIndexSource::new(index_path.to_string_lossy(), Arc::new(FileFetcher), trust.clone()).with_cache(&cache_path);
        let listing = source.list(None).expect("good index lists");
        assert_eq!(listing.entries.len(), 1);
        assert!(!listing.from_cache);
        assert!(source.list(Some("nomatch")).unwrap().entries.is_empty());
        let fetched = source.fetch(&listing.entries[0]).expect("good package fetches");
        assert_eq!(fetched.package.package_id, "org.example.fleet-sim");
        let offline = StaticIndexSource::new(dir.path().join("gone.json").to_string_lossy(), Arc::new(FileFetcher), trust.clone()).with_cache(&cache_path);
        assert!(offline.list(None).expect("offline falls back to cache").from_cache);

        // Local folder lists the same package.
        let local = LocalSource::new(dir.path(), trust.clone());
        assert_eq!(local.list(None).unwrap().entries[0].content_hash, entry.content_hash);

        // Tampered index: entry altered after signing.
        let mut tampered = index.clone();
        tampered.entries[0].size += 1;
        let tampered_path = dir.path().join("tampered.json");
        std::fs::write(&tampered_path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        let refused = StaticIndexSource::new(tampered_path.to_string_lossy(), Arc::new(FileFetcher), trust.clone());
        assert_eq!(refused.list(None).unwrap_err(), RegistryError::IndexSignature);

        // Bad package signature: a byte flipped inside the sealed archive.
        let mut corrupt = good.clone();
        let middle = corrupt.len() / 2;
        corrupt[middle] ^= 0xff;
        std::fs::write(dir.path().join("corrupt.hexon"), &corrupt).unwrap();
        let mut corrupt_entry = entry.clone();
        corrupt_entry.url = "corrupt.hexon".into();
        let bad_index = SignedIndex::sign(vec![corrupt_entry.clone()], "2026-11-08T00:00:00Z", "index-root", &index_key);
        let bad_path = dir.path().join("bad.json");
        std::fs::write(&bad_path, serde_json::to_vec(&bad_index).unwrap()).unwrap();
        let bad_source = StaticIndexSource::new(bad_path.to_string_lossy(), Arc::new(FileFetcher), trust.clone());
        assert!(matches!(bad_source.fetch(&corrupt_entry), Err(RegistryError::PackageInvalid(_))));

        // Protocol mismatch: refused before any bytes are trusted.
        let mismatched = ProtocolV1Source::new("v2 registry", Arc::new(FakeV1 { protocol: Some("2".into()), bytes: good.clone() }), trust.clone());
        assert_eq!(mismatched.list(None).unwrap_err(), RegistryError::ProtocolMismatch { found: Some("2".into()) });
        let matched = ProtocolV1Source::new("v1 registry", Arc::new(FakeV1 { protocol: Some("1".into()), bytes: good }), trust);
        assert_eq!(matched.fetch(&entry).unwrap().package.version, "1.0.0");
    }
}
