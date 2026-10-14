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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Hexon-declared world units per real meter; authoritative render scale.
    /// `None` on legacy archives — see `src/AGENTS.md` §scale for backfill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_scale: Option<f64>,
    /// Ground sample distance in meters/pixel at the tileset's native zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_sample_distance_m: Option<f64>,
    /// Coordinate reference system identifier; recorded, not reprojected.
    /// `None` on legacy archives and stays absent on re-serialize (see `crs_or_default`).
    #[serde(default = "default_crs", skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    /// `[min, max]` world-scale bounds a user nudge may clamp within.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_bounds: Option<[f64; 2]>,
}

/// Default CRS for tilesets predating the `crs` field.
fn default_crs() -> Option<String> {
    Some("EPSG:4326".to_string())
}

impl TilesetMeta {
    /// Effective CRS, defaulting absent (legacy) values to EPSG:4326.
    pub fn crs_or_default(&self) -> &str {
        self.crs.as_deref().unwrap_or("EPSG:4326")
    }
}

/// Manual `Default` (not `#[derive(Default)]`) because `crs` must agree with
/// what serde produces for a crs-less archive (`default_crs()`, i.e.
/// `Some("EPSG:4326")`) — a derived `Default` would give `None` instead and
/// silently disagree with `legacy_tileset_meta_deserializes_with_defaults`.
/// Every other field uses the type's own zero value, matching every other
/// field's `#[serde(default)]` (plain `Option::is_none`/`0`/`false`).
impl Default for TilesetMeta {
    fn default() -> Self {
        Self {
            bounds: [0.0; 4],
            min_zoom: 0,
            max_zoom: 0,
            tile_size: 0,
            elevation_encoding: ElevationEncoding::default(),
            has_satellite: false,
            tile_count: 0,
            satellite_tile_count: 0,
            region_name: String::new(),
            parent_tileset: None,
            chunk_index: None,
            native_scale: None,
            ground_sample_distance_m: None,
            crs: default_crs(),
            scale_bounds: None,
        }
    }
}

/// Elevation tile encoding format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ElevationEncoding {
    /// Mapbox Terrain-RGB: elevation = -10000 + (R*256*256 + G*256 + B) * 0.1
    #[default]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkIndex {
    /// Unique ID for the complete tileset this chunk belongs to.
    pub tileset_id: String,
    /// This chunk's sequence number (0-indexed).
    pub chunk_seq: u32,
    /// Total number of chunks in the tileset.
    pub total_chunks: u32,
    /// Bounds of this specific chunk [min_lat, min_lon, max_lat, max_lon].
    pub chunk_bounds: [f64; 4],
    /// blake3 hex digest of this chunk's exported archive bytes (D-76 per-chunk
    /// hash slot; PAS-P5 signs it). `None` on legacy chunks predating this field —
    /// `#[serde(default, skip_serializing_if)]` keeps them byte-identical on re-export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
}

/// One bundle's entry in a [`TilesetBundleIndex`]: its sequence, integrity
/// hash, size, and the tile-coordinate range it covers (see
/// `fe-terrain/src/AGENTS.md` §bundles for how bundles are built).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleEntry {
    /// This bundle's sequence number (0-indexed), matching its `ChunkIndex.chunk_seq`.
    pub seq: u32,
    /// blake3 hex digest of the bundle's exported archive bytes.
    pub blake3_hex: String,
    /// Size of the exported archive in bytes.
    pub byte_len: u64,
    /// Number of elevation tiles the bundle carries.
    pub tile_count: u32,
    /// Zoom level this bundle's tile range belongs to.
    pub zoom: u8,
    /// Inclusive `[min, max]` tile-x range covered by this bundle.
    pub x_range: [u32; 2],
    /// Inclusive `[min, max]` tile-y range covered by this bundle.
    pub y_range: [u32; 2],
    /// Inclusive `[min, max]` Hilbert-distance range covered by this bundle
    /// (evidence/debugging only — `bundle_for_tile` never reads it).
    pub hilbert_range: [u64; 2],
}

