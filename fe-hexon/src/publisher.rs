use std::collections::HashMap;
use std::path::Path;

use ed25519_dalek::SigningKey;

use crate::manifest::{
    AssetEntry, EntryKind, HexonDep, HexonManifest, HexonType, License, LicenseType,
};
use crate::package::HexonPackage;
use crate::signature;

/// Builder error types.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),

    #[error("asset hash mismatch for entry {entry_id}: expected {expected}, got {actual}")]
    HashMismatch {
        entry_id: String,
        expected: String,
        actual: String,
    },

    #[error("entry {0} references asset hash {1} but no asset data was provided")]
    MissingAsset(String, String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("build error: {0}")]
    Build(#[from] anyhow::Error),
}

/// Fluent builder for signed `.hexon` packages on fe-format (see `src/AGENTS.md` §unified-format).
///
/// # Example
/// ```no_run
/// use fe_hexon::publisher::HexonBuilder;
/// use fe_hexon::manifest::{EntryKind, HexonType, LicenseType};
/// # use ed25519_dalek::SigningKey;
/// # use rand::rngs::OsRng;
/// # let signing_key = SigningKey::generate(&mut OsRng);
///
/// let hexon_bytes = HexonBuilder::new("my-model-v1", "did:key:z6Mk...")
///     .name("My Model")
///     .description("A beautiful 3D model")
///     .hexon_type(HexonType::Model)
///     .version("1.0.0")
///     .tag("model")
///     .tag("architecture")
///     .license(LicenseType::Free)
///     .add_entry_with_file(
///         "model_main",
///         EntryKind::Model,
///         "gltf",
///         "Main Model",
///         b"...gltf bytes...",
///     )
///     .build(&signing_key)
///     .expect("build failed");
/// ```
pub struct HexonBuilder {
    hexon_id: String,
    publisher_did: String,
    publisher_name: Option<String>,
    version: String,
    build_id: Option<String>,
    name: String,
    description: Option<String>,
    tags: Vec<String>,
    hexon_type: HexonType,
    min_engine_version: Option<String>,
    homepage_url: Option<String>,
    dependencies: Vec<HexonDep>,
    entries: Vec<AssetEntry>,
    assets: HashMap<String, Vec<u8>>,
    license_type: LicenseType,
    attribution: Option<String>,
    free_entries: Vec<String>,
    icon: Option<Vec<u8>>,
    previews: HashMap<String, Vec<u8>>,
    readme: Option<String>,
    schema: Option<String>,
}

