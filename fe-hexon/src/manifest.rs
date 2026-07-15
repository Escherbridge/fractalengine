#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hexon_manifest_roundtrip() {
        let manifest = HexonManifest {
            schema_version: 1,
            crate_id: "fe-model-v1".to_string(),
            publisher_did: "did:example:publisher-123".to_string(),
            publisher_name: "Test Publisher".to_string(),
            version: "1.0.0".to_string(),
            build_id: "abc123def456".to_string(),
            name: "My Hexon Model".to_string(),
            description: "A test hexon model for FractalEngine".to_string(),
            tags: vec!["model".to_string(), "terrain".to_string()],
            hexon_type: HexonKind::Model,
            created_at: "2026-05-10T10:00:00Z".to_string(),
            updated_at: "2026-05-10T10:30:00Z".to_string(),
            approx_size_bytes: 1_234_567,
            min_engine_version: "0.8.0".to_string(),
            homepage_url: Some("https://example.com/my-hexon".to_string()),
            dependencies: vec![CrateDep {
                hexon_uri: "fe-material-v1#v2.0.0".to_string(),
                version_req: ">=2.0.0,<3.0.0".to_string(),
            }],
            signature: "signature_placeholder_here".to_string(),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: HexonManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(manifest.schema_version, deserialized.schema_version);
        assert_eq!(manifest.crate_id, deserialized.crate_id);
        assert_eq!(manifest.publisher_did, deserialized.publisher_did);
        assert_eq!(manifest.name, deserialized.name);
    }

    #[test]
    fn test_asset_entry_roundtrip() {
        let entry = AssetEntry {
            entry_id: "texture_main".to_string(),
            kind: EntryKind::Texture,
            asset_hash: "bafybeigdyrzt3sduhp2lul5zq4a3mjsu7h4x4m26rjy3d2njkf3k6p".to_string(),
            format: "png".to_string(),
            label: "Main Texture".to_string(),
            tags: vec!["color".to_string(), "diffuse".to_string()],
            description: "The main diffuse texture for the model".to_string(),
            is_placeable: false,
            is_private: true,
            preview_image: Some("preview.png".to_string()),
            metadata: Some(
                serde_json::json!({ "dimensions": [2048, 2048], "color_space": "sRGB" }),
            ),
            sub_assets: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: AssetEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(entry.entry_id, deserialized.entry_id);
        assert_eq!(entry.kind, deserialized.kind);
        assert_eq!(entry.asset_hash, deserialized.asset_hash);
    }

    #[test]
    fn test_license_roundtrip() {
        let license = License {
            license_type: LicenseType::Attribution,
            attribution: Some("Creative Commons BY 4.0".to_string()),
            payment_provider: None,
            payment_verification_url: None,
            free_entries: vec!["free_model_v1".to_string()],
            encrypted_key: None,
        };

        let json = serde_json::to_string(&license).unwrap();
        let deserialized: License = serde_json::from_str(&json).unwrap();

        assert_eq!(license.license_type, deserialized.license_type);
        if let Some(attrib1) = &license.attribution {
            assert_eq!(
                &attrib1[..],
                deserialized.attribution.as_ref().map_or("", |s| s.as_str())
            );
        }
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A manifest describing a hexon crate (model, material, terrain tileset, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonManifest {
    /// Schema version of this manifest structure
    pub schema_version: u32,
    /// Unique identifier for the hexon crate definition
    pub crate_id: String,
    /// Decentralized identity of the publisher
    pub publisher_did: String,
    /// Human-readable publisher name
    pub publisher_name: String,
    /// Semantic version of this specific manifest
    pub version: String,
    /// Build identifier (git commit, CI run ID, etc.)
    pub build_id: String,
    /// Display name of the hexon
    pub name: String,
    /// Description of what this hexon is and how to use it
    pub description: String,
    /// Optional tags for discovery and filtering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Type of hexon content (model, material, terrain, etc.)
    pub hexon_type: HexonKind,
    /// ISO 8601 timestamp when the manifest was first created
    pub created_at: String,
    /// ISO 8601 timestamp of the last update to this manifest
    pub updated_at: String,
    /// Approximate size in bytes (for bandwidth estimation)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub approx_size_bytes: u64,
    /// Minimum Bevy/Fe engine version required to load this hexon
    pub min_engine_version: String,
    /// Optional homepage or documentation URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage_url: Option<String>,
    /// List of other hexons that must be installed before using this one
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<CrateDep>,
    /// Cryptographic signature to verify manifest integrity (from publisher)
    pub signature: String,
}

/// Is zero for u64, used to skip serialization when size is unknown/zero
fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// The kind of hexon content this manifest describes
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HexonKind {
    Model,
    Material,
    Skybox,
    Terrain,
    GpxCollection,
    Sound,
    Theme,
    Script,
}

impl std::fmt::Display for HexonKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HexonKind::Model => write!(f, "model"),
            HexonKind::Material => write!(f, "material"),
            HexonKind::Skybox => write!(f, "skybox"),
            HexonKind::Terrain => write!(f, "terrain"),
            HexonKind::GpxCollection => write!(f, "gpx"),
            HexonKind::Sound => write!(f, "sound"),
            HexonKind::Theme => write!(f, "theme"),
            HexonKind::Script => write!(f, "script"),
        }
    }
}

