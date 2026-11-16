//! Installed model packages as `{id}@{version}.hexon` files (see `src/AGENTS.md` §model-library).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fe_format::{split_hexon_uri, HexonArchiveIndex};

use crate::handlers::model::handle_model_install;
use crate::manifest::{AssetEntry, EntryKind, HexonType, License, LicenseType};
use crate::package::HexonPackage;
use crate::registry::FsBlobStore;

/// File extension of an installed package.
const PACKAGE_EXTENSION: &str = "hexon";
/// Member holding the licence; read through the header reader.
const LICENSE_MEMBER: &str = "license.json";
/// Petal label passed to the model install handler (a library install belongs to no petal).
const LIBRARY_SCOPE: &str = "library";
/// Distinguishes concurrent temp files inside one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Why a library operation refused; one variant per distinct refusal the UI words differently.
#[derive(Debug, thiserror::Error)]
pub enum ModelLibraryError {
    #[error("not a readable hexon package: {0}")]
    Unreadable(String),
    #[error("package is not signed by its publisher")]
    Unsigned,
    #[error("package type is {0}, not model")]
    NotModel(String),
    #[error("licence missing")]
    LicenseMissing,
    #[error("paid licences are not supported in this build")]
    LicensePaid,
    #[error("an attribution licence requires attribution text")]
    AttributionMissing,
    #[error("package id or version is not a safe file name: {0}")]
    UnsafeName(String),
    #[error("package contains no model entries")]
    NoModels,
    #[error("model entry {0} has no asset in the package")]
    MissingAsset(String),
    #[error("{0} is already installed")]
    AlreadyInstalled(String),
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("{uri} has no model entry {entry_id}")]
    EntryNotFound { uri: String, entry_id: String },
    #[error("storing model blobs failed: {0}")]
    Blobs(String),
    #[error("library file error: {0}")]
    Io(#[from] std::io::Error),
}

/// Header facts of one installed package, read without loading its assets.
#[derive(Debug, Clone)]
pub struct ModelPackageSummary {
    /// `{hexon_id}@{version}`.
    pub uri: String,
    pub hexon_id: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub publisher_did: String,
    pub publisher_name: Option<String>,
    pub tags: Vec<String>,
    pub license: Option<License>,
    /// The package's model entries (kind `Model`).
    pub models: Vec<AssetEntry>,
    /// Archive size on disk in bytes.
    pub size_bytes: u64,
}

/// Directory of installed model packages plus the shared blob store their assets live in.
pub struct ModelLibrary {
    dir: PathBuf,
    blob_store: FsBlobStore,
}

