use std::collections::HashMap;
use std::path::PathBuf;

use fe_runtime::messages::DbCommand;
use tracing::{info, warn};

use crate::manifest::{HexonKind, HexonManifest, InstalledCrate};
use crate::package::{hexon_uri, HexonPackageData};
use crate::signature;

/// Error types for registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("signature verification failed for crate {0}")]
    SignatureVerificationFailed(String),

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

/// Content-addressed blob store for asset files.
///
/// Stores assets under `assets/<blake3_hash>` so identical blobs are only stored once.
pub struct FsBlobStore {
    base_dir: PathBuf,
}

impl FsBlobStore {
    /// Create a new blob store rooted at `base_dir`.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Store a blob if it doesn't already exist. Returns true if newly stored.
    pub fn store(&self, hash: &str, data: &[u8]) -> Result<bool, RegistryError> {
        let path = self.blob_path(hash);
        if path.exists() {
            return Ok(false); // already exists (shared content addressing)
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, data)?;
        Ok(true)
    }

    /// Check if a blob exists.
    pub fn has(&self, hash: &str) -> bool {
        self.blob_path(hash).exists()
    }

    /// Read a blob by hash.
    pub fn load(&self, hash: &str) -> Result<Vec<u8>, RegistryError> {
        std::fs::read(self.blob_path(hash)).map_err(|e| RegistryError::Io(e))
    }

    /// Get the path for a blob by hash.
    fn blob_path(&self, hash: &str) -> PathBuf {
        self.base_dir.join(hash)
    }

    /// Default hexon blob store location: `{dirs::data_local_dir()}/fractalengine/hexon_blobs/`.
    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fractalengine")
            .join("hexon_blobs")
    }

    /// Convenience constructor using [`Self::default_path`].
    pub fn open_default() -> Self {
        Self::new(Self::default_path())
    }
}

/// Local hexon crate registry.
///
/// Tracks installed crates per petal and manages asset blob storage.
/// Can be used as a Bevy Resource.
pub struct HexonRegistry {
    /// Installed crates keyed by hexon URI.
    installed: HashMap<String, InstalledCrate>,
    /// Content-addressed blob storage.
    blob_store: FsBlobStore,
    /// DB command sender for persisting registry state.
    db_tx: Option<crossbeam::channel::Sender<DbCommand>>,
}