impl std::str::FromStr for HexonKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "model" => Ok(HexonKind::Model),
            "material" => Ok(HexonKind::Material),
            "skybox" => Ok(HexonKind::Skybox),
            "terrain" => Ok(HexonKind::Terrain),
            "gpx" | "gpxcollection" => Ok(HexonKind::GpxCollection),
            "sound" => Ok(HexonKind::Sound),
            "theme" => Ok(HexonKind::Theme),
            "script" => Ok(HexonKind::Script),
            _ => anyhow::bail!("Unknown HexonKind: {}", s),
        }
    }
}

/// A dependency on another hexon crate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateDep {
    /// The URI identifying the hexon (e.g., "fe-material-v1" or "https://example.com/my-hexon")
    pub hexon_uri: String,
    /// Version range requirement (semver-style)
    pub version_req: String,
}

/// Represents a single asset within a hexon manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetEntry {
    /// Unique identifier for this entry within its parent manifest
    pub entry_id: String,
    /// The kind of asset (texture, model mesh, gpx file, etc.)
    pub kind: EntryKind,
    /// BLAKE3 hash of the actual asset content
    pub asset_hash: String,
    /// File format or MIME type hint
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    /// Human-readable label for this entry (e.g., "Main Diffuse" or "Mountain Texture")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Optional tags for filtering and search
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Description of what this asset does
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Whether this asset can be placed/used as a runtime resource (e.g., terrain tile, texture)
    #[serde(default)]
    pub is_placeable: bool,
    /// Whether this entry is private and requires authentication to access
    #[serde(default)]
    pub is_private: bool,
    /// Optional URL or path to preview image in the UI
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_image: Option<String>,
    /// Arbitrary metadata that can be serialized JSON
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<serde_json::Value>,
    /// Optional sub-assets (e.g., separate textures for a material)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_assets: Option<HashMap<String, String>>,
}

/// The kind of asset entry this represents
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryKind {
    Model,
    Texture,
    Material,
    Skybox,
    TerrainTileset,
    GpxFile,
    Sound,
    Sprite,
    Surface,
    Script,
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryKind::Model => write!(f, "model"),
            EntryKind::Texture => write!(f, "texture"),
            EntryKind::Material => write!(f, "material"),
            EntryKind::Skybox => write!(f, "skybox"),
            EntryKind::TerrainTileset => write!(f, "terrain_tileset"),
            EntryKind::GpxFile => write!(f, "gpx"),
            EntryKind::Sound => write!(f, "sound"),
            EntryKind::Sprite => write!(f, "sprite"),
            EntryKind::Surface => write!(f, "surface"),
            EntryKind::Script => write!(f, "script"),
        }
    }
}

impl std::str::FromStr for EntryKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "model" => Ok(EntryKind::Model),
            "texture" => Ok(EntryKind::Texture),
            "material" => Ok(EntryKind::Material),
            "skybox" => Ok(EntryKind::Skybox),
            "terrain_tileset" | "terraintileset" => Ok(EntryKind::TerrainTileset),
            "gpx" | "gpxfile" => Ok(EntryKind::GpxFile),
            "sound" => Ok(EntryKind::Sound),
            "sprite" => Ok(EntryKind::Sprite),
            "surface" => Ok(EntryKind::Surface),
            "script" => Ok(EntryKind::Script),
            _ => anyhow::bail!("Unknown EntryKind: {}", s),
        }
    }
}

/// Represents licensing and access control information for a hexon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    /// The type of license (free, attribution-only, paid)
    pub license_type: LicenseType,
    /// Optional text describing required attribution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    /// Optional payment provider ID for monetized hexons
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_provider: Option<String>,
    /// URL where users can verify they have paid or unlocked this hexon
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_verification_url: Option<String>,
    /// IDs of entries that are free to use even within a paid hexon
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub free_entries: Vec<String>,
    /// Optional encrypted key for signing operations (not stored in plain text)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub encrypted_key: Option<String>,
}

/// The type of license this hexon uses
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LicenseType {
    /// Free to use with no restrictions beyond fair use
    Free,
    /// Can be used freely but requires attribution
    Attribution,
    /// Requires payment or subscription for access
    Paid,
}

impl std::fmt::Display for LicenseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseType::Free => write!(f, "free"),
            LicenseType::Attribution => write!(f, "attribution"),
            LicenseType::Paid => write!(f, "paid"),
        }
    }
}

impl std::str::FromStr for LicenseType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "free" => Ok(LicenseType::Free),
            "attribution" | "cc-by" => Ok(LicenseType::Attribution),
            "paid" => Ok(LicenseType::Paid),
            _ => anyhow::bail!("Unknown LicenseType: {}", s),
        }
    }
}

/// A manifest entry that has been downloaded and verified, ready to be installed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledCrate {
    /// The URI of the original hexon manifest (e.g., "fe-model-v1")
    pub hexon_uri: String,
    /// The manifest that describes this crate's contents
    pub manifest: HexonManifest,
    /// All assets included in this installation
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<AssetEntry>,
    /// When this crate was installed (ISO 8601)
    pub installed_at: String,
    /// The Petal ID that owns/hosts this installed crate
    pub petal_id: String,
    /// Whether the manifest's signature verifies successfully against publisher_did
    #[serde(default)]
    pub signature_valid: bool,
}
