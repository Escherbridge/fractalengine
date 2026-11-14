//! Runtime install record; every on-disk type is fe-format's (see `src/AGENTS.md` §unified-format).

use serde::{Deserialize, Serialize};

pub use fe_format::{AssetEntry, EntryKind, HexonDep, HexonManifest, HexonType, License, LicenseType};

/// A hexon installed into a petal after its package opened and verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledHexon {
    /// `{hexon_id}@{version}` of the installed build.
    pub hexon_uri: String,
    /// The signed manifest of the installed build.
    pub manifest: HexonManifest,
    /// Every catalogued asset of the build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<AssetEntry>,
    /// Install time (RFC 3339).
    pub installed_at: String,
    /// The petal this install belongs to.
    pub petal_id: String,
    /// True when the package's seal and manifest signature verified on open.
    #[serde(default)]
    pub signature_valid: bool,
    /// The package's `license.json`, when it carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "fe-model-v1".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: "did:example:publisher-123".to_string(),
            publisher_name: Some("Test Publisher".to_string()),
            version: "1.0.0".to_string(),
            build_id: Some("abc123def456".to_string()),
            name: "My Hexon Model".to_string(),
            description: Some("A test hexon model for FractalEngine".to_string()),
            tags: vec!["model".to_string(), "terrain".to_string()],
            created_at: "2026-05-10T10:00:00Z".to_string(),
            updated_at: "2026-05-10T10:30:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(1_234_567),
            min_engine_version: Some("0.8.0".to_string()),
            homepage_url: Some("https://example.com/my-hexon".to_string()),
            dependencies: vec![HexonDep {
                hexon_uri: "fe-material-v1@2.0.0".to_string(),
                version_req: ">=2.0.0,<3.0.0".to_string(),
            }],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: Some("signature_placeholder_here".to_string()),
        }
    }

    fn entry() -> AssetEntry {
        AssetEntry {
            entry_id: "texture_main".to_string(),
            kind: EntryKind::Texture,
            asset_hash: "bafybeigdyrzt3sduhp2lul5zq4a3mjsu7h4x4m26rjy3d2njkf3k6p".to_string(),
            format: "png".to_string(),
            label: "Main Texture".to_string(),
            tags: vec!["color".to_string(), "diffuse".to_string()],
            description: Some("The main diffuse texture for the model".to_string()),
            is_placeable: false,
            is_private: true,
            auto_scale: false,
            preview_image: Some("preview.png".to_string()),
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: Some(serde_json::json!({ "dimensions": [2048, 2048], "color_space": "sRGB" })),
        }
    }

    /// The install record round-trips with its fe-format manifest, entries and licence intact.
    #[test]
    fn test_installed_hexon_roundtrip() {
        let installed = InstalledHexon {
            hexon_uri: "fe-model-v1@1.0.0".to_string(),
            manifest: manifest(),
            entries: vec![entry()],
            installed_at: "2026-05-10T11:00:00Z".to_string(),
            petal_id: "petal-1".to_string(),
            signature_valid: true,
            license: Some(License {
                license_type: LicenseType::Attribution,
                attribution: Some("Creative Commons BY 4.0".to_string()),
                free_entries: vec!["free_model_v1".to_string()],
                ..License::default()
            }),
        };

        let json = serde_json::to_string(&installed).unwrap();
        let back: InstalledHexon = serde_json::from_str(&json).unwrap();

        assert_eq!(back.hexon_uri, installed.hexon_uri);
        assert_eq!(back.petal_id, "petal-1");
        assert!(back.signature_valid);
        assert_eq!(back.manifest.hexon_id, "fe-model-v1");
        assert_eq!(back.manifest.hexon_type, HexonType::Model);
        assert_eq!(back.manifest.dependencies[0].version_req, ">=2.0.0,<3.0.0");
        assert_eq!(back.entries[0].asset_hash, installed.entries[0].asset_hash);
        assert!(matches!(back.entries[0].kind, EntryKind::Texture));
        let license = back.license.expect("licence survives the round trip");
        assert!(matches!(license.license_type, LicenseType::Attribution));
        assert_eq!(license.attribution.as_deref(), Some("Creative Commons BY 4.0"));
        assert_eq!(license.free_entries, vec!["free_model_v1"]);
    }

    /// A record written without the optional fields (no licence, no entries) still reads back.
    #[test]
    fn test_installed_hexon_reads_minimal_record() {
        let json = serde_json::json!({
            "hexon_uri": "fe-model-v1@1.0.0",
            "manifest": manifest(),
            "installed_at": "2026-05-10T11:00:00Z",
            "petal_id": "petal-1",
        });
        let back: InstalledHexon = serde_json::from_value(json).unwrap();
        assert!(back.entries.is_empty());
        assert!(back.license.is_none());
        assert!(!back.signature_valid, "an absent flag never reads as verified");
    }
}