impl HexonRegistry {
    /// Create a new registry with the given blob store directory.
    pub fn new(blob_store_dir: PathBuf) -> Self {
        Self {
            installed: HashMap::new(),
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

    /// Install a hexon crate into a petal.
    ///
    /// Workflow:
    /// 1. Verify manifest signature
    /// 2. Extract manifest + entries
    /// 3. Copy asset blobs to FsBlobStore (skip if hash exists)
    /// 4. Write registry entries to DB (via DbCommand)
    /// 5. For terrain crates: auto-configure petal TerrainConfig
    /// 6. For model crates: register assets in petal
    pub fn install(
        &mut self,
        package: HexonPackageData,
        petal_id: &str,
    ) -> Result<InstalledCrate, RegistryError> {
        // Step 1: Trust signature_verified from open() (verified against raw ZIP bytes).
        // Re-verification here would fail because re-serialization differs from raw bytes.
        if !package.signature_verified {
            let uri = hexon_uri(&package.manifest);
            return Err(RegistryError::SignatureVerificationFailed(uri));
        }

        let uri = hexon_uri(&package.manifest);

        // Check if already installed for this petal
        if let Some(existing) = self.installed.get(&uri) {
            if existing.petal_id == petal_id {
                return Err(RegistryError::AlreadyInstalled(uri));
            }
        }

        // Step 2 & 3: Copy asset blobs to blob store
        for entry in &package.entries {
            if let Some(data) = package.assets.get(&entry.asset_hash) {
                let stored = self.blob_store.store(&entry.asset_hash, data)?;
                if stored {
                    info!(
                        "Stored new asset blob {} for entry {} in crate {}",
                        entry.asset_hash, entry.entry_id, uri
                    );
                } else {
                    info!(
                        "Asset blob {} already exists (shared), skipping for entry {}",
                        entry.asset_hash, entry.entry_id
                    );
                }
            } else {
                warn!(
                    "Missing asset blob for entry {} (hash {}) in crate {}",
                    entry.entry_id, entry.asset_hash, uri
                );
            }
        }

        // Step 4: Write registry entries to DB
        if let Some(tx) = &self.db_tx {
            self.persist_install_to_db(&package, petal_id, &tx)?;
        }

        // Build the InstalledCrate record
        let installed = InstalledCrate {
            hexon_uri: uri.clone(),
            manifest: package.manifest.clone(),
            entries: package.entries.clone(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            petal_id: petal_id.to_string(),
            signature_valid: true,
        };

        // Step 5 & 6: Type-specific post-install actions via handlers
        match crate::handlers::handle_install(&package, petal_id, &self.blob_store) {
            Ok(result) => {
                info!("{}", result.summary);
            }
            Err(e) => {
                warn!("Handler post-install failed for {}: {}", uri, e);
            }
        }

        self.installed.insert(uri, installed.clone());
        Ok(installed)
    }

    /// Uninstall a hexon crate from a petal.
    ///
    /// Removes registry entries. Asset blobs remain if referenced elsewhere
    /// (shared content addressing).
    pub fn uninstall(&mut self, hexon_uri: &str, _petal_id: &str) -> Result<(), RegistryError> {
        let crate_data = self
            .installed
            .remove(hexon_uri)
            .ok_or_else(|| RegistryError::NotFound(hexon_uri.to_string()))?;

        // Remove from DB
        if let Some(tx) = &self.db_tx {
            let cmd = DbCommand::UninstallCrate {
                hexon_uri: hexon_uri.to_string(),
            };
            if tx.send(cmd).is_err() {
                warn!("Failed to send UninstallCrate command to DB thread");
            }
        }

        // Note: we do NOT remove asset blobs from the blob store because other
        // crates may reference the same hashes (shared content addressing).
        info!("Uninstalled crate {} from petal {}", hexon_uri, crate_data.petal_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// List all installed crates, optionally filtered by petal.
    pub fn list_installed(&self, petal_id: Option<&str>) -> Vec<InstalledCrate> {
        self.installed
            .values()
            .filter(|c| match petal_id {
                Some(pid) => c.petal_id == pid,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// Search local installed crates by query string, tags, and kind.
    pub fn search_local(
        &self,
        query: &str,
        tags: &[&str],
        kind: Option<HexonKind>,
    ) -> Vec<HexonManifest> {
        let query_lower = query.to_lowercase();

        self.installed
            .values()
            .filter(|c| {
                // Match on kind
                if let Some(k) = kind {
                    if c.manifest.hexon_type != k {
                        return false;
                    }
                }

                // Match on query (name, description, crate_id)
                if !query.is_empty() {
                    let matches = c.manifest.name.to_lowercase().contains(&query_lower)
                        || c.manifest.description.to_lowercase().contains(&query_lower)
                        || c.manifest.crate_id.to_lowercase().contains(&query_lower);
                    if !matches {
                        return false;
                    }
                }

                // Match on tags
                if !tags.is_empty() {
                    let manifest_tags_lower: Vec<String> =
                        c.manifest.tags.iter().map(|t| t.to_lowercase()).collect();
                    let has_all_tags = tags.iter().all(|t| {
                        manifest_tags_lower.iter().any(|mt| mt.contains(&t.to_lowercase()))
                    });
                    if !has_all_tags {
                        return false;
                    }
                }

                true
            })
            .map(|c| c.manifest.clone())
            .collect()
    }

    /// Get a specific installed crate by URI.
    pub fn get_installed(&self, hexon_uri: &str) -> Option<&InstalledCrate> {
        self.installed.get(hexon_uri)
    }

    /// Get the blob path for an asset hash.
    pub fn get_blob_path(&self, asset_hash: &str) -> Option<PathBuf> {
        let path = self.blob_store.blob_path(asset_hash);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // DB Persistence
    // -----------------------------------------------------------------------

    /// Persist an install to the database via DbCommand.
    fn persist_install_to_db(
        &self,
        package: &HexonPackageData,
        petal_id: &str,
        tx: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), RegistryError> {
        let manifest = &package.manifest;

        // Insert crate_registry row
        let cmd = DbCommand::InstallCrate {
            hexon_uri: hexon_uri(manifest),
            manifest_hash: signature::manifest_hash(
                &serde_json::to_vec(manifest).unwrap_or_default(),
            ),
            publisher_did: manifest.publisher_did.clone(),
            hexon_type: manifest.hexon_type.to_string(),
            version: manifest.version.clone(),
            name: manifest.name.clone(),
            tags: manifest.tags.clone(),
            petal_id: petal_id.to_string(),
            size_bytes: manifest.approx_size_bytes,
        };
        if tx.send(cmd).is_err() {
            warn!("Failed to send InstallCrate command to DB thread");
        }

        // Insert crate_entry rows
        for entry in &package.entries {
            let cmd = DbCommand::InstallCrateEntry {
                entry_id: entry.entry_id.clone(),
                hexon_uri: hexon_uri(manifest),
                kind: entry.kind.to_string(),
                asset_hash: entry.asset_hash.clone(),
                format: entry.format.clone(),
                label: entry.label.clone(),
                metadata: entry.metadata.clone().unwrap_or_default(),
            };
            if tx.send(cmd).is_err() {
                warn!("Failed to send InstallCrateEntry command to DB thread");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        AssetEntry, EntryKind, HexonKind, HexonManifest, License, LicenseType,
    };
    use crate::package::{hexon_uri, HexonPackage};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::collections::HashMap;

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
            description: "A test model crate for FractalEngine".to_string(),
            tags: vec!["test".to_string(), "model".to_string()],
            hexon_type: HexonKind::Model,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            approx_size_bytes: 2048,
            min_engine_version: "0.1.0".to_string(),
            homepage_url: None,
            dependencies: vec![],
            signature: String::new(),
        }
    }

    fn test_package(keypair: &SigningKey) -> HexonPackageData {
        let manifest = test_manifest(keypair);
        let entries = vec![AssetEntry {
            entry_id: "model_001".to_string(),
            kind: EntryKind::Model,
            asset_hash: "def456".to_string(),
            format: "gltf".to_string(),
            label: "Test Model".to_string(),
            tags: vec!["test".to_string()],
            description: "A test model".to_string(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: None,
        }];
        let license = License {
            license_type: LicenseType::Free,
            attribution: None,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec![],
            encrypted_key: None,
        };

        let mut assets = HashMap::new();
        assets.insert("def456".to_string(), b"fake model data".to_vec());

        let zip_bytes = HexonPackage::build(
            manifest.clone(),
            entries.clone(),
            license,
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

    #[test]
    fn test_install_and_list() {
        let keypair = test_signing_key();
        let package = test_package(&keypair);
        let uri = hexon_uri(&package.manifest);

        let temp_dir = std::env::temp_dir().join(format!("hexon_test_{}", ulid::Ulid::new()));
        let mut registry = HexonRegistry::new(temp_dir.clone());

        let installed = registry
            .install(package, "petal-123")
            .expect("install should succeed");
        assert_eq!(installed.hexon_uri, uri);
        assert!(installed.signature_valid);

        let list = registry.list_installed(None);
        assert_eq!(list.len(), 1);

        let list_petal = registry.list_installed(Some("petal-123"));
        assert_eq!(list_petal.len(), 1);

        let list_other = registry.list_installed(Some("petal-456"));
        assert_eq!(list_other.len(), 0);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
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
            .install(package, "petal-123")
            .expect("install should succeed");

        // Search by name
        let results = registry.search_local("Test", &[], None);
        assert_eq!(results.len(), 1);

        // Search by tag
        let results = registry.search_local("", &["model"], None);
        assert_eq!(results.len(), 1);

        // Search by kind
        let results = registry.search_local("", &[], Some(HexonKind::Model));
        assert_eq!(results.len(), 1);

        // Search by wrong kind
        let results = registry.search_local("", &[], Some(HexonKind::Terrain));
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
