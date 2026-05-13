use serde::{Deserialize, Serialize};

/// The type of content a `.hexon` archive contains.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HexonType {
    Scene,
    Model,
    Material,
    Skybox,
    Terrain,
    /// Pre-baked elevation/satellite tile data for offline terrain rendering.
    /// Contains `terrain/tiles/{z}/{x}/{y}.png` (elevation) and optionally
    /// `terrain/satellite/{z}/{x}/{y}.jpg` (imagery). Manifest includes
    /// `tileset_meta` with bounds, zoom range, encoding, and tile count.
    TerrainTileset,
    GpxCollection,
    Surface,
    Sound,
    VisualLayer,
    Theme,
    Bundle,
}

/// Metadata for a `TerrainTileset` hexon.
///
/// Stored in `terrain/tileset_meta.json` inside the archive.
/// Describes the geographic bounds, zoom levels, tile encoding,
/// and distribution/chunking information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TilesetMeta {
    /// Geographic bounding box [min_lat, min_lon, max_lat, max_lon] (WGS84).
    pub bounds: [f64; 4],
    /// Minimum zoom level included.
    pub min_zoom: u8,
    /// Maximum zoom level included.
    pub max_zoom: u8,
    /// Tile pixel dimensions (typically 256 or 512).
    pub tile_size: u16,
    /// Elevation data encoding format.
    pub elevation_encoding: ElevationEncoding,
    /// Whether satellite imagery tiles are included.
    pub has_satellite: bool,
    /// Total number of elevation tiles in the archive.
    pub tile_count: u32,
    /// Total number of satellite tiles (0 if none).
    pub satellite_tile_count: u32,
    /// Human-readable region name (e.g., "North America — Pacific Northwest").
    pub region_name: String,
    /// Optional parent tileset hexon URI for lower zoom levels.
    /// Enables cascading: a regional hexon can reference a continental base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tileset: Option<String>,
    /// Chunk index for relay distribution. When a large tileset is split
    /// into relay-friendly chunks, each chunk carries its sequence number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<ChunkIndex>,
}

/// Elevation tile encoding format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationEncoding {
    /// Mapbox Terrain-RGB: elevation = -10000 + (R*256*256 + G*256 + B) * 0.1
    TerrainRgb,
    /// Mapzen Terrarium: elevation = (R*256 + G + B/256) - 32768
    Terrarium,
    /// Raw 16-bit heightmap (little-endian u16, meters).
    Raw16,
}

/// Chunk index for relay-distributed tilesets.
///
/// Large tilesets are split into chunks that can be fetched independently
/// over P2P relays. Each chunk covers a contiguous spatial region at the
/// same zoom range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    /// Unique ID for the complete tileset this chunk belongs to.
    pub tileset_id: String,
    /// This chunk's sequence number (0-indexed).
    pub chunk_seq: u32,
    /// Total number of chunks in the tileset.
    pub total_chunks: u32,
    /// Bounds of this specific chunk [min_lat, min_lon, max_lat, max_lon].
    pub chunk_bounds: [f64; 4],
}

/// A dependency on another hexon package, referenced by URI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonDep {
    pub hexon_uri: String,
    pub version_req: String,
}

/// A target platform constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
}

/// amp-compatible 3-level tag hierarchy using 128-bit UIDs as hex strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

/// The main manifest for a `.hexon` archive (v1.0.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonManifest {
    pub schema_version: String,
    pub hexon_id: String,
    pub hexon_type: HexonType,
    pub publisher_did: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher_name: Option<String>,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_peer_did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approx_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_engine_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage_url: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<HexonDep>,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<HexonAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
