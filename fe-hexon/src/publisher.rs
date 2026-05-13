use std::collections::HashMap;
use std::path::Path;

use ed25519_dalek::SigningKey;

use crate::manifest::{
    AssetEntry, CrateDep, EntryKind, HexonKind, HexonManifest, License, LicenseType,
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

/// Fluent builder for constructing signed `.fecrate` packages.
///
/// # Example
/// ```no_run
/// use fe_hexon::publisher::HexonBuilder;
/// use fe_hexon::manifest::{EntryKind, HexonKind, LicenseType};
/// # use ed25519_dalek::SigningKey;
/// # use rand::rngs::OsRng;
/// # let signing_key = SigningKey::generate(&mut OsRng);
///
/// let crate_bytes = HexonBuilder::new("my-model-v1", "did:key:z6Mk...")
///     .name("My Model")
///     .description("A beautiful 3D model")
///     .hexon_type(HexonKind::Model)
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
    crate_id: String,
    publisher_did: String,
    publisher_name: String,
    version: String,
    build_id: String,
    name: String,
    description: String,
    tags: Vec<String>,
    hexon_type: HexonKind,
    min_engine_version: String,
    homepage_url: Option<String>,
    dependencies: Vec<CrateDep>,
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
    /// Create a new builder with the required crate_id and publisher DID.
    pub fn new(crate_id: impl Into<String>, publisher_did: impl Into<String>) -> Self {
        Self {
            crate_id: crate_id.into(),
            publisher_did: publisher_did.into(),
            publisher_name: String::new(),
            version: "0.1.0".to_string(),
            build_id: ulid::Ulid::new().to_string(),
            name: String::new(),
            description: String::new(),
            tags: Vec::new(),
            hexon_type: HexonKind::Model,
            min_engine_version: "0.1.0".to_string(),
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
        self.description = desc.into();
        self
    }

    pub fn publisher_name(mut self, name: impl Into<String>) -> Self {
        self.publisher_name = name.into();
        self
    }

    pub fn hexon_type(mut self, kind: HexonKind) -> Self {
        self.hexon_type = kind;
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn build_id(mut self, build_id: impl Into<String>) -> Self {
        self.build_id = build_id.into();
        self
    }

    pub fn min_engine_version(mut self, version: impl Into<String>) -> Self {
        self.min_engine_version = version.into();
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

    pub fn dependency(mut self, hexon_uri: impl Into<String>, version_req: impl Into<String>) -> Self {
        self.dependencies.push(CrateDep {
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
            description: String::new(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: None,
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

    /// Validate and build the signed `.fecrate` ZIP package.
    ///
    /// Returns raw ZIP bytes suitable for distribution or writing to disk.
    pub fn build(self, signing_key: &SigningKey) -> Result<Vec<u8>, BuilderError> {
        // Validate required fields
        if self.name.is_empty() {
            return Err(BuilderError::MissingField("name"));
        }
        if self.crate_id.is_empty() {
            return Err(BuilderError::MissingField("crate_id"));
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
        let manifest = HexonManifest {
            schema_version: 1,
            crate_id: self.crate_id,
            publisher_did: self.publisher_did,
            publisher_name: self.publisher_name,
            version: self.version,
            build_id: self.build_id,
            name: self.name,
            description: self.description,
            tags: self.tags,
            hexon_type: self.hexon_type,
            created_at: now.clone(),
            updated_at: now,
            approx_size_bytes: total_size,
            min_engine_version: self.min_engine_version,
            homepage_url: self.homepage_url,
            dependencies: self.dependencies,
            signature: String::new(), // filled by HexonPackage::build
        };

        let license = License {
            license_type: self.license_type,
            attribution: self.attribution,
            payment_provider: None,
            payment_verification_url: None,
            free_entries: self.free_entries,
            encrypted_key: None,
        };

        HexonPackage::build(
            manifest,
            self.entries,
            license,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::HexonPackage;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn test_did(keypair: &SigningKey) -> String {
        let pub_key = keypair.verifying_key();
        let mut bytes = vec![0xed, 0x01];
        bytes.extend_from_slice(&pub_key.to_bytes());
        format!(
            "did:key:{}",
            multibase::encode(multibase::Base::Base58Btc, &bytes)
        )
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
            .hexon_type(HexonKind::Model)
            .version("1.0.0")
            .tag("test")
            .tag("model")
            .license(LicenseType::Free)
            .add_entry_with_file("model_001", EntryKind::Model, "glb", "Main Model", model_data)
            .readme("# Test Model\nBuilt with HexonBuilder")
            .build(&keypair)
            .expect("build should succeed");

        let package = HexonPackage::open(&zip_bytes).expect("open should succeed");
        assert_eq!(package.manifest.crate_id, "test-model-v1");
        assert_eq!(package.manifest.name, "Test Model");
        assert_eq!(package.manifest.hexon_type, HexonKind::Model);
        assert_eq!(package.entries.len(), 1);
        assert!(package.signature_valid());
        assert!(package.readme.is_some());

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
            description: String::new(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: Some(HashMap::from([
                ("normal".to_string(), normal_hash.clone()),
            ])),
        };

        let mut builder = HexonBuilder::new("test-mat-v1", &did)
            .name("Test Material")
            .hexon_type(HexonKind::Material)
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
            description: String::new(),
            is_placeable: true,
            is_private: false,
            preview_image: None,
            metadata: None,
            sub_assets: None,
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
            .hexon_type(HexonKind::Model)
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
