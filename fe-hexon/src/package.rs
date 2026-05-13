use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use crate::manifest::{AssetEntry, HexonManifest, License};
use crate::signature;
use ed25519_dalek::SigningKey;

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
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

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
            zip_writer.start_file(&format!("preview/{}", name), options)?;
            zip_writer.write_all(bytes)?;
        }

        // assets/{hash}
        for (hash, bytes) in &assets {
            zip_writer.start_file(&format!("assets/{}", hash), options)?;
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
        let reader = Cursor::new(bytes.to_vec());
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| anyhow::anyhow!("invalid ZIP archive: {}", e))?;

        // Read manifest.json (keep raw bytes for signature verification)
        let (manifest, manifest_raw): (HexonManifest, Vec<u8>) = {
            let mut file = archive.by_name("manifest.json")
                .map_err(|e| anyhow::anyhow!("missing manifest.json: {}", e))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            let parsed: HexonManifest = serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("manifest.json parse error: {}", e))?;
            (parsed, buf)
        };

        // Read entries.json
        let entries: Vec<AssetEntry> = {
            let mut file = archive.by_name("entries.json")
                .map_err(|e| anyhow::anyhow!("missing entries.json: {}", e))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("entries.json parse error: {}", e))?
        };

        // Read license.json
        let license: License = {
            let mut file = archive.by_name("license.json")
                .map_err(|e| anyhow::anyhow!("missing license.json: {}", e))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("license.json parse error: {}", e))?
        };

        // Read optional signature.txt
        let signature = {
            let mut file = archive.by_name("signature.txt")
                .map_err(|e| anyhow::anyhow!("missing signature.txt: {}", e))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            buf.trim().to_string()
        };

        // Read optional icon.png
        let icon = archive.by_name("icon.png").ok().and_then(|mut f| {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).ok()?;
            Some(buf)
        });

        // Read preview/* files
        let mut previews = HashMap::new();
        for i in 0..archive.len() {
            let name = {
                let file = archive.by_index(i)?;
                file.enclosed_name().and_then(|p| p.to_str()).map(|s| s.to_string())
            };
            if let Some(name) = name {
                if name.starts_with("preview/") && name != "preview/" {
                    let mut buf = Vec::new();
                    let mut f = archive.by_index(i)?;
                    f.read_to_end(&mut buf)?;
                    let file_name = name.strip_prefix("preview/").unwrap_or(&name).to_string();
                    previews.insert(file_name, buf);
                }
            }
        }

        // Read assets/{hash} files
        let mut assets = HashMap::new();
        for i in 0..archive.len() {
            let name = {
                let file = archive.by_index(i)?;
                file.enclosed_name().and_then(|p| p.to_str()).map(|s| s.to_string())
            };
            if let Some(name) = name {
                if name.starts_with("assets/") && name != "assets/" {
                    let mut buf = Vec::new();
                    let mut f = archive.by_index(i)?;
                    f.read_to_end(&mut buf)?;
                    let hash = name.strip_prefix("assets/").unwrap_or(&name).to_string();
                    assets.insert(hash, buf);
                }
            }
        }

        // Read optional README.md
        let readme = archive.by_name("README.md").ok().and_then(|mut f| {
            let mut buf = String::new();
            f.read_to_string(&mut buf).ok()?;
            Some(buf)
        });

        // Read optional schema.json
        let schema = archive.by_name("schema.json").ok().and_then(|mut f| {
            let mut buf = String::new();
            f.read_to_string(&mut buf).ok()?;
            Some(buf)
        });

        // Verify signature against raw manifest bytes (not re-serialized)
        let sig_valid = signature::verify_manifest(&manifest_raw, &signature, &manifest.publisher_did)?;

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

/// Extract a hexon URI from a manifest.
/// Format: `{crate_id}@{version}`
pub fn hexon_uri(manifest: &HexonManifest) -> String {
    format!("{}@{}", manifest.crate_id, manifest.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::collections::HashMap;

    use crate::manifest::{
        AssetEntry, EntryKind, HexonKind, HexonManifest, License, LicenseType,
    };

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
            manifest, entries.clone(), license, assets, None, HashMap::new(), None, None, &keypair,
        ).unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();

        // Verify every entry's asset_hash matches BLAKE3 of the actual blob
        for entry in &package.entries {
            let blob = package.assets.get(&entry.asset_hash)
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
                    ("normal".to_string(), crate::signature::asset_hash(normal_data)),
                    ("roughness".to_string(), crate::signature::asset_hash(roughness_data)),
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
        assets.insert(crate::signature::asset_hash(albedo_data), albedo_data.to_vec());
        assets.insert(crate::signature::asset_hash(normal_data), normal_data.to_vec());
        assets.insert(crate::signature::asset_hash(roughness_data), roughness_data.to_vec());

        let zip_bytes = HexonPackage::build(
            manifest, entries.clone(), license.clone(), assets, None, HashMap::new(), None, None, &keypair,
        ).unwrap();

        let package = HexonPackage::open(&zip_bytes).unwrap();

        // Verify structure
        assert_eq!(package.entries.len(), 3);
        assert_eq!(package.assets.len(), 3);
        assert_eq!(package.license.license_type, LicenseType::Attribution);
        assert_eq!(package.license.free_entries, vec!["mat_pbr"]);

        // Verify sub_assets are preserved
        let mat = package.entries.iter().find(|e| e.entry_id == "mat_pbr").unwrap();
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
                metadata: Some(serde_json::json!({"zoom_min": 5, "zoom_max": 14, "bounds": [5.9, 45.8, 10.5, 47.8]})),
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
            terrain_manifest, entries.clone(), license, assets, None, HashMap::new(), None, None, &keypair,
        ).unwrap();

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
            skybox_manifest, skybox_entries, license2, skybox_assets, None, HashMap::new(), None, None, &keypair,
        ).unwrap();

        let pkg2 = HexonPackage::open(&zip2).unwrap();
        assert_eq!(pkg2.manifest.hexon_type, HexonKind::Skybox);
        assert_eq!(pkg2.license.license_type, LicenseType::Paid);
        assert!(pkg2.license.payment_provider.is_some());
        assert!(pkg2.signature_valid());
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
            assert!(!pkg.signature_valid(), "tampered package should fail signature");
        }
        // If open fails due to corruption, that's also acceptable
    }
}
