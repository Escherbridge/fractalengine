use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fe_format::entitlement::{unix_now, EntitlementVerifier};
use fe_format::split_hexon_uri;
use fe_runtime::messages::DbCommand;
use tracing::{info, warn};

use crate::entitlement_client::Licensee;
use crate::entitlement_gate::{archive_content_hash, EntitlementGate, GateRefusal, LicenseTerms};
use crate::manifest::{AssetEntry, EntryKind, HexonManifest, HexonType, InstalledHexon};
use crate::package::{hexon_uri, HexonPackage, HexonPackageData};
use crate::signature;

/// Overrides [`FsBlobStore::default_path`]'s `data/blobs` default — identical
/// name and precedence to fe-sync's own copy (OF-1/S1b).
const FE_BLOB_DIR_ENV: &str = "FE_BLOB_DIR";

/// Error types for registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("signature verification failed for crate {0}")]
    SignatureVerificationFailed(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("crate already installed: {0}")]
    AlreadyInstalled(String),

    #[error("crate not found: {0}")]
    NotFound(String),

    #[error("missing asset blob for hash {0}")]
    MissingAssetBlob(String),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("package error: {0}")]
    Package(#[from] anyhow::Error),
}

/// Why [`HexonRegistry::load_entry`] returned no bytes.
#[derive(Debug, thiserror::Error)]
pub enum LoadEntryError {
    /// The petal holds no install of this build.
    #[error("crate {0} is not installed in this petal")]
    NotInstalled(String),
    /// The gate was opened for other licence terms than this install's (fail closed).
    #[error("entitlement gate does not belong to crate {0}")]
    GateMismatch(String),
    /// The entitlement gate refused the entry (unknown, missing, needs an entitlement, does not open).
    #[error(transparent)]
    Gate(#[from] GateRefusal),
    /// The blob store failed to read the entry's blob.
    #[error(transparent)]
    Store(#[from] RegistryError),
}

impl LoadEntryError {
    /// True when the entry needs an entitlement the caller does not hold (a state, not a fault).
    pub fn is_entitlement_required(&self) -> bool {
        matches!(self, Self::Gate(GateRefusal::EntitlementRequired { .. }) | Self::GateMismatch(_))
    }
}

/// One install record plus what entitlement needs from install time (spec §4.5).
#[derive(Debug, Clone)]
struct InstallRecord {
    hexon: InstalledHexon,
    /// `license.json` terms, computed once at install.
    terms: LicenseTerms,
    /// The build's `content_hash` recomputed from its own archive; `None` when installed from parsed data only.
    content_hash: Option<String>,
}

/// Content-addressed blob store for asset files, sharing the runtime store's
/// canonical `{root}/{hex[0..2]}/{hex}` layout so either side reads the other's
/// blobs (fe-hexon/src/AGENTS.md §blob-store).
pub struct FsBlobStore {
    base_dir: PathBuf,
}

impl FsBlobStore {
    /// Create a new blob store rooted at `base_dir`.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Store a blob if it doesn't already exist. Returns true if newly stored.
    /// Refuses a non-BLAKE3-hex `hash` or one that does not match `data`.
    pub fn store(&self, hash: &str, data: &[u8]) -> Result<bool, RegistryError> {
        let path = self.blob_path(hash)?;
        if blake3::hash(data).to_hex().as_str() != hash.to_ascii_lowercase() {
            return Err(RegistryError::Package(anyhow::anyhow!(
                "blob hash mismatch: data does not hash to {hash}"
            )));
        }
        if path.exists() {
            return Ok(false); // already exists (shared content addressing)
        }
        let shard_dir = path.parent().expect("canonical blob path has a parent");
        std::fs::create_dir_all(shard_dir)?;
        // Atomic write (tmp + same-directory rename), as fe-sync's FsBlobStore.
        let tmp_path = shard_dir.join(format!("{}.tmp", hash.to_ascii_lowercase()));
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(data)?;
            file.sync_all()?;
        }
        match std::fs::rename(&tmp_path, &path) {
            Ok(()) => Ok(true),
            Err(_) if path.exists() => {
                let _ = std::fs::remove_file(&tmp_path);
                Ok(false)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp_path);
                Err(RegistryError::Io(error))
            }
        }
    }

    /// Check if a blob exists (`false` for an invalid hash).
    pub fn has(&self, hash: &str) -> bool {
        self.blob_path(hash).is_ok_and(|path| path.exists())
    }

    /// Read a blob by hash.
    pub fn load(&self, hash: &str) -> Result<Vec<u8>, RegistryError> {
        std::fs::read(self.blob_path(hash)?).map_err(RegistryError::Io)
    }

    /// Canonical path for a hex BLAKE3 hash; errors on a non-64-hex hash.
    fn blob_path(&self, hash: &str) -> Result<PathBuf, RegistryError> {
        let parsed = fe_runtime::blob_store::hash_from_hex(hash)
            .map_err(|_| RegistryError::Package(anyhow::anyhow!("invalid blob hash")))?;
        Ok(fe_runtime::blob_store::canonical_blob_path(
            &self.base_dir,
            &parsed,
        ))
    }

    /// Default location: the runtime blob root, co-located with this
    /// install's own DB at `data/blobs` (CWD-relative), overridable by
    /// `FE_BLOB_DIR` — identical logic to fe-sync's `FsBlobStore::default_path`
    /// so the two independently-maintained copies never re-diverge (OF-1/S1b,
    /// fe-hexon/src/AGENTS.md §blob-store).
    pub fn default_path() -> PathBuf {
        if let Ok(dir) = std::env::var(FE_BLOB_DIR_ENV) {
            return PathBuf::from(dir);
        }
        PathBuf::from("data").join(fe_runtime::blob_store::DEFAULT_BLOB_DIR_NAME)
    }

