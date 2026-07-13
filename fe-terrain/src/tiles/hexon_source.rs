//! Offline tile source backed by a `.hexon` archive.
//!
//! `HexonTileSource` loads elevation and satellite tiles from a hexon archive
//! into memory, then serves them synchronously via HashMap lookup. No network
//! access required.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use fe_format::manifest::TilesetMeta;
use fe_format::HexonArchive;

use crate::splat::format::{read_baked_splats_from_archive, BakedSplatBuffer};

use super::source::TileCoord;

/// Offline tile source loaded from a `.hexon` archive.
///
/// All tiles are held in memory as `HashMap<cache_key, bytes>`.
/// Cache keys use the `{z}/{x}/{y}` format.
pub struct HexonTileSource {
    pub tileset_meta: TilesetMeta,
    /// Elevation tiles: cache_key → PNG bytes.
    pub tiles: HashMap<String, Vec<u8>>,
    /// Satellite tiles: cache_key → JPG bytes.
    pub satellite: HashMap<String, Vec<u8>>,
    /// Baked splat coverage buffers (track `splat_hexon_bake_20260712`, FR-2/FR-4):
    /// cache_key → dense splat buffer, precomputed at build time. Absent for
    /// tiles/hexons built before this track or for tiles the bake step skipped
    /// — callers fall back to live `synthesize_splats` (FR-5).
    pub baked_splats: HashMap<String, BakedSplatBuffer>,
}

impl HexonTileSource {
    /// Load a tileset from raw `.hexon` archive bytes.
    pub fn from_archive(bytes: &[u8]) -> Result<Self> {
        let data = HexonArchive::import(bytes).context("failed to import hexon archive")?;
        let tileset_meta = data
            .tileset_meta
            .context("archive is missing tileset_meta (not a TerrainTileset hexon)")?;

        let tiles: HashMap<String, Vec<u8>> = data.elevation_tiles.into_iter().collect();
        let satellite: HashMap<String, Vec<u8>> = data.satellite_tiles.into_iter().collect();
        // Best-effort: a malformed/absent splats section never fails the whole
        // hexon load (FR-5) — read_baked_splats_from_archive already degrades
        // to an empty vec on any error.
        let baked_splats: HashMap<String, BakedSplatBuffer> = read_baked_splats_from_archive(bytes)
            .unwrap_or_default()
            .into_iter()
            .collect();

        Ok(Self {
            tileset_meta,
            tiles,
            satellite,
            baked_splats,
        })
    }

    /// Load a tileset from a directory layout (for development/pre-packaging).
    ///
    /// Expected layout:
    /// ```text
    /// dir/
    ///   tileset_meta.json
    ///   tiles/{z}/{x}/{y}.png
    ///   satellite/{z}/{x}/{y}.jpg   (optional)
    /// ```
    pub fn from_directory(dir: &Path) -> Result<Self> {
        let meta_path = dir.join("tileset_meta.json");
        let meta_str =
            std::fs::read_to_string(&meta_path).context("failed to read tileset_meta.json")?;
        let tileset_meta: TilesetMeta =
            serde_json::from_str(&meta_str).context("invalid tileset_meta.json")?;

        let mut tiles = HashMap::new();
        let tiles_dir = dir.join("tiles");
        if tiles_dir.is_dir() {
            walk_tiles(&tiles_dir, "png", &mut tiles)?;
        }

        let mut satellite = HashMap::new();
        let sat_dir = dir.join("satellite");
        if sat_dir.is_dir() {
            walk_tiles(&sat_dir, "jpg", &mut satellite)?;
        }

        Ok(Self {
            tileset_meta,
            tiles,
            satellite,
            // Directory layout is dev-only pre-packaging; no baked-splats
            // convention there yet — falls back to live synthesis (FR-5).
            baked_splats: HashMap::new(),
        })
    }