/// Deterministic spatial-bundle index for a tileset (D-73 A4): the tileset is
/// split into Hilbert-ordered, size-capped bundles instead of flat chunks.
/// Ships as the opaque HashSeq child 0 (fe-sync never parses it; see
/// `fe-sync/src/AGENTS.md` §tileset-transfer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TilesetBundleIndex {
    /// Index format version, for forward-compatible parsing.
    pub schema: u32,
    /// The tileset id all bundles share (becomes the installed hexon's id).
    pub tileset_id: String,
    /// The tileset's metadata with `chunk_index` always `None` — per-bundle
    /// chunk metadata lives in `bundles`, not here.
    pub tileset_meta: TilesetMeta,
    pub bundles: Vec<BundleEntry>,
}

impl TilesetBundleIndex {
    /// The one tile → bundle-sequence lookup (critic C4: `fe-terrain`'s
    /// `StreamedTileSource` and PAS-P5 both call this instead of
    /// re-implementing a chunk table).
    pub fn bundle_for_tile(&self, zoom: u8, x: u32, y: u32) -> Option<u32> {
        self.bundles
            .iter()
            .find(|b| {
                b.zoom == zoom
                    && x >= b.x_range[0]
                    && x <= b.x_range[1]
                    && y >= b.y_range[0]
                    && y <= b.y_range[1]
            })
            .map(|b| b.seq)
    }
}

/// Web-Mercator equatorial circumference in meters (mirrors `fe-terrain::tile_world_size_m`).
const WEB_MERCATOR_EQUATOR_M: f64 = 40_075_016.686;

/// Derive `(ground_sample_distance_m, native_scale)` from bounds + zoom via
/// Web-Mercator when a legacy tileset has no declared scale fields.
///
/// `native_scale` assumes 1 tile == `tile_size` world units at native zoom
/// (i.e. world units per real meter = `tile_size / tile_world_size_m`).
pub fn derive_scale_from_bounds(bounds: [f64; 4], max_zoom: u8, tile_size: u16) -> (f64, f64) {
    /// Clamp latitude to the standard Web-Mercator limit so `cos(lat)` never hits 0.
    const MAX_ABS_LAT: f64 = 85.0;
    let center_lat = ((bounds[0] + bounds[2]) / 2.0).clamp(-MAX_ABS_LAT, MAX_ABS_LAT);
    let tile_world_size_m =
        WEB_MERCATOR_EQUATOR_M * center_lat.to_radians().cos() / 2f64.powi(max_zoom as i32);
    let gsd = if tile_size > 0 {
        tile_world_size_m / tile_size as f64
    } else {
        tile_world_size_m
    };
    let native_scale = if tile_world_size_m > 0.0 {
        tile_size as f64 / tile_world_size_m
    } else {
        1.0
    };
    (gsd, native_scale)
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

#[cfg(test)]
mod scale_field_tests {
    use super::*;

    /// A legacy `.hexon` tileset_meta.json blob predating scale fields.
    fn legacy_tileset_meta_json() -> serde_json::Value {
        serde_json::json!({
            "bounds": [45.0, -122.0, 46.0, -121.0],
            "min_zoom": 10,
            "max_zoom": 12,
            "tile_size": 256,
            "elevation_encoding": "terrain_rgb",
            "has_satellite": false,
            "tile_count": 4,
            "satellite_tile_count": 0,
            "region_name": "Legacy Region"
        })
    }

    #[test]
    fn legacy_tileset_meta_deserializes_with_defaults() {
        let meta: TilesetMeta =
            serde_json::from_value(legacy_tileset_meta_json()).expect("legacy blob must parse");
        assert!(meta.native_scale.is_none());
        assert!(meta.ground_sample_distance_m.is_none());
        assert_eq!(meta.crs.as_deref(), Some("EPSG:4326"));
        assert!(meta.scale_bounds.is_none());
    }

    /// The test that would go red if `Default` and serde's per-field defaults
    /// ever drift: a legacy archive (only the original required fields) must
    /// deserialize to exactly what a producer gets from
    /// `TilesetMeta { ..required fields.., ..Default::default() }`.
    #[test]
    fn default_agrees_with_serde_defaults_for_a_legacy_archive() {
        let from_json: TilesetMeta =
            serde_json::from_value(legacy_tileset_meta_json()).expect("legacy blob must parse");
        let from_default = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: 4,
            satellite_tile_count: 0,
            region_name: "Legacy Region".into(),
            ..Default::default()
        };
        assert_eq!(from_json, from_default);
    }

    #[test]
    fn scale_fields_roundtrip_through_serde() {
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: 4,
            satellite_tile_count: 0,
            region_name: "Test".into(),
            native_scale: Some(0.001),
            ground_sample_distance_m: Some(2.5),
            crs: Some("EPSG:3857".into()),
            scale_bounds: Some([0.0001, 1.0]),
            ..Default::default()
        };
        let json = serde_json::to_value(&meta).unwrap();
        let parsed: TilesetMeta = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.native_scale, Some(0.001));
        assert_eq!(parsed.ground_sample_distance_m, Some(2.5));
        assert_eq!(parsed.crs.as_deref(), Some("EPSG:3857"));
        assert_eq!(parsed.scale_bounds, Some([0.0001, 1.0]));
    }

    #[test]
    fn derive_scale_matches_web_mercator_tile_size_at_center_lat() {
        let bounds = [45.0, -122.0, 46.0, -121.0];
        let center_lat: f64 = (bounds[0] + bounds[2]) / 2.0;
        let max_zoom = 12u8;
        let tile_size = 256u16;

        let expected_tile_world_size_m =
            WEB_MERCATOR_EQUATOR_M * center_lat.to_radians().cos() / 2f64.powi(max_zoom as i32);

        let (gsd, native_scale) = derive_scale_from_bounds(bounds, max_zoom, tile_size);

        // GSD * tile_size reconstructs the tile's real-meter edge length.
        assert!((gsd * tile_size as f64 - expected_tile_world_size_m).abs() < 1e-6);
        // native_scale is the inverse relationship (world units per meter).
        assert!((native_scale * expected_tile_world_size_m - tile_size as f64).abs() < 1e-6);
        assert!(gsd > 0.0 && gsd != 1.0);
        assert!(native_scale > 0.0 && native_scale != 1.0);
    }
}