impl ModelLibrary {
    /// Open (creating) `dir`; leftover temp files from a crashed install are removed.
    pub fn open(dir: impl Into<PathBuf>, blob_store: FsBlobStore) -> Result<Self, ModelLibraryError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        if let Ok(read) = std::fs::read_dir(&dir) {
            for item in read.flatten() {
                let path = item.path();
                if is_temp_file(&path) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(Self { dir, blob_store })
    }

    /// Verify, store blobs and write the package atomically; an installed id and version is refused.
    pub fn install(&self, bytes: &[u8]) -> Result<ModelPackageSummary, ModelLibraryError> {
        let package =
            HexonPackage::open(bytes).map_err(|error| ModelLibraryError::Unreadable(format!("{error:#}")))?;
        if !package.signature_valid() {
            return Err(ModelLibraryError::Unsigned);
        }
        if package.manifest.hexon_type != HexonType::Model {
            return Err(ModelLibraryError::NotModel(format!("{:?}", package.manifest.hexon_type)));
        }
        check_license(package.license.as_ref())?;

        let uri = package.hexon_uri();
        let path = self.package_path(&package.manifest.hexon_id, &package.manifest.version)?;
        if path.exists() {
            return Err(ModelLibraryError::AlreadyInstalled(uri));
        }
        let mut model_count = 0usize;
        for entry in package.entries.iter().filter(|entry| entry.kind == EntryKind::Model) {
            if !package.assets.contains_key(&entry.asset_hash) {
                return Err(ModelLibraryError::MissingAsset(entry.entry_id.clone()));
            }
            model_count += 1;
        }
        if model_count == 0 {
            return Err(ModelLibraryError::NoModels);
        }

        handle_model_install(&package, LIBRARY_SCOPE, &self.blob_store)
            .map_err(|error| ModelLibraryError::Blobs(format!("{error:#}")))?;
        write_atomic(&path, bytes)?;
        summarize(&path).map_err(|error| {
            let _ = std::fs::remove_file(&path);
            ModelLibraryError::Unreadable(format!("{error:#}"))
        })
    }

    /// Every installed package, sorted by uri; unreadable files are skipped with a warning.
    pub fn list(&self) -> Vec<ModelPackageSummary> {
        let mut found = Vec::new();
        let read = match std::fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(dir = %self.dir.display(), %error, "model library directory unreadable");
                return found;
            }
        };
        for item in read.flatten() {
            let path = item.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(PACKAGE_EXTENSION) || is_temp_file(&path) {
                continue;
            }
            match summarize(&path) {
                Ok(summary) => found.push(summary),
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %format!("{error:#}"), "skipping unreadable model package");
                }
            }
        }
        found.sort_by(|left, right| left.uri.cmp(&right.uri));
        found
    }

    /// Delete the package file; its blobs stay (placed nodes may still reference them).
    pub fn remove(&self, uri: &str) -> Result<(), ModelLibraryError> {
        let path = self.path_for_uri(uri)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(ModelLibraryError::NotInstalled(uri.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// One model entry of an installed package.
    pub fn entry(&self, uri: &str, entry_id: &str) -> Result<AssetEntry, ModelLibraryError> {
        let path = self.path_for_uri(uri)?;
        if !path.exists() {
            return Err(ModelLibraryError::NotInstalled(uri.to_string()));
        }
        let summary = summarize(&path).map_err(|error| ModelLibraryError::Unreadable(format!("{error:#}")))?;
        summary
            .models
            .into_iter()
            .find(|entry| entry.entry_id == entry_id)
            .ok_or_else(|| ModelLibraryError::EntryNotFound {
                uri: uri.to_string(),
                entry_id: entry_id.to_string(),
            })
    }

    /// For each asset hash a library model uses, the installed packages that carry it.
    pub fn packages_for_hashes(&self, hashes: &[String]) -> HashMap<String, Vec<ModelPackageSummary>> {
        let wanted: HashSet<&str> = hashes.iter().map(String::as_str).collect();
        let mut by_hash: HashMap<String, Vec<ModelPackageSummary>> = HashMap::new();
        if wanted.is_empty() {
            return by_hash;
        }
        for summary in self.list() {
            let mut seen: HashSet<&str> = HashSet::new();
            for entry in &summary.models {
                let sub_hashes = entry.sub_assets.iter().flat_map(|subs| subs.values());
                for hash in std::iter::once(&entry.asset_hash).chain(sub_hashes) {
                    if wanted.contains(hash.as_str()) && seen.insert(hash.as_str()) {
                        by_hash.entry(hash.clone()).or_default().push(summary.clone());
                    }
                }
            }
        }
        by_hash
    }

    /// `{dir}/{id}@{version}.hexon`, only for a safe id and version.
    fn package_path(&self, hexon_id: &str, version: &str) -> Result<PathBuf, ModelLibraryError> {
        if !is_safe_name_part(hexon_id) || !is_safe_name_part(version) {
            return Err(ModelLibraryError::UnsafeName(format!("{hexon_id}@{version}")));
        }
        Ok(self.dir.join(format!("{hexon_id}@{version}.{PACKAGE_EXTENSION}")))
    }

    /// Path for a full `id@version` uri; an unversioned uri is refused.
    fn path_for_uri(&self, uri: &str) -> Result<PathBuf, ModelLibraryError> {
        match split_hexon_uri(uri) {
            (id, Some(version)) => self.package_path(id, version),
            _ => Err(ModelLibraryError::UnsafeName(uri.to_string())),
        }
    }
}

/// Licence rule: present, not paid, and attribution text when the licence is Attribution.
fn check_license(license: Option<&License>) -> Result<(), ModelLibraryError> {
    let license = license.ok_or(ModelLibraryError::LicenseMissing)?;
    match license.license_type {
        LicenseType::Paid => Err(ModelLibraryError::LicensePaid),
        LicenseType::Attribution
            if license.attribution.as_deref().is_none_or(|text| text.trim().is_empty()) =>
        {
            Err(ModelLibraryError::AttributionMissing)
        }
        _ => Ok(()),
    }
}

/// An id or version as one file-name part: the shared safe-id guard, with `+` (build metadata) allowed.
fn is_safe_name_part(part: &str) -> bool {
    fe_format::is_safe_path_id(&part.replace('+', "_"))
}

/// Library temp files are dot-prefixed with a `.tmp` extension.
fn is_temp_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("tmp")
        && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with('.'))
}