impl HexonBuilder {
    /// Create a new builder with the required hexon_id and publisher DID.
    pub fn new(hexon_id: impl Into<String>, publisher_did: impl Into<String>) -> Self {
        Self {
            hexon_id: hexon_id.into(),
            publisher_did: publisher_did.into(),
            publisher_name: None,
            version: "0.1.0".to_string(),
            build_id: Some(ulid::Ulid::new().to_string()),
            name: String::new(),
            description: None,
            tags: Vec::new(),
            hexon_type: HexonType::Model,
            min_engine_version: None,
            homepage_url: None,
            dependencies: Vec::new(),
            entries: Vec::new(),
            assets: HashMap::new(),
            license_type: LicenseType::Free,
            attribution: None,
            free_entries: Vec::new(),
            icon: None,
            previews: HashMap::new(),
            readme: None,
            schema: None,
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn publisher_name(mut self, name: impl Into<String>) -> Self {
        self.publisher_name = Some(name.into());
        self
    }

    pub fn hexon_type(mut self, kind: HexonType) -> Self {
        self.hexon_type = kind;
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn build_id(mut self, build_id: impl Into<String>) -> Self {
        self.build_id = Some(build_id.into());
        self
    }

    pub fn min_engine_version(mut self, version: impl Into<String>) -> Self {
        self.min_engine_version = Some(version.into());
        self
    }

    pub fn homepage(mut self, url: impl Into<String>) -> Self {
        self.homepage_url = Some(url.into());
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags.extend(tags.into_iter().map(|t| t.into()));
        self
    }

    pub fn dependency(
        mut self,
        hexon_uri: impl Into<String>,
        version_req: impl Into<String>,
    ) -> Self {
        self.dependencies.push(HexonDep {
            hexon_uri: hexon_uri.into(),
            version_req: version_req.into(),
        });
        self
    }

    pub fn license(mut self, license_type: LicenseType) -> Self {
        self.license_type = license_type;
        self
    }

    pub fn attribution(mut self, attr: impl Into<String>) -> Self {
        self.attribution = Some(attr.into());
        self
    }

    pub fn free_entry(mut self, entry_id: impl Into<String>) -> Self {
        self.free_entries.push(entry_id.into());
        self
    }

    /// Add a pre-built AssetEntry and its raw data.
    pub fn add_entry(mut self, entry: AssetEntry, data: Vec<u8>) -> Self {
        self.assets.insert(entry.asset_hash.clone(), data);
        self.entries.push(entry);
        self
    }

    /// Add an entry by providing raw bytes. The asset_hash is computed automatically.
    pub fn add_entry_with_file(
        mut self,
        entry_id: impl Into<String>,
        kind: EntryKind,
        format: impl Into<String>,
        label: impl Into<String>,
        data: &[u8],
    ) -> Self {
        let hash = signature::asset_hash(data);
        let entry = AssetEntry {
            entry_id: entry_id.into(),
            kind,
            asset_hash: hash.clone(),
            format: format.into(),
            label: label.into(),
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
        };
        self.assets.insert(hash, data.to_vec());
        self.entries.push(entry);
        self
    }

    /// Add an asset file from disk. The asset_hash is computed from file contents.
    pub fn add_asset_file(
        self,
        entry_id: impl Into<String>,
        kind: EntryKind,
        format: impl Into<String>,
        label: impl Into<String>,
        path: &Path,
    ) -> Result<Self, BuilderError> {
        let data = std::fs::read(path)?;
        Ok(self.add_entry_with_file(entry_id, kind, format, label, &data))
    }

    pub fn icon(mut self, data: Vec<u8>) -> Self {
        self.icon = Some(data);
        self
    }

    /// Add a preview image (e.g., sphere render for materials, thumbnail for models).
    pub fn preview(mut self, name: impl Into<String>, data: Vec<u8>) -> Self {
        self.previews.insert(name.into(), data);
        self
    }

    pub fn readme(mut self, text: impl Into<String>) -> Self {
        self.readme = Some(text.into());
        self
    }

    pub fn schema(mut self, schema_json: impl Into<String>) -> Self {
        self.schema = Some(schema_json.into());
        self
    }

    /// Validate and build the sealed, signed `.hexon` package.
    ///
    /// Returns raw ZIP bytes suitable for distribution or writing to disk.
    pub fn build(self, signing_key: &SigningKey) -> Result<Vec<u8>, BuilderError> {
        // Validate required fields
        if self.name.is_empty() {
            return Err(BuilderError::MissingField("name"));
        }
        if self.hexon_id.is_empty() {
            return Err(BuilderError::MissingField("hexon_id"));
        }
        if self.publisher_did.is_empty() {
            return Err(BuilderError::MissingField("publisher_did"));
        }

        // Validate all entry asset hashes match actual blob data
        for entry in &self.entries {
            match self.assets.get(&entry.asset_hash) {
                Some(data) => {
                    let computed = signature::asset_hash(data);
                    if computed != entry.asset_hash {
                        return Err(BuilderError::HashMismatch {
                            entry_id: entry.entry_id.clone(),
                            expected: entry.asset_hash.clone(),
                            actual: computed,
                        });
                    }
                }
                None => {
                    return Err(BuilderError::MissingAsset(
                        entry.entry_id.clone(),
                        entry.asset_hash.clone(),
                    ));
                }
            }

            // Validate sub_asset hashes too
            if let Some(sub) = &entry.sub_assets {
                for (role, hash) in sub {
                    if !self.assets.contains_key(hash) {
                        return Err(BuilderError::MissingAsset(
                            format!("{}:{}", entry.entry_id, role),
                            hash.clone(),
                        ));
                    }
                }
            }
        }

        // Compute total size
        let total_size: u64 = self.assets.values().map(|v| v.len() as u64).sum();

        let now = chrono::Utc::now().to_rfc3339();
        // `HexonPackage::build` seals and signs; the seal stamps the signer's DID over `publisher_did`.
        let manifest = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: self.hexon_id,
            hexon_type: self.hexon_type,
            publisher_did: self.publisher_did,
            publisher_name: self.publisher_name,
            version: self.version,
            build_id: self.build_id,
            name: self.name,
            description: self.description,
            tags: self.tags,
            created_at: now.clone(),
            updated_at: now,
            source_peer_did: None,
            approx_size_bytes: Some(total_size),
            min_engine_version: self.min_engine_version,
            homepage_url: self.homepage_url,
            dependencies: self.dependencies,
            platforms: Vec::new(),
            address: None,
            integrity: None,
            signature: None,
        };

        let license = License {
            license_type: self.license_type,
            attribution: self.attribution,
            free_entries: self.free_entries,
            ..License::default()
        };

        HexonPackage::build(
            manifest,
            self.entries,
            Some(license),
            self.assets,
            self.icon,
            self.previews,
            self.readme,
            self.schema,
            signing_key,
        )
        .map_err(BuilderError::Build)
    }
}

/// The `entries.json` every plugin package carries (docs/extensions/package-format.md §Members).
pub const PLUGIN_ENTRIES_JSON: &[u8] = br#"{"entries":[]}"#;

/// Builds a sealed `HexonType::Plugin` package (see `fe-format/src/AGENTS.md` §plugin-package).
pub struct PluginHexonBuilder {
    extension_json: Vec<u8>,
    component: Vec<u8>,
    license_json: Option<Vec<u8>>,
    publisher_name: Option<String>,
}

impl PluginHexonBuilder {
    /// `extension_json` is manifest v2; `hexon_id`, `version` and `name` are taken from it.
    pub fn new(extension_json: impl Into<Vec<u8>>, component: impl Into<Vec<u8>>) -> Self {
        Self {
            extension_json: extension_json.into(),
            component: component.into(),
            license_json: None,
            publisher_name: None,
        }
    }

    /// Optional `license.json` header member.
    pub fn license_json(mut self, license_json: impl Into<Vec<u8>>) -> Self {
        self.license_json = Some(license_json.into());
        self
    }

    pub fn publisher_name(mut self, name: impl Into<String>) -> Self {
        self.publisher_name = Some(name.into());
        self
    }

    /// Write the members, then `seal` (the last transform) with `signing_key`.
    pub fn build(self, signing_key: &SigningKey) -> Result<Vec<u8>, BuilderError> {
        use std::io::Write;

        if self.component.is_empty() {
            return Err(BuilderError::MissingField("plugin/component.wasm"));
        }
        let extension: serde_json::Value = serde_json::from_slice(&self.extension_json)
            .map_err(|e| anyhow::anyhow!("plugin/extension.json parse error: {e}"))?;
        let text = |key: &'static str| {
            extension
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let hexon_id = text("id").ok_or(BuilderError::MissingField("extension.id"))?;
        let version = text("version").ok_or(BuilderError::MissingField("extension.version"))?;
        let name = text("name").ok_or(BuilderError::MissingField("extension.name"))?;

        let now = chrono::Utc::now().to_rfc3339();
        let manifest = fe_format::HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id,
            hexon_type: fe_format::HexonType::Plugin,
            publisher_did: fe_format::verifying_key_to_did(&signing_key.verifying_key()),
            publisher_name: self.publisher_name,
            version,
            build_id: Some(ulid::Ulid::new().to_string()),
            name,
            description: text("description"),
            tags: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            source_peer_did: None,
            approx_size_bytes: Some(self.component.len() as u64),
            min_engine_version: None,
            homepage_url: None,
            dependencies: Vec::new(),
            platforms: Vec::new(),
            address: None,
            integrity: None,
            signature: None,
        };

        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut members: Vec<(&str, Vec<u8>)> = vec![
            (
                fe_format::integrity::MANIFEST_PATH,
                serde_json::to_vec_pretty(&manifest).map_err(anyhow::Error::from)?,
            ),
            // Empty catalogue: `HexonArchiveIndex::open` (registry header + download routes) requires it.
            (
                fe_format::integrity::ENTRIES_PATH,
                PLUGIN_ENTRIES_JSON.to_vec(),
            ),
            (
                fe_format::integrity::PLUGIN_EXTENSION_PATH,
                self.extension_json,
            ),
            (fe_format::integrity::PLUGIN_COMPONENT_PATH, self.component),
        ];
        if let Some(license) = self.license_json {
            members.push(("license.json", license));
        }
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, bytes) in members {
            writer.start_file(path, options).map_err(anyhow::Error::from)?;
            writer.write_all(&bytes)?;
        }
        let unsealed = writer.finish().map_err(anyhow::Error::from)?.into_inner();
        Ok(fe_format::seal(&unsealed, signing_key)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::HexonPackage;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn test_did(keypair: &SigningKey) -> String {
        fe_format::verifying_key_to_did(&keypair.verifying_key())
    }

    #[test]
    fn builder_roundtrip() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = test_did(&keypair);

        let model_data = b"fake glb model bytes";

        let zip_bytes = HexonBuilder::new("test-model-v1", &did)
            .name("Test Model")
            .description("A builder test")
            .publisher_name("Test Publisher")
            .hexon_type(HexonType::Model)
            .version("1.0.0")
            .tag("test")
            .tag("model")
            .license(LicenseType::Free)
            .add_entry_with_file(
                "model_001",
                EntryKind::Model,
                "glb",
                "Main Model",
                model_data,
            )
            .readme("# Test Model\nBuilt with HexonBuilder")
            .build(&keypair)
            .expect("build should succeed");

        let package = HexonPackage::open(&zip_bytes).expect("open should succeed");
        assert_eq!(package.manifest.hexon_id, "test-model-v1");
        assert_eq!(package.manifest.name, "Test Model");
        assert_eq!(package.manifest.hexon_type, HexonType::Model);
        assert_eq!(package.entries.len(), 1);
        assert!(package.signature_valid());
        assert!(package.readme.is_some());
        assert_eq!(package.manifest.schema_version, "1.0.0");

        // The package opens through fe-format's own importer and is sealed by the signer.
        let imported = fe_format::HexonArchive::import(&zip_bytes).expect("fe-format import");
        assert_eq!(imported.manifest.hexon_id, "test-model-v1");
        assert!(matches!(
            imported.integrity,
            fe_format::IntegrityStatus::Sealed { ref publisher_did } if *publisher_did == did
        ));

        // Verify asset hash matches
        let entry = &package.entries[0];
        let blob = package.assets.get(&entry.asset_hash).unwrap();
        assert_eq!(blob, model_data);
    }

    #[test]
    fn builder_material_with_sub_assets() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = test_did(&keypair);

        let albedo = b"albedo texture";
        let normal = b"normal texture";
        let albedo_hash = signature::asset_hash(albedo);
        let normal_hash = signature::asset_hash(normal);

        let mat_entry = AssetEntry {
            entry_id: "mat_pbr".to_string(),
            kind: EntryKind::Material,
            asset_hash: albedo_hash.clone(),
            format: "pbr_bundle".to_string(),
            label: "PBR Material".to_string(),
            tags: vec![],
            description: None,
            is_placeable: true,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: Some(HashMap::from([("normal".to_string(), normal_hash.clone())])),
            address: None,
            metadata: None,
        };

        let mut builder = HexonBuilder::new("test-mat-v1", &did)
            .name("Test Material")
            .hexon_type(HexonType::Material)
            .version("1.0.0")
            .license(LicenseType::Attribution)
            .attribution("CC-BY 4.0")
            .add_entry(mat_entry, albedo.to_vec());

        // Add the sub-asset blob
        builder.assets.insert(normal_hash.clone(), normal.to_vec());

        let zip_bytes = builder.build(&keypair).expect("build should succeed");
        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert_eq!(package.entries.len(), 1);
        assert_eq!(package.assets.len(), 2);
        assert!(package.signature_valid());
    }