    /// Pre-2026-10-14 per-user default, kept only so existing callers of a
    /// literal `dirs::data_local_dir()` path can be compared in tests
    /// (migration itself lives in `fe-database::handlers::blob_migration`,
    /// which reads fe-sync's `legacy_default_path` — fe-hexon has no
    /// migration pass of its own, since both stores share one on-disk root).
    pub fn legacy_default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fractalengine")
            .join(fe_runtime::blob_store::DEFAULT_BLOB_DIR_NAME)
    }

    /// Convenience constructor using [`Self::default_path`].
    pub fn open_default() -> Self {
        Self::new(Self::default_path())
    }
}

/// The petal binding that marks a hexon's published (registry-level) record.
pub const PUBLISHED_BINDING: &str = "global";

/// Local hexon registry: one install record per `(petal_id, hexon_uri)`, blobs shared by content address.
///
/// Can be used as a Bevy Resource; see `src/AGENTS.md` §authz for the petal-keyed install model.
pub struct HexonRegistry {
    /// Install records keyed by petal id, then `{hexon_id}@{version}`.
    installed: BTreeMap<String, BTreeMap<String, InstallRecord>>,
    /// Content-addressed blob storage.
    blob_store: FsBlobStore,
    /// DB command sender for persisting registry state.
    db_tx: Option<crossbeam::channel::Sender<DbCommand>>,
}

impl HexonRegistry {
    /// Create a new registry with the given blob store directory.
    pub fn new(blob_store_dir: PathBuf) -> Self {
        Self {
            installed: BTreeMap::new(),
            blob_store: FsBlobStore::new(blob_store_dir),
            db_tx: None,
        }
    }

    /// Attach a DB command channel for persisting registry entries.
    pub fn with_db_channel(mut self, tx: crossbeam::channel::Sender<DbCommand>) -> Self {
        self.db_tx = Some(tx);
        self
    }

    // -----------------------------------------------------------------------
    // Install / Uninstall
    // -----------------------------------------------------------------------

    /// Install `.hexon` archive bytes into `petal_id`, keeping the build's recomputed `content_hash` for entitlement.
    pub fn install_archive(
        &mut self,
        archive: &[u8],
        petal_id: &str,
    ) -> Result<InstalledHexon, RegistryError> {
        let package = HexonPackage::open(archive)?;
        let content_hash = archive_content_hash(archive)?;
        self.install_with_content_hash(package, petal_id, Some(content_hash))
    }

    /// Install a verified package into `petal_id`: check every blob, store them, persist, run the type handler.
    pub fn install(
        &mut self,
        package: HexonPackageData,
        petal_id: &str,
    ) -> Result<InstalledHexon, RegistryError> {
        self.install_with_content_hash(package, petal_id, None)
    }

    /// [`install`](Self::install) recording the archive's `content_hash` (only ever recomputed from archive bytes).
    fn install_with_content_hash(
        &mut self,
        package: HexonPackageData,
        petal_id: &str,
        content_hash: Option<String>,
    ) -> Result<InstalledHexon, RegistryError> {
        let uri = hexon_uri(&package.manifest);
        // `signature_verified` was established by open() over the stored bytes; re-serializing would differ.
        if !package.signature_verified {
            return Err(RegistryError::SignatureVerificationFailed(uri));
        }
        if self.get_in_petal(&uri, petal_id).is_some() {
            return Err(RegistryError::AlreadyInstalled(uri));
        }
        // Script, PetalSnapshot and Plugin have their own install paths: refuse before any blob or row write.
        crate::handlers::check_content_installable(&package.manifest.hexon_type)
            .map_err(RegistryError::Package)?;

        // Every catalogued blob must be present and hash to its asset_hash before ANY is stored
        // (D-76 (3); see src/AGENTS.md §blob-store).
        for entry in &package.entries {
            let Some(data) = package.assets.get(&entry.asset_hash) else {
                return Err(RegistryError::MissingAssetBlob(entry.asset_hash.clone()));
            };
            if signature::asset_hash(data) != entry.asset_hash.to_ascii_lowercase() {
                return Err(RegistryError::Package(anyhow::anyhow!(
                    "blob hash mismatch: entry {} does not hash to {}",
                    entry.entry_id,
                    entry.asset_hash
                )));
            }
        }

        for entry in &package.entries {
            let data = &package.assets[&entry.asset_hash];
            if self.blob_store.store(&entry.asset_hash, data)? {
                info!(
                    "Stored new asset blob {} for entry {} in hexon {}",
                    entry.asset_hash, entry.entry_id, uri
                );
            } else {
                info!(
                    "Asset blob {} already exists (shared), skipping for entry {}",
                    entry.asset_hash, entry.entry_id
                );
            }
        }

        if let Some(tx) = &self.db_tx {
            persist_install_to_db(&package.manifest, &package.entries, petal_id, tx);
        }

        let installed = InstalledHexon {
            hexon_uri: uri.clone(),
            manifest: package.manifest.clone(),
            entries: package.entries.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            petal_id: petal_id.to_string(),
            signature_valid: true,
            license: package.license.clone(),
        };

        // Type-specific post-install actions (terrain, model, ...) via handlers.
        match crate::handlers::handle_install(&package, petal_id, &self.blob_store) {
            Ok(result) => info!("{}", result.summary),
            Err(error) => warn!("Handler post-install failed for {}: {}", uri, error),
        }

        let terms = package
            .license
            .as_ref()
            .map_or_else(LicenseTerms::unlicensed, LicenseTerms::from_format_license);
        self.insert_record(InstallRecord { hexon: installed.clone(), terms, content_hash });
        Ok(installed)
    }

