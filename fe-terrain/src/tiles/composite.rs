//! Composite tile source with ordered fallback chain.
//!
//! `CompositeTileSource` tries hexon sources first (by bounds check),
//! then disk cache, then online sources. Results from online fetches
//! are written back to the disk cache.

use super::cache::DiskTileCache;
use super::hexon_source::HexonTileSource;
use super::source::TileCoord;

/// Ordered fallback tile source.
///
/// Lookup order:
/// 1. Hexon sources (checked by `covers()` — bounds + zoom range)
/// 2. Disk cache
/// 3. Online sources (results cached to disk)
pub struct CompositeTileSource {
    hexon_sources: Vec<HexonTileSource>,
    cache: Option<DiskTileCache>,
    #[cfg(feature = "fetch")]
    online_sources: Vec<Box<dyn super::source::TileSource>>,
    tile_source_mode: crate::config::TileSourceMode,
}

impl CompositeTileSource {
    pub fn new() -> Self {
        Self {
            hexon_sources: Vec::new(),
            cache: None,
            #[cfg(feature = "fetch")]
            online_sources: Vec::new(),
            tile_source_mode: crate::config::TileSourceMode::Hybrid,
        }
    }

    /// Set the tile resolution mode.
    pub fn set_tile_source_mode(&mut self, mode: crate::config::TileSourceMode) {
        self.tile_source_mode = mode;
    }

    /// Check if demo mode is active (no hexons loaded + Hybrid mode + FE_DEMO_TILE_URL set).
    pub fn is_demo_mode(&self) -> bool {
        self.tile_source_mode == crate::config::TileSourceMode::Hybrid
            && self.hexon_sources.is_empty()
            && std::env::var("FE_DEMO_TILE_URL").is_ok()
    }

    /// Add a hexon tile source (checked first, by bounds).
    pub fn add_hexon_source(&mut self, source: HexonTileSource) {
        self.hexon_sources.push(source);
    }

    /// Set the disk cache layer.
    pub fn set_cache(&mut self, cache: DiskTileCache) {
        self.cache = Some(cache);
    }

    /// Add an online tile source (checked last; results written to cache).
    #[cfg(feature = "fetch")]
    pub fn add_online_source(&mut self, source: Box<dyn super::source::TileSource>) {
        self.online_sources.push(source);
    }

    /// Synchronous lookup — checks hexon sources and disk cache only.
    /// Returns `None` if tile is not available offline.
    pub fn get_tile_sync(&self, coord: TileCoord) -> Option<Vec<u8>> {
        use crate::config::TileSourceMode;

        match self.tile_source_mode {
            TileSourceMode::Offline => {
                // Only hexon sources — skip cache (may contain online tiles)
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_tile(coord) {
                            return Some(data.to_vec());
                        }
                    }
                }
                None
            }
            TileSourceMode::Online => {
                // Skip hexon sources, check cache only
                if let Some(cache) = &self.cache {
                    if let Some(data) = cache.get("composite", &coord.cache_key()) {
                        return Some(data);
                    }
                }
                None
            }
            TileSourceMode::Hybrid => {
                // 1. Hexon sources (by bounds check)
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_tile(coord) {
                            return Some(data.to_vec());
                        }
                    }
                }

                // 2. Disk cache
                if let Some(cache) = &self.cache {
                    if let Some(data) = cache.get("composite", &coord.cache_key()) {
                        return Some(data);
                    }
                }

                None
            }
        }
    }
}

#[cfg(feature = "fetch")]
impl CompositeTileSource {
    /// Async lookup with full fallback chain: hexon → cache → online.
    ///
    /// Tiles fetched from online sources are written to the disk cache.
    /// Respects `tile_source_mode`:
    /// - `Offline` → hexon only, no cache or network
    /// - `Online` → skip hexon, cache → online
    /// - `Hybrid` → hexon → cache → online (default)
    pub async fn fetch_tile(&self, coord: TileCoord) -> anyhow::Result<Vec<u8>> {
        use crate::config::TileSourceMode;

        match self.tile_source_mode {
            TileSourceMode::Offline => {
                // Only hexon sources
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_tile(coord) {
                            return Ok(data.to_vec());
                        }
                    }
                }
                anyhow::bail!(
                    "tile {} not found in hexon sources (offline mode)",
                    coord.cache_key()
                )
            }
            TileSourceMode::Online => {
                // Skip hexon, check cache then online
                if let Some(cache) = &self.cache {
                    if let Some(data) = cache.get("composite", &coord.cache_key()) {
                        return Ok(data);
                    }
                }

                for src in &self.online_sources {
                    match src.fetch_tile(coord).await {
                        Ok(data) => {
                            if let Some(cache) = &self.cache {
                                let _ = cache.put("composite", &coord.cache_key(), &data);
                            }
                            return Ok(data);
                        }
                        Err(_) => continue,
                    }
                }

                anyhow::bail!(
                    "tile {} not found in cache or online sources (online mode)",
                    coord.cache_key()
                )
            }
            TileSourceMode::Hybrid => {
                // 1. Hexon sources
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_tile(coord) {
                            return Ok(data.to_vec());
                        }
                    }
                }

                // 2. Disk cache
                if let Some(cache) = &self.cache {
                    if let Some(data) = cache.get("composite", &coord.cache_key()) {
                        return Ok(data);
                    }
                }

                // 3. Online sources — try each in order
                for src in &self.online_sources {
                    match src.fetch_tile(coord).await {
                        Ok(data) => {
                            if let Some(cache) = &self.cache {
                                let _ = cache.put("composite", &coord.cache_key(), &data);
                            }
                            return Ok(data);
                        }
                        Err(_) => continue,
                    }
                }

                anyhow::bail!(
                    "tile {} not found in any source (hexon/cache/online)",
                    coord.cache_key()
                )
            }
        }
    }
}

#[cfg(feature = "fetch")]
#[async_trait::async_trait]
impl super::source::TileSource for CompositeTileSource {
    async fn fetch_tile(&self, coord: TileCoord) -> anyhow::Result<Vec<u8>> {
        self.fetch_tile(coord).await
    }

    fn source_id(&self) -> &str {
        "composite"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::HexonArchive;

    fn make_hexon_source(tiles: Vec<(String, Vec<u8>)>) -> HexonTileSource {
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: tiles.len() as u32,
            satellite_tile_count: 0,
            region_name: "Test".into(),
            parent_tileset: None,
            chunk_index: None,
        };
        let manifest = HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "test".into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Test".into(),
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
        };
        let bytes = HexonArchive::export_tileset(manifest, &meta, &tiles, &[], None).unwrap();
        HexonTileSource::from_archive(&bytes).unwrap()
    }

    #[test]
    fn composite_sync_from_hexon() {
        // Use a tile coordinate that falls within bounds [45.0, -122.0, 46.0, -121.0]
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        let key = coord.cache_key();
        let source = make_hexon_source(vec![(key, vec![42u8; 16])]);
        let mut composite = CompositeTileSource::new();
        composite.add_hexon_source(source);

        let data = composite.get_tile_sync(coord);
        assert!(data.is_some());
        assert_eq!(data.unwrap(), vec![42u8; 16]);
    }

    #[test]
    fn composite_returns_none_for_missing() {
        let composite = CompositeTileSource::new();
        let coord = TileCoord::new(0, 0, 5);
        assert!(composite.get_tile_sync(coord).is_none());
    }
}