/// Read a package's header (manifest, entries, licence) from disk; assets are never loaded.
fn summarize(path: &Path) -> anyhow::Result<ModelPackageSummary> {
    let index = HexonArchiveIndex::open(path)?;
    let manifest = index.manifest();
    let uri = format!("{}@{}", manifest.hexon_id, manifest.version);
    anyhow::ensure!(
        path.file_stem().and_then(|stem| stem.to_str()) == Some(uri.as_str()),
        "file name does not match its manifest {uri}"
    );
    let license: Option<License> = match index.read_member(LICENSE_MEMBER)? {
        Some(raw) => Some(serde_json::from_slice(&raw)?),
        None => None,
    };
    Ok(ModelPackageSummary {
        uri,
        hexon_id: manifest.hexon_id.clone(),
        version: manifest.version.clone(),
        name: manifest.name.clone(),
        description: manifest.description.clone(),
        publisher_did: manifest.publisher_did.clone(),
        publisher_name: manifest.publisher_name.clone(),
        tags: manifest.tags.clone(),
        license,
        models: index
            .entries()
            .iter()
            .filter(|entry| entry.kind == EntryKind::Model)
            .cloned()
            .collect(),
        size_bytes: std::fs::metadata(path)?.len(),
    })
}

/// Temp file in the target directory, flush and sync, rename; the temp is removed on any failure.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target.file_name().and_then(|name| name.to_str()).unwrap_or("package");
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}.{counter}.tmp", std::process::id()));
    let result = write_synced(&temp, bytes).and_then(|()| std::fs::rename(&temp, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn write_synced(temp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(temp)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::manifest::HexonManifest;
    use crate::publisher::HexonBuilder;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn model_package(id: &str, kind: HexonType, license: LicenseType, attribution: Option<&str>) -> Vec<u8> {
        let did = fe_format::verifying_key_to_did(&key().verifying_key());
        let mut builder = HexonBuilder::new(id, did)
            .name("Chair")
            .hexon_type(kind)
            .license(license)
            .add_entry_with_file("chair", EntryKind::Model, "glb", "Chair", b"glb-bytes-of-a-chair");
        if let Some(text) = attribution {
            builder = builder.attribution(text);
        }
        builder.build(&key()).expect("build package")
    }

    fn unlicensed_package() -> Vec<u8> {
        let manifest = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "bare".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: String::new(),
            publisher_name: None,
            version: "1.0.0".to_string(),
            build_id: None,
            name: "Bare".to_string(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: None,
        };
        HexonPackage::build(manifest, vec![], None, HashMap::new(), None, HashMap::new(), None, None, &key())
            .expect("build unlicensed package")
    }

    #[test]
    fn install_refusals_then_one_library_journey() {
        let root = tempfile::tempdir().unwrap();
        let library =
            ModelLibrary::open(root.path().join("models"), FsBlobStore::new(root.path().join("blobs"))).unwrap();

        let refusals: Vec<(&str, Vec<u8>, &str)> = vec![
            ("wrong type", model_package("mat", HexonType::Material, LicenseType::Free, None), "NotModel"),
            ("no licence", unlicensed_package(), "LicenseMissing"),
            ("paid", model_package("paid", HexonType::Model, LicenseType::Paid, None), "LicensePaid"),
            (
                "attribution without text",
                model_package("attr", HexonType::Model, LicenseType::Attribution, Some("  ")),
                "AttributionMissing",
            ),
        ];
        for (label, bytes, variant) in refusals {
            let error = library.install(&bytes).expect_err(label);
            assert!(format!("{error:?}").starts_with(variant), "{label}: got {error}");
        }
        assert!(library.list().is_empty(), "refusals leave nothing installed");

        let good = model_package("chair", HexonType::Model, LicenseType::Attribution, Some("Chair by Ada, CC-BY-4.0"));
        let installed = library.install(&good).expect("install");
        assert_eq!(installed.uri, "chair@0.1.0");
        assert!(matches!(
            library.install(&good),
            Err(ModelLibraryError::AlreadyInstalled(uri)) if uri == "chair@0.1.0"
        ));

        let listed = library.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].license.as_ref().and_then(|license| license.attribution.as_deref()),
            Some("Chair by Ada, CC-BY-4.0")
        );
        let entry = library.entry("chair@0.1.0", "chair").expect("entry");
        assert!(library.entry("chair@0.1.0", "table").is_err());
        assert!(library.blob_store.has(&entry.asset_hash), "blob landed in the shared store");

        let hashes = vec![entry.asset_hash.clone(), "0".repeat(64)];
        let by_hash = library.packages_for_hashes(&hashes);
        assert_eq!(by_hash.len(), 1);
        assert_eq!(by_hash[&entry.asset_hash][0].uri, "chair@0.1.0");

        assert!(matches!(library.remove("../chair@0.1.0"), Err(ModelLibraryError::UnsafeName(_))));
        library.remove("chair@0.1.0").expect("remove");
        assert!(library.list().is_empty());
        assert!(matches!(library.remove("chair@0.1.0"), Err(ModelLibraryError::NotInstalled(_))));
        let leftovers = std::fs::read_dir(root.path().join("models")).unwrap().count();
        assert_eq!(leftovers, 0, "no temp files remain");
    }
}