    /// Bind the published record of `hexon_uri` to the `content_hash` recomputed from its archive bytes;
    /// refuses an archive whose signed manifest (integrity root included) is not the recorded one.
    pub fn bind_published_archive(&mut self, hexon_uri: &str, archive: &[u8]) -> Result<(), RegistryError> {
        let opened = HexonPackage::open(archive)?;
        let content_hash = archive_content_hash(archive)?;
        let record = self
            .installed
            .get_mut(PUBLISHED_BINDING)
            .and_then(|records| records.get_mut(hexon_uri))
            .ok_or_else(|| RegistryError::NotFound(hexon_uri.to_string()))?;
        let same_build = opened.signature_verified
            && opened.hexon_uri() == hexon_uri
            && serde_json::to_value(&opened.manifest).ok().is_some_and(|opened_manifest| {
                serde_json::to_value(&record.hexon.manifest).ok() == Some(opened_manifest)
            });
        if !same_build {
            return Err(RegistryError::PermissionDenied(format!(
                "archive is not the published build {hexon_uri}"
            )));
        }
        record.content_hash = Some(content_hash);
        Ok(())
    }

    /// Associate an already-published hexon with `petal_id`; its blobs are reused, never re-stored.
    pub fn install_published(
        &mut self,
        hexon_uri: &str,
        petal_id: &str,
    ) -> Result<InstalledHexon, RegistryError> {
        if petal_id == PUBLISHED_BINDING {
            return Err(RegistryError::PermissionDenied(format!(
                "the {PUBLISHED_BINDING} binding is written only by publish"
            )));
        }
        let (hexon_id, version) = split_hexon_uri(hexon_uri);
        if hexon_id.is_empty() || version.is_none_or(str::is_empty) {
            return Err(RegistryError::NotFound(hexon_uri.to_string()));
        }
        let published = self
            .record(hexon_uri, PUBLISHED_BINDING)
            .ok_or_else(|| RegistryError::NotFound(hexon_uri.to_string()))?;
        if self.get_in_petal(hexon_uri, petal_id).is_some() {
            return Err(RegistryError::AlreadyInstalled(hexon_uri.to_string()));
        }
        // The publish install stored every blob; refuse if one has since gone missing.
        for entry in &published.hexon.entries {
            let sub_asset_hashes = entry.sub_assets.iter().flat_map(|sub_assets| sub_assets.values());
            for hash in std::iter::once(&entry.asset_hash).chain(sub_asset_hashes) {
                if !self.blob_store.has(hash) {
                    return Err(RegistryError::MissingAssetBlob(hash.clone()));
                }
            }
        }

        let installed = InstalledHexon {
            installed_at: chrono::Utc::now().to_rfc3339(),
            petal_id: petal_id.to_string(),
            ..published.hexon.clone()
        };
        let record = InstallRecord {
            hexon: installed.clone(),
            terms: published.terms.clone(),
            content_hash: published.content_hash.clone(),
        };
        if let Some(tx) = &self.db_tx {
            persist_install_to_db(&installed.manifest, &installed.entries, petal_id, tx);
        }
        info!("Installed published hexon {} into petal {}", hexon_uri, petal_id);
        self.insert_record(record);
        Ok(installed)
    }

    /// Remove the `(hexon_uri, petal_id)` record; blobs stay (shared content addressing).
    pub fn uninstall(&mut self, hexon_uri: &str, petal_id: &str) -> Result<(), RegistryError> {
        let removed = self
            .installed
            .get_mut(petal_id)
            .and_then(|petal_records| petal_records.remove(hexon_uri));
        if removed.is_none() {
            return Err(RegistryError::NotFound(hexon_uri.to_string()));
        }
        if self.installed.get(petal_id).is_some_and(BTreeMap::is_empty) {
            self.installed.remove(petal_id);
        }

        if let Some(tx) = &self.db_tx {
            let cmd = DbCommand::UninstallCrate {
                hexon_uri: hexon_uri.to_string(),
                petal_id: Some(petal_id.to_string()),
            };
            if tx.send(cmd).is_err() {
                warn!("Failed to send UninstallCrate command to DB thread");
            }
        }

        info!("Uninstalled hexon {} from petal {}", hexon_uri, petal_id);
        Ok(())
    }

    /// Insert or replace one install record under its own `(petal_id, hexon_uri)` key.
    fn insert_record(&mut self, record: InstallRecord) {
        self.installed
            .entry(record.hexon.petal_id.clone())
            .or_default()
            .insert(record.hexon.hexon_uri.clone(), record);
    }

    /// The full install record of `hexon_uri` in `petal_id`.
    fn record(&self, hexon_uri: &str, petal_id: &str) -> Option<&InstallRecord> {
        self.installed.get(petal_id)?.get(hexon_uri)
    }

    // -----------------------------------------------------------------------
    // Entitlement (FR-6; see src/AGENTS.md §entitlement)
    // -----------------------------------------------------------------------

