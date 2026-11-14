use serde::{Deserialize, Serialize};

/// The kind of asset an entry represents within a `.hexon` archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(alias = "gpx_file", alias = "GpxFile")]
    GpxTrack,
    GeoJson,
    TerrainTileset,
    Script,
}

/// An entry kind string that matches no known kind or legacy spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEntryKind(pub String);

impl std::fmt::Display for UnknownEntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown entry kind: {}", self.0)
    }
}

impl std::error::Error for UnknownEntryKind {}

impl std::str::FromStr for EntryKind {
    type Err = UnknownEntryKind;

    /// Case-insensitive; snake_case names plus legacy "gpx"/"gpxfile"/"terraintileset".
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "model" => Ok(EntryKind::Model),
            "texture" => Ok(EntryKind::Texture),
            "sprite" => Ok(EntryKind::Sprite),
            "material" => Ok(EntryKind::Material),
            "skybox" => Ok(EntryKind::Skybox),
            "surface" => Ok(EntryKind::Surface),
            "visual_layer" => Ok(EntryKind::VisualLayer),
            "visual_scope" => Ok(EntryKind::VisualScope),
            "sound" => Ok(EntryKind::Sound),
            "gpx_track" | "gpx" | "gpxfile" | "gpx_file" => Ok(EntryKind::GpxTrack),
            "geo_json" => Ok(EntryKind::GeoJson),
            "terrain_tileset" | "terraintileset" => Ok(EntryKind::TerrainTileset),
            "script" => Ok(EntryKind::Script),
            _ => Err(UnknownEntryKind(text.to_string())),
        }
    }
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