    /// Synchronous tile lookup by coordinate.
    pub fn get_tile(&self, coord: TileCoord) -> Option<&[u8]> {
        self.tiles.get(&coord.cache_key()).map(|v| v.as_slice())
    }

    /// Synchronous satellite tile lookup by coordinate.
    pub fn get_satellite_tile(&self, coord: TileCoord) -> Option<&[u8]> {
        self.satellite.get(&coord.cache_key()).map(|v| v.as_slice())
    }

    /// Check if an elevation tile exists for the given coordinate.
    pub fn has_tile(&self, coord: TileCoord) -> bool {
        self.tiles.contains_key(&coord.cache_key())
    }

    /// Baked splat coverage buffer for a tile, if the hexon carries one
    /// (FR-4). `None` means the runtime should fall back to live
    /// `synthesize_splats` at native density (FR-5).
    pub fn get_baked_splats(&self, coord: TileCoord) -> Option<&BakedSplatBuffer> {
        self.baked_splats.get(&coord.cache_key())
    }

    /// Geographic bounding box `[min_lat, min_lon, max_lat, max_lon]`.
    pub fn bounds(&self) -> [f64; 4] {
        self.tileset_meta.bounds
    }

    /// `(min_zoom, max_zoom)` range covered by this tileset.
    pub fn zoom_range(&self) -> (u8, u8) {
        (self.tileset_meta.min_zoom, self.tileset_meta.max_zoom)
    }

    /// Check if a coordinate falls within this tileset's bounds and zoom range.
    ///
    /// Uses the tile's full geographic extent (its NW corner to the NW corner of
    /// the next tile), not just its NW-corner point — a coarse-zoom tile can serve
    /// a region while its own NW corner lies just outside a tightly-drawn bounds box.
    pub fn covers(&self, coord: TileCoord) -> bool {
        let (min_z, max_z) = self.zoom_range();
        if coord.zoom < min_z || coord.zoom > max_z {
            return false;
        }
        // Tile geographic extent: NW corner of this tile → NW corner of the (x+1, y+1) tile.
        let (nw_lat, nw_lon) = coord.to_lat_lon();
        let (se_lat, se_lon) = TileCoord::new(coord.x + 1, coord.y + 1, coord.zoom).to_lat_lon();
        let tile_min_lat = se_lat.min(nw_lat);
        let tile_max_lat = se_lat.max(nw_lat);
        let tile_min_lon = nw_lon.min(se_lon);
        let tile_max_lon = nw_lon.max(se_lon);
        let [min_lat, min_lon, max_lat, max_lon] = self.tileset_meta.bounds;
        // Rectangle intersection between the tile's extent and the tileset bounds.
        tile_min_lat <= max_lat
            && tile_max_lat >= min_lat
            && tile_min_lon <= max_lon
            && tile_max_lon >= min_lon
    }
}

#[cfg(feature = "fetch")]
#[async_trait::async_trait]
impl super::source::TileSource for HexonTileSource {
    async fn fetch_tile(&self, coord: TileCoord) -> Result<Vec<u8>> {
        self.get_tile(coord)
            .map(|s| s.to_vec())
            .ok_or_else(|| anyhow::anyhow!("tile not found in hexon: {}", coord.cache_key()))
    }

    fn source_id(&self) -> &str {
        &self.tileset_meta.region_name
    }
}