    /// Open the entitlement gate for one install with its stored terms and content hash; an unlicensed
    /// or Free/Attribution build never reaches `verifier` (NFR-2: `EntitlementGate::open` is the only entry).
    pub fn gate_for(
        &self,
        hexon_uri: &str,
        petal_id: &str,
        token: Option<&str>,
        licensee: &Licensee<'_>,
        verifier: &dyn EntitlementVerifier,
    ) -> Result<EntitlementGate, RegistryError> {
        let record = self
            .record(hexon_uri, petal_id)
            .ok_or_else(|| RegistryError::NotFound(hexon_uri.to_string()))?;
        // Without an archive-recomputed hash no token can be checked for this build: preview only.
        let (content_hash, token) = match &record.content_hash {
            Some(content_hash) => (content_hash.as_str(), token),
            None => ("", None),
        };
        Ok(EntitlementGate::open(
            record.terms.clone(),
            &record.hexon.manifest.hexon_id,
            content_hash,
            licensee,
            token,
            verifier,
            unix_now(),
        ))
    }

    /// Load one entry's blob through `gate`: plaintext of a sealed entry exists in memory only and is
    /// never written back to the blob store (spec §4.5).
    pub fn load_entry(
        &self,
        hexon_uri: &str,
        petal_id: &str,
        entry_id: &str,
        gate: &EntitlementGate,
    ) -> Result<Cow<'static, [u8]>, LoadEntryError> {
        let record = self
            .record(hexon_uri, petal_id)
            .ok_or_else(|| LoadEntryError::NotInstalled(hexon_uri.to_string()))?;
        if gate.terms() != &record.terms {
            return Err(LoadEntryError::GateMismatch(hexon_uri.to_string()));
        }
        let entry = record
            .hexon
            .entries
            .iter()
            .find(|entry| entry.entry_id == entry_id)
            .ok_or_else(|| GateRefusal::UnknownEntry(entry_id.to_owned()))?;
        let stored = match self.blob_store.load(&entry.asset_hash) {
            Ok(stored) => stored,
            Err(RegistryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(GateRefusal::MissingContent(entry_id.to_owned()).into());
            }
            Err(error) => return Err(error.into()),
        };
        // A pass-through borrows `stored`; hand the buffer itself back rather than copying it.
        let plaintext = match gate.load(entry_id, &stored)? {
            Cow::Borrowed(_) => None,
            Cow::Owned(plaintext) => Some(plaintext),
        };
        Ok(Cow::Owned(plaintext.unwrap_or(stored)))
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Every install record, optionally only those of one petal.
    pub fn list_installed(&self, petal_id: Option<&str>) -> Vec<InstalledHexon> {
        self.records(petal_id).cloned().collect()
    }

    /// Search installed hexons by query, tags and type across every petal (one hit per build).
    pub fn search_local(
        &self,
        query: &str,
        tags: &[&str],
        kind: Option<HexonType>,
    ) -> Vec<HexonManifest> {
        self.search_local_in_petal(None, query, tags, kind)
    }

    /// Search installed hexons, optionally in one petal; the query matches name, description and hexon_id.
    pub fn search_local_in_petal(
        &self,
        petal_id: Option<&str>,
        query: &str,
        tags: &[&str],
        kind: Option<HexonType>,
    ) -> Vec<HexonManifest> {
        let query_lower = query.to_lowercase();
        let tags_lower: Vec<String> = tags.iter().map(|tag| tag.to_lowercase()).collect();
        let mut seen_uris = BTreeSet::new();

        self.records(petal_id)
            .filter(|record| {
                let manifest = &record.manifest;
                if kind.as_ref().is_some_and(|kind| manifest.hexon_type != *kind) {
                    return false;
                }
                if !query_lower.is_empty() {
                    let matches = manifest.name.to_lowercase().contains(&query_lower)
                        || manifest
                            .description
                            .as_deref()
                            .is_some_and(|text| text.to_lowercase().contains(&query_lower))
                        || manifest.hexon_id.to_lowercase().contains(&query_lower);
                    if !matches {
                        return false;
                    }
                }
                if !tags_lower.is_empty() {
                    let manifest_tags: Vec<String> =
                        manifest.tags.iter().map(|tag| tag.to_lowercase()).collect();
                    let has_all_tags = tags_lower
                        .iter()
                        .all(|tag| manifest_tags.iter().any(|held| held.contains(tag.as_str())));
                    if !has_all_tags {
                        return false;
                    }
                }
                true
            })
            .filter_map(|record| {
                seen_uris
                    .insert(record.hexon_uri.as_str())
                    .then(|| record.manifest.clone())
            })
            .collect()
    }

    /// The install record of `hexon_uri` in `petal_id`, if that petal holds it.
    pub fn get_in_petal(&self, hexon_uri: &str, petal_id: &str) -> Option<&InstalledHexon> {
        self.record(hexon_uri, petal_id).map(|record| &record.hexon)
    }

    /// The published (registry-level) record of `hexon_uri`.
    pub fn get_published(&self, hexon_uri: &str) -> Option<&InstalledHexon> {
        self.get_in_petal(hexon_uri, PUBLISHED_BINDING)
    }

    /// A published record of `hexon_id` (any version) signed by someone other than `publisher_did`.
    pub fn published_by_another(
        &self,
        hexon_id: &str,
        publisher_did: &str,
    ) -> Option<&InstalledHexon> {
        self.installed.get(PUBLISHED_BINDING)?.values().map(|record| &record.hexon).find(|record| {
            record.manifest.hexon_id == hexon_id && record.manifest.publisher_did != publisher_did
        })
    }

