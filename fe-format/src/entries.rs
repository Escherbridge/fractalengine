use serde::{Deserialize, Serialize};

/// The kind of asset an entry represents within a `.hexon` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Model,
    Texture,
    Sprite,
    Material,
    Skybox,
    Surface,
    VisualLayer,
    VisualScope,
    Sound,
    GpxTrack,
    GeoJson,
    TerrainTileset,
    Script,
}

/// A single asset entry within the `entries.json` catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetEntry {
    pub entry_id: String,
    pub kind: EntryKind,
    pub asset_hash: String,
    pub format: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub is_placeable: bool,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default)]
    pub auto_scale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extents: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_assets: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<super::manifest::HexonAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Wraps the list of asset entries for `entries.json` serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryCatalog {
    pub entries: Vec<AssetEntry>,
}