/// Walk a `{z}/{x}/{y}.{ext}` directory tree, collecting tiles into the map.
fn walk_tiles(
    base: &Path,
    ext: &str,
    out: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    let z_entries = std::fs::read_dir(base).context("failed to read tiles directory")?;
    for z_entry in z_entries.flatten() {
        let z_path = z_entry.path();
        if !z_path.is_dir() {
            continue;
        }
        let z_name = z_entry.file_name();
        let z_str = z_name.to_string_lossy();

        let x_entries = match std::fs::read_dir(&z_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for x_entry in x_entries.flatten() {
            let x_path = x_entry.path();
            if !x_path.is_dir() {
                continue;
            }
            let x_name = x_entry.file_name();
            let x_str = x_name.to_string_lossy();

            let y_entries = match std::fs::read_dir(&x_path) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for y_entry in y_entries.flatten() {
                let y_path = y_entry.path();
                let y_name = y_entry.file_name();
                let y_str = y_name.to_string_lossy();
                let suffix = format!(".{ext}");
                if y_str.ends_with(&suffix) {
                    let y_num = y_str.strip_suffix(&suffix).unwrap_or(&y_str);
                    let cache_key = format!("{z_str}/{x_str}/{y_num}");
                    let data = std::fs::read(&y_path)
                        .with_context(|| format!("failed to read tile {cache_key}"))?;
                    out.insert(cache_key, data);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::HexonArchive;

    fn test_tileset_meta() -> TilesetMeta {
        TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: 1,
            satellite_tile_count: 0,
            region_name: "Test Region".into(),
            parent_tileset: None,
            chunk_index: None,
            native_scale: None,
            ground_sample_distance_m: None,
            crs: None,
            scale_bounds: None,
        }
    }

    fn test_manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "test-tileset".into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Test Tileset".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            signature: None,
        }
    }

    #[test]
    fn roundtrip_hexon_tile_source() {
        let meta = test_tileset_meta();
        let elevation = vec![("10/512/340".to_string(), vec![0u8; 64])];
        let satellite = vec![];

        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &elevation, &satellite, None)
                .expect("export failed");

        let source =
            HexonTileSource::from_archive(&archive_bytes).expect("from_archive failed");

        assert_eq!(source.bounds(), [45.0, -122.0, 46.0, -121.0]);
        assert_eq!(source.zoom_range(), (10, 12));
        assert!(source.has_tile(TileCoord::new(512, 340, 10)));
        assert!(!source.has_tile(TileCoord::new(0, 0, 10)));

        let data = source.get_tile(TileCoord::new(512, 340, 10)).unwrap();
        assert_eq!(data.len(), 64);
    }

    #[test]
    fn loads_baked_splats_when_present() {
        use crate::splat::format::{append_baked_splats_to_archive, encode_baked_splats};

        let meta = test_tileset_meta();
        let elevation = vec![("10/512/340".to_string(), vec![0u8; 64])];
        let base = HexonArchive::export_tileset(test_manifest(), &meta, &elevation, &[], None)
            .expect("export failed");

        let baked = BakedSplatBuffer {
            version: 1,
            positions: vec![[1.0, 2.0, 3.0]],
            colors: vec![[1.0, 1.0, 1.0, 1.0]],
            scales: vec![[1.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]],
        };
        let entries = vec![("10/512/340".to_string(), encode_baked_splats(&baked).unwrap())];
        let augmented = append_baked_splats_to_archive(&base, &entries).unwrap();

        let source = HexonTileSource::from_archive(&augmented).expect("from_archive failed");
        let coord = TileCoord::new(512, 340, 10);
        let loaded = source.get_baked_splats(coord).expect("baked splats should load");
        assert_eq!(loaded.positions, baked.positions);
        assert!(source.get_baked_splats(TileCoord::new(0, 0, 10)).is_none());
    }

    #[test]
    fn no_baked_splats_falls_back_to_empty_map() {
        let meta = test_tileset_meta();
        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &[], &[], None).unwrap();
        let source = HexonTileSource::from_archive(&archive_bytes).unwrap();
        assert!(source.baked_splats.is_empty());
        assert!(source.get_baked_splats(TileCoord::new(512, 340, 10)).is_none());
    }

    #[test]
    fn covers_checks_bounds_and_zoom() {
        let meta = test_tileset_meta();
        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &[], &[], None).unwrap();
        let source = HexonTileSource::from_archive(&archive_bytes).unwrap();

        // A tile at zoom 11 within bounds should be covered
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        assert!(source.covers(coord));

        // Zoom 5 is outside range
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 5);
        assert!(!source.covers(coord));
    }
}