    /// Get the blob path for an asset hash.
    pub fn get_blob_path(&self, asset_hash: &str) -> Option<PathBuf> {
        let path = self.blob_store.blob_path(asset_hash).ok()?;
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// Records of one petal, or of every petal when `petal_id` is `None`.
    fn records<'a>(
        &'a self,
        petal_id: Option<&'a str>,
    ) -> impl Iterator<Item = &'a InstalledHexon> + 'a {
        self.installed
            .iter()
            .filter(move |(petal, _)| petal_id.is_none_or(|wanted| petal.as_str() == wanted))
            .flat_map(|(_, petal_records)| petal_records.values().map(|record| &record.hexon))
    }
}

/// Send the `InstallCrate` row and one `InstallCrateEntry` row per entry; a failed send warns.
fn persist_install_to_db(
    manifest: &HexonManifest,
    entries: &[AssetEntry],
    petal_id: &str,
    tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let uri = hexon_uri(manifest);
    let cmd = DbCommand::InstallCrate {
        hexon_uri: uri.clone(),
        manifest_hash: signature::manifest_hash(&serde_json::to_vec(manifest).unwrap_or_default()),
        publisher_did: manifest.publisher_did.clone(),
        hexon_type: manifest.hexon_type.to_string(),
        version: manifest.version.clone(),
        name: manifest.name.clone(),
        tags: manifest.tags.clone(),
        petal_id: petal_id.to_string(),
        size_bytes: manifest.approx_size_bytes.unwrap_or(0),
    };
    if tx.send(cmd).is_err() {
        warn!("Failed to send InstallCrate command to DB thread");
    }

    for entry in entries {
        let cmd = DbCommand::InstallCrateEntry {
            entry_id: entry.entry_id.clone(),
            hexon_uri: uri.clone(),
            kind: entry_kind_name(&entry.kind),
            asset_hash: entry.asset_hash.clone(),
            format: entry.format.clone(),
            label: entry.label.clone(),
            metadata: entry.metadata.clone().unwrap_or_default(),
        };
        if tx.send(cmd).is_err() {
            warn!("Failed to send InstallCrateEntry command to DB thread");
        }
    }
}