#[cfg(test)]
mod bundle_index_tests {
    use super::*;

    fn index_with_one_bundle() -> TilesetBundleIndex {
        TilesetBundleIndex {
            schema: 1,
            tileset_id: "t".into(),
            tileset_meta: TilesetMeta {
                bounds: [0.0; 4],
                min_zoom: 10,
                max_zoom: 10,
                tile_size: 256,
                elevation_encoding: ElevationEncoding::TerrainRgb,
                tile_count: 4,
                region_name: "x".into(),
                ..Default::default()
            },
            bundles: vec![BundleEntry {
                seq: 0,
                blake3_hex: "deadbeef".into(),
                byte_len: 10,
                tile_count: 4,
                zoom: 10,
                x_range: [2, 3],
                y_range: [5, 6],
                hilbert_range: [0, 3],
            }],
        }
    }

    /// (critic) The one tile -> bundle lookup, table-driven: inside, on a
    /// range border, outside, and wrong zoom.
    #[test]
    fn bundle_for_tile_table() {
        let index = index_with_one_bundle();
        let cases: &[(&str, u8, u32, u32, Option<u32>)] = &[
            ("inside the range", 10, 2, 5, Some(0)),
            ("on the x border", 10, 3, 5, Some(0)),
            ("on the y border", 10, 2, 6, Some(0)),
            ("outside the x range", 10, 4, 5, None),
            ("outside the y range", 10, 2, 7, None),
            ("below the x range", 10, 1, 5, None),
            ("wrong zoom", 11, 2, 5, None),
        ];
        for &(desc, zoom, x, y, expected) in cases {
            assert_eq!(index.bundle_for_tile(zoom, x, y), expected, "case: {desc}");
        }
    }

    #[test]
    fn chunk_index_blake3_defaults_absent_for_legacy_json() {
        let legacy = serde_json::json!({
            "tileset_id": "t",
            "chunk_seq": 0,
            "total_chunks": 1,
            "chunk_bounds": [0.0, 0.0, 0.0, 0.0]
        });
        let parsed: ChunkIndex = serde_json::from_value(legacy).expect("legacy chunk must parse");
        assert!(parsed.blake3.is_none());

        // And re-serializing a `None` stays byte-identical to the legacy shape
        // (no `blake3` key emitted), per `#[serde(skip_serializing_if)]`.
        let json = serde_json::to_value(&parsed).unwrap();
        assert!(json.get("blake3").is_none());
    }
}