    #[test]
    fn builder_missing_name_fails() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = test_did(&keypair);

        let result = HexonBuilder::new("test", &did)
            .version("1.0.0")
            .build(&keypair);

        assert!(matches!(result, Err(BuilderError::MissingField("name"))));

        let result = HexonBuilder::new("", &did).name("Named").build(&keypair);
        assert!(matches!(result, Err(BuilderError::MissingField("hexon_id"))));
    }

    #[test]
    fn builder_missing_asset_fails() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = test_did(&keypair);

        let entry = AssetEntry {
            entry_id: "missing".to_string(),
            kind: EntryKind::Model,
            asset_hash: "nonexistent_hash".to_string(),
            format: "glb".to_string(),
            label: "Missing".to_string(),
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
        };

        let result = HexonBuilder::new("test", &did)
            .name("Test")
            .add_entry(entry, vec![]) // empty data won't match hash
            .build(&keypair);

        // Should fail because asset_hash won't match
        assert!(result.is_err());
    }

    #[test]
    fn builder_with_previews() {
        let keypair = SigningKey::generate(&mut OsRng);
        let did = test_did(&keypair);

        let model_data = b"model data";
        let preview_data = b"fake png thumbnail";

        let zip_bytes = HexonBuilder::new("preview-test", &did)
            .name("Preview Test")
            .hexon_type(HexonType::Model)
            .version("1.0.0")
            .license(LicenseType::Free)
            .add_entry_with_file("model_001", EntryKind::Model, "glb", "Model", model_data)
            .preview("thumbnail.png", preview_data.to_vec())
            .icon(b"icon png data".to_vec())
            .build(&keypair)
            .expect("build should succeed");

        let package = HexonPackage::open(&zip_bytes).unwrap();
        assert!(package.icon.is_some());
        assert_eq!(package.previews.len(), 1);
        assert!(package.previews.contains_key("thumbnail.png"));
    }
}