/// The snake_case wire name of an entry kind (fe-format's serde spelling).
fn entry_kind_name(kind: &EntryKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entitlement_gate::{GateStatus, PreviewReason};
    use crate::manifest::{License, LicenseType};
    use crate::package::{hexon_uri, HexonPackage};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use fe_format::entitlement::{seal_paid_entry, ContentKey, IssuerKeyring, KeyringVerifier, LocalIssuer, MintRequest};
    use rand::rngs::OsRng;
    use std::collections::HashMap;

    const PAID_PACKAGE: &str = "test-paid-pack";
    const SEALED_PLAINTEXT: &[u8] = b"full-resolution model";
    const PREVIEW_BYTES: &[u8] = b"low-resolution preview";

    /// `FE_BLOB_DIR` is process-global; serialize tests that touch it.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_path_under_data_dir_with_no_override() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(FE_BLOB_DIR_ENV);
        assert_eq!(FsBlobStore::default_path(), PathBuf::from("data/blobs"));
    }

    #[test]
    fn default_path_honors_fe_blob_dir_override() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(FE_BLOB_DIR_ENV, "/custom/blobs");
        let got = FsBlobStore::default_path();
        std::env::remove_var(FE_BLOB_DIR_ENV);
        assert_eq!(got, PathBuf::from("/custom/blobs"));
    }

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
            description: Some("A test model hexon for FractalEngine".to_string()),
            tags: vec!["test".to_string(), "model".to_string()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(2048),
            min_engine_version: Some("0.1.0".to_string()),
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        }
    }

    fn test_package(keypair: &SigningKey) -> HexonPackageData {
        let manifest = test_manifest(keypair);
        let model_bytes = b"fake model data".to_vec();
        let model_hash = crate::signature::asset_hash(&model_bytes);
        let entries = vec![AssetEntry {
            entry_id: "model_001".to_string(),
            kind: EntryKind::Model,
            asset_hash: model_hash.clone(),
            format: "gltf".to_string(),
            label: "Test Model".to_string(),
            tags: vec!["test".to_string()],
            description: Some("A test model".to_string()),
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }];
        let license = License {
            license_type: LicenseType::Free,
            ..License::default()
        };

        let mut assets = HashMap::new();
        assets.insert(model_hash, model_bytes);

        let zip_bytes = HexonPackage::build(
            manifest,
            entries,
            Some(license),
            assets,
            None,
            HashMap::new(),
            None,
            None,
            keypair,
        )
        .expect("build should succeed");

        HexonPackage::open(&zip_bytes).expect("open should succeed")
    }

    /// A placeable model entry whose blob is `data`.
    fn model_entry(entry_id: &str, data: &[u8]) -> AssetEntry {
        AssetEntry {
            entry_id: entry_id.to_string(),
            kind: EntryKind::Model,
            asset_hash: crate::signature::asset_hash(data),
            format: "gltf".to_string(),
            label: entry_id.to_string(),
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

    /// A Paid archive: `preview_1` free, `terrain_full` sealed under `content_key`, key wrapped to `issuer`.
    fn paid_archive(keypair: &SigningKey, content_key: &ContentKey, issuer: &LocalIssuer) -> (Vec<u8>, String) {
        let sealed = seal_paid_entry(content_key, PAID_PACKAGE, "terrain_full", SEALED_PLAINTEXT).unwrap();
        let sealed_hash = crate::signature::asset_hash(&sealed);
        let manifest = HexonManifest {
            hexon_id: PAID_PACKAGE.to_string(),
            name: "Paid Pack".to_string(),
            ..test_manifest(keypair)
        };
        let license = License {
            license_type: LicenseType::Paid,
            free_entries: vec!["preview_1".to_string()],
            encrypted_key: Some(issuer.encrypted_key_for(content_key, PAID_PACKAGE).unwrap()),
            ..License::default()
        };
        let entries = vec![model_entry("preview_1", PREVIEW_BYTES), model_entry("terrain_full", &sealed)];
        let assets = HashMap::from([
            (crate::signature::asset_hash(PREVIEW_BYTES), PREVIEW_BYTES.to_vec()),
            (sealed_hash.clone(), sealed),
        ]);
        let archive = HexonPackage::build(manifest, entries, Some(license), assets, None, HashMap::new(), None, None, keypair)
            .expect("paid build");
        (archive, sealed_hash)
    }

    /// Install records are keyed by (hexon_uri, petal_id): one build lives in two petals and the
    /// published record, each persisted with fe-format names, and uninstalling one leaves the rest.
    #[test]
    fn test_install_and_list() {
        let keypair = test_signing_key();
        let package = test_package(&keypair);
        let uri = hexon_uri(&package.manifest);

        let temp_dir = std::env::temp_dir().join(format!("hexon_test_{}", ulid::Ulid::new()));
        let (db_tx, db_rx) = crossbeam::channel::unbounded();
        let mut registry = HexonRegistry::new(temp_dir.clone()).with_db_channel(db_tx);

        let installed = registry
            .install(package.clone(), "petal-a")
            .expect("install should succeed");
        assert_eq!(installed.hexon_uri, uri);
        assert!(installed.signature_valid);
        assert!(installed.license.is_some(), "the package licence is kept on the record");

        // The DB rows carry fe-format's snake_case type and entry-kind names.
        let rows: Vec<DbCommand> = db_rx.try_iter().collect();
        assert!(rows.iter().any(|row| matches!(row,
            DbCommand::InstallCrate { hexon_uri, hexon_type, petal_id, size_bytes, .. }
                if *hexon_uri == uri && hexon_type == "model" && petal_id == "petal-a" && *size_bytes == 2048)));
        assert!(rows.iter().any(|row| matches!(row,
            DbCommand::InstallCrateEntry { entry_id, kind, .. } if entry_id == "model_001" && kind == "model")));

        // The same build into a second petal is a second record, not an overwrite.
        registry
            .install(package.clone(), "petal-b")
            .expect("a second petal gets its own install");
        assert_eq!(registry.list_installed(None).len(), 2);
        for petal in ["petal-a", "petal-b"] {
            let listed = registry.list_installed(Some(petal));
            assert_eq!(listed.len(), 1, "{petal} lists its own install");
            assert_eq!(listed[0].petal_id, petal);
            assert!(registry.get_in_petal(&uri, petal).is_some());
        }
        assert!(registry.list_installed(Some("petal-456")).is_empty());

        // Uninstall from A leaves B and tells the DB to drop only A's row.
        let _ = db_rx.try_iter().count();
        registry.uninstall(&uri, "petal-a").expect("uninstall from A");
        assert!(registry.get_in_petal(&uri, "petal-a").is_none());
        assert_eq!(registry.list_installed(Some("petal-b")).len(), 1);
        let uninstalls_a: Vec<DbCommand> = db_rx
            .try_iter()
            .filter(|row| matches!(row, DbCommand::UninstallCrate { .. }))
            .collect();
        assert_eq!(uninstalls_a.len(), 1, "exactly one UninstallCrate for A");
        assert!(matches!(&uninstalls_a[0],
            DbCommand::UninstallCrate { petal_id: Some(petal), .. } if petal == "petal-a"));
        registry.uninstall(&uri, "petal-b").expect("uninstall from B");
        assert!(registry.list_installed(None).is_empty());
        assert!(db_rx
            .try_iter()
            .any(|row| matches!(row, DbCommand::UninstallCrate { hexon_uri, petal_id: Some(petal) }
                if hexon_uri == uri && petal == "petal-b")));

        // A published record installs into a petal without its package: blobs are reused.
        registry
            .install(package, PUBLISHED_BINDING)
            .expect("publish-scope record");
        let copied = registry
            .install_published(&uri, "petal-c")
            .expect("install the published build into petal C");
        assert_eq!(copied.petal_id, "petal-c");
        assert_eq!(copied.manifest.hexon_id, "test-model-v1");
        assert!(db_rx.try_iter().any(|row| matches!(row,
            DbCommand::InstallCrate { petal_id, hexon_type, .. } if petal_id == "petal-c" && hexon_type == "model")));
        assert_eq!(registry.list_installed(Some("petal-c")).len(), 1);
        assert!(registry.get_published(&uri).is_some(), "the published record stays");
        assert!(matches!(
            registry.install_published(&uri, "petal-c"),
            Err(RegistryError::AlreadyInstalled(_))
        ));
        assert!(matches!(
            registry.install_published(&uri, PUBLISHED_BINDING),
            Err(RegistryError::PermissionDenied(_))
        ));
        for unknown in ["test-model-v1@9.9.9", "test-model-v1", "@1.0.0"] {
            assert!(
                matches!(
                    registry.install_published(unknown, "petal-d"),
                    Err(RegistryError::NotFound(_))
                ),
                "{unknown} is not a published build"
            );
        }
        assert!(registry.list_installed(Some("petal-d")).is_empty());

        // FR-6 wiring (AC-1, AC-2, AC-4): entitlement is decided per install through gate_for/load_entry.
        let identity = test_signing_key();
        let mut device_secret = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut device_secret);
        let licensee = Licensee { identity: &identity, device_secret: &device_secret };
        let issuer = LocalIssuer::generate("issuer-1").unwrap();
        let mut keyring = IssuerKeyring::new();
        keyring.pin(issuer.kid(), issuer.verifying_key()).unwrap();
        let verifier = KeyringVerifier::new(keyring);

        // A Free build never consults the verifier: even a junk token leaves it NotRequired, bytes verbatim.
        let free_gate = registry.gate_for(&uri, "petal-c", Some("junk"), &licensee, &verifier).unwrap();
        assert_eq!(free_gate.status(), GateStatus::NotRequired);
        assert_eq!(registry.load_entry(&uri, "petal-c", "model_001", &free_gate).unwrap().as_ref(), b"fake model data");

        // A Paid build installs from its archive; with no token only `preview_1` loads.
        let content_key = ContentKey::generate().unwrap();
        let (archive, sealed_hash) = paid_archive(&keypair, &content_key, &issuer);
        let paid = registry.install_archive(&archive, "petal-paid").expect("a paid build installs");
        let paid_uri = paid.hexon_uri.clone();
        let preview_gate = registry.gate_for(&paid_uri, "petal-paid", None, &licensee, &verifier).unwrap();
        assert_eq!(preview_gate.status_label_at(unix_now()), "installed-preview");
        let preview = registry.load_entry(&paid_uri, "petal-paid", "preview_1", &preview_gate).unwrap();
        assert_eq!(preview.as_ref(), PREVIEW_BYTES);
        let refused = registry.load_entry(&paid_uri, "petal-paid", "terrain_full", &preview_gate).unwrap_err();
        assert!(refused.is_entitlement_required(), "no token: {refused:?}");
        // A gate opened for another build's terms never unlocks this one.
        let borrowed = registry.load_entry(&paid_uri, "petal-paid", "terrain_full", &free_gate).unwrap_err();
        assert!(matches!(borrowed, LoadEntryError::GateMismatch(_)), "{borrowed:?}");

        // A LocalIssuer token for this licensee, device and build opens the sealed entry to its plaintext.
        let content_hash = archive_content_hash(&archive).unwrap();
        let encrypted_key = paid.license.as_ref().and_then(|license| license.encrypted_key.clone()).unwrap();
        let (licensee_did, device_did) = (licensee.did(), licensee.device_did());
        let mint = MintRequest {
            licensee_did: &licensee_did,
            device_x25519_did: &device_did,
            package_id: PAID_PACKAGE,
            content_hash: &content_hash,
            encrypted_key: &encrypted_key,
            ttl_secs: 3600,
            jti: None,
        };
        let token = issuer.mint(&mint, unix_now()).unwrap();
        let entitled = registry.gate_for(&paid_uri, "petal-paid", Some(&token), &licensee, &verifier).unwrap();
        assert_eq!(entitled.status_label_at(unix_now()), "entitled");
        let opened = registry.load_entry(&paid_uri, "petal-paid", "terrain_full", &entitled).unwrap();
        assert_eq!(opened.as_ref(), SEALED_PLAINTEXT);
        // Plaintext lives in memory only: the stored blob is still the sealed bytes.
        assert_ne!(FsBlobStore::new(temp_dir.clone()).load(&sealed_hash).unwrap(), SEALED_PLAINTEXT);

        // One flipped signature byte: refused at open, the sealed entry never yields plaintext.
        let (signed_part, signature) = token.rsplit_once('.').unwrap();
        let mut signature_bytes = URL_SAFE_NO_PAD.decode(signature).unwrap();
        signature_bytes[0] ^= 0x01;
        let forged = format!("{signed_part}.{}", URL_SAFE_NO_PAD.encode(&signature_bytes));
        let forged_gate = registry.gate_for(&paid_uri, "petal-paid", Some(&forged), &licensee, &verifier).unwrap();
        assert!(matches!(forged_gate.status(), GateStatus::Preview(PreviewReason::Refused(_))));
        let forged_load = registry.load_entry(&paid_uri, "petal-paid", "terrain_full", &forged_gate).unwrap_err();
        assert!(forged_load.is_entitlement_required(), "forged token: {forged_load:?}");

        // A build published from parsed data has no content hash until its own archive is bound.
        registry.install(HexonPackage::open(&archive).unwrap(), PUBLISHED_BINDING).expect("publish the paid build");
        let unbound = registry.gate_for(&paid_uri, PUBLISHED_BINDING, Some(&token), &licensee, &verifier).unwrap();
        assert_eq!(unbound.status(), GateStatus::Preview(PreviewReason::NoToken));
        assert!(matches!(registry.bind_published_archive(&uri, &archive), Err(RegistryError::PermissionDenied(_))));
        registry.bind_published_archive(&paid_uri, &archive).expect("bind the published archive");
        let bound = registry.gate_for(&paid_uri, PUBLISHED_BINDING, Some(&token), &licensee, &verifier).unwrap();
        assert_eq!(bound.status_label_at(unix_now()), "entitled");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// A catalogued blob that is absent or does not hash to its `asset_hash`
    /// refuses the whole install: nothing is recorded and no blob is stored.
    #[test]
    fn legacy_install_refuses_missing_or_mismatched_blob() {
        enum Corruption {
            Missing,
            Mismatched,
            NotContent,
        }
        for corruption in [Corruption::Missing, Corruption::Mismatched, Corruption::NotContent] {
            let keypair = test_signing_key();
            let mut package = test_package(&keypair);
            let hash = package.entries[0].asset_hash.clone();
            match corruption {
                Corruption::Missing => {
                    package.assets.clear();
                }
                Corruption::Mismatched => {
                    package.assets.insert(
                        hash.clone(),
                        b"bytes that are not the catalogued model".to_vec(),
                    );
                }
                // A petal snapshot restores through its own import route, never as content.
                Corruption::NotContent => {
                    package.manifest.hexon_type = HexonType::PetalSnapshot;
                }
            }
            let temp_dir = std::env::temp_dir().join(format!("hexon_test_{}", ulid::Ulid::new()));
            let mut registry = HexonRegistry::new(temp_dir.clone());

            let result = registry.install(package, "petal-123");

            match corruption {
                Corruption::Missing => assert!(
                    matches!(&result, Err(RegistryError::MissingAssetBlob(h)) if *h == hash),
                    "missing blob: {result:?}"
                ),
                Corruption::Mismatched | Corruption::NotContent => assert!(
                    matches!(&result, Err(RegistryError::Package(_))),
                    "refused package: {result:?}"
                ),
            }
            assert!(registry.list_installed(None).is_empty());
            assert!(!FsBlobStore::new(temp_dir.clone()).has(&hash));
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_install_duplicate_fails() {
        let keypair = test_signing_key();
        let package = test_package(&keypair);

        let temp_dir = std::env::temp_dir().join(format!("hexon_dup_{}", ulid::Ulid::new()));
        let mut registry = HexonRegistry::new(temp_dir.clone());

        registry
            .install(package.clone(), "petal-123")
            .expect("first install should succeed");

        let result = registry.install(package, "petal-123");
        assert!(
            matches!(result, Err(RegistryError::AlreadyInstalled(_))),
            "duplicate install should fail with AlreadyInstalled"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_uninstall() {
        let keypair = test_signing_key();
        let package = test_package(&keypair);
        let uri = hexon_uri(&package.manifest);

        let temp_dir = std::env::temp_dir().join(format!("hexon_uninstall_{}", ulid::Ulid::new()));
        let mut registry = HexonRegistry::new(temp_dir.clone());

        registry
            .install(package, "petal-123")
            .expect("install should succeed");
        assert_eq!(registry.list_installed(None).len(), 1);

        registry
            .uninstall(&uri, "petal-123")
            .expect("uninstall should succeed");
        assert_eq!(registry.list_installed(None).len(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_uninstall_not_found_errors() {
        let temp_dir = std::env::temp_dir().join(format!("hexon_notfound_{}", ulid::Ulid::new()));
        let mut registry = HexonRegistry::new(temp_dir.clone());

        let result = registry.uninstall("nonexistent-crate", "petal-123");
        assert!(
            matches!(result, Err(RegistryError::NotFound(_))),
            "uninstall of nonexistent crate should fail"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_search_local() {
        let keypair = test_signing_key();
        let package = test_package(&keypair);

        let temp_dir = std::env::temp_dir().join(format!("hexon_search_{}", ulid::Ulid::new()));
        let mut registry = HexonRegistry::new(temp_dir.clone());

        registry
            .install(package.clone(), "petal-123")
            .expect("install should succeed");
        registry
            .install(package, "petal-456")
            .expect("second petal install should succeed");

        // Search by name: one hit per build even though two petals hold it
        let results = registry.search_local("Test", &[], None);
        assert_eq!(results.len(), 1);

        // Search by hexon_id, unscoped and scoped
        let results = registry.search_local("test-model-v1", &[], None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hexon_id, "test-model-v1");
        let results = registry.search_local_in_petal(Some("petal-456"), "test-model", &[], None);
        assert_eq!(results.len(), 1);
        assert!(registry
            .search_local_in_petal(Some("petal-789"), "test-model", &[], None)
            .is_empty());

        // Search by tag
        let results = registry.search_local("", &["model"], None);
        assert_eq!(results.len(), 1);

        // Search by kind
        let results = registry.search_local("", &[], Some(HexonType::Model));
        assert_eq!(results.len(), 1);

        // Search by wrong kind
        let results = registry.search_local("", &[], Some(HexonType::Terrain));
        assert_eq!(results.len(), 0);

        // Search with no matches
        let results = registry.search_local("nonexistent", &[], None);
        assert_eq!(results.len(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_blob_store_shared_content_addressing() {
        let temp_dir = std::env::temp_dir().join(format!("hexon_blob_{}", ulid::Ulid::new()));
        let store = FsBlobStore::new(temp_dir.clone());

        let data = b"shared asset content";
        let hash = crate::signature::asset_hash(data);

        // First store
        let first = store.store(&hash, data).expect("store should succeed");
        assert!(first, "first store should return true (newly stored)");

        // Second store with same hash
        let second = store.store(&hash, data).expect("store should succeed");
        assert!(!second, "second store should return false (already exists)");

        // Load the blob
        let loaded = store.load(&hash).expect("load should succeed");
        assert_eq!(loaded, data, "loaded data should match");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
