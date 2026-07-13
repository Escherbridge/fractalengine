//! Composite tile source with ordered fallback chain.
//!
//! `CompositeTileSource` tries hexon sources first (by bounds check),
//! then disk cache, then online sources. Results from online fetches
//! are written back to the disk cache.

use super::cache::DiskTileCache;
use super::hexon_source::HexonTileSource;
use super::source::TileCoord;

/// Reconciled real-meter metric frame across all composite sources (FR-4).
///
/// `finest_gsd_m` is the smallest (highest-resolution) per-source GSD, used
/// as the common frame's reference resolution; `native_scale` mirrors the
/// source that contributed it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReconciledMetricFrame {
    pub finest_gsd_m: f64,
    pub native_scale: f64,
}

/// Per-source LOD/zoom pick derived from GSD rather than blind `covers()` order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceLodPick {
    pub source_index: usize,
    pub zoom: u8,
}

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

/// Resolve `(gsd_m, native_scale)` for a source's meta, deriving from
/// bounds+zoom (Web-Mercator) when the meta lacks declared scale fields.
fn resolved_gsd_and_scale(meta: &fe_format::manifest::TilesetMeta) -> (f64, f64) {
    match (meta.ground_sample_distance_m, meta.native_scale) {
        (Some(gsd), Some(scale)) => (gsd, scale),
        _ => fe_format::manifest::derive_scale_from_bounds(
            meta.bounds,
            meta.max_zoom,
            meta.tile_size,
        ),
    }
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

    /// Read-only access to each source's [`fe_format::manifest::TilesetMeta`],
    /// retained per-source for FR-4 reconciliation (in source-add order).
    pub fn source_metas(&self) -> Vec<&fe_format::manifest::TilesetMeta> {
        self.hexon_sources.iter().map(|s| &s.tileset_meta).collect()
    }

    /// Baked splat coverage buffer for a tile, if any covering hexon source carries one (FR-4).
    pub fn get_baked_splats_sync(
        &self,
        coord: TileCoord,
    ) -> Option<&crate::splat::format::BakedSplatBuffer> {
        self.hexon_sources
            .iter()
            .find_map(|src| src.covers(coord).then(|| src.get_baked_splats(coord)).flatten())
    }

    /// Reconcile all hexon sources into one common real-meter metric frame
    /// (FR-4): the finest (smallest) GSD across sources, backfilling any
    /// source missing scale fields via `derive_scale_from_bounds`. Returns
    /// `None` if no sources are present.
    pub fn reconcile_metric_frame(&self) -> Option<ReconciledMetricFrame> {
        self.hexon_sources
            .iter()
            .map(|s| resolved_gsd_and_scale(&s.tileset_meta))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(gsd, scale)| ReconciledMetricFrame {
                finest_gsd_m: gsd,
                native_scale: scale,
            })
    }

    /// Select each source's zoom level from its own GSD rather than blindly
    /// taking the first `covers()` hit (FR-4): the source whose native zoom
    /// range's max level yields the closest-to-target GSD is picked.
    ///
    /// `target_gsd_m` is the desired ground-sample-distance (meters/pixel);
    /// smaller means higher resolution requested.
    pub fn select_source_lods(&self, target_gsd_m: f64) -> Vec<SourceLodPick> {
        self.hexon_sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let meta = &source.tileset_meta;
                let (native_gsd, _) = resolved_gsd_and_scale(meta);
                // Approximate GSD doubles per zoom level down from max_zoom.
                let zoom = if target_gsd_m <= native_gsd {
                    meta.max_zoom
                } else {
                    let levels_down = (target_gsd_m / native_gsd).log2().floor().max(0.0) as i32;
                    let candidate = meta.max_zoom as i32 - levels_down;
                    candidate.clamp(meta.min_zoom as i32, meta.max_zoom as i32) as u8
                };
                SourceLodPick { source_index: index, zoom }
            })
            .collect()
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

    /// Synchronous satellite lookup — hexon sources and disk cache (`composite_sat`) only.
    pub fn get_satellite_tile_sync(&self, coord: TileCoord) -> Option<Vec<u8>> {
        use crate::config::TileSourceMode;

        match self.tile_source_mode {
            TileSourceMode::Offline => {
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_satellite_tile(coord) {
                            return Some(data.to_vec());
                        }
                    }
                }
                None
            }
            TileSourceMode::Online => self
                .cache
                .as_ref()
                .and_then(|c| c.get("composite_sat", &coord.cache_key())),
            TileSourceMode::Hybrid => {
                for src in &self.hexon_sources {
                    if src.covers(coord) {
                        if let Some(data) = src.get_satellite_tile(coord) {
                            return Some(data.to_vec());
                        }
                    }
                }
                self.cache
                    .as_ref()
                    .and_then(|c| c.get("composite_sat", &coord.cache_key()))
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

    fn make_hexon_source(
        tiles: Vec<(String, Vec<u8>)>,
        satellite: Vec<(String, Vec<u8>)>,
    ) -> HexonTileSource {
        make_hexon_source_with_zoom(tiles, satellite, 10, 12)
    }

    fn make_hexon_source_with_zoom(
        tiles: Vec<(String, Vec<u8>)>,
        satellite: Vec<(String, Vec<u8>)>,
        min_zoom: u8,
        max_zoom: u8,
    ) -> HexonTileSource {
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom,
            max_zoom,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: !satellite.is_empty(),
            tile_count: tiles.len() as u32,
            satellite_tile_count: satellite.len() as u32,
            region_name: "Test".into(),
            parent_tileset: None,
            chunk_index: None,
            native_scale: None,
            ground_sample_distance_m: None,
            crs: None,
            scale_bounds: None,
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
        let bytes =
            HexonArchive::export_tileset(manifest, &meta, &tiles, &satellite, None).unwrap();
        HexonTileSource::from_archive(&bytes).unwrap()
    }

    #[test]
    fn composite_sync_from_hexon() {
        // Use a tile coordinate that falls within bounds [45.0, -122.0, 46.0, -121.0]
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        let key = coord.cache_key();
        let source = make_hexon_source(vec![(key, vec![42u8; 16])], vec![]);
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
        assert!(composite.get_satellite_tile_sync(coord).is_none());
    }

    #[test]
    fn composite_satellite_sync_from_hexon() {
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        let key = coord.cache_key();
        let source = make_hexon_source(vec![], vec![(key, vec![7u8; 8])]);
        let mut composite = CompositeTileSource::new();
        composite.add_hexon_source(source);

        assert_eq!(composite.get_satellite_tile_sync(coord).unwrap(), vec![7u8; 8]);
        // Elevation lookup must not see satellite tiles.
        assert!(composite.get_tile_sync(coord).is_none());
    }

    #[test]
    fn composite_satellite_sync_respects_offline_mode() {
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        let key = coord.cache_key();
        let source = make_hexon_source(vec![], vec![(key, vec![9u8; 4])]);
        let mut composite = CompositeTileSource::new();
        composite.add_hexon_source(source);
        composite.set_tile_source_mode(crate::config::TileSourceMode::Offline);

        assert_eq!(composite.get_satellite_tile_sync(coord).unwrap(), vec![9u8; 4]);
    }

    #[test]
    fn reconcile_metric_frame_picks_finest_gsd_across_mixed_sources() {
        // Coarser source: max_zoom 10 → larger tile → larger GSD.
        let coarse = make_hexon_source_with_zoom(vec![], vec![], 5, 10);
        // Finer source: max_zoom 16 → smaller tile → smaller GSD.
        let fine = make_hexon_source_with_zoom(vec![], vec![], 12, 16);

        let coarse_gsd = resolved_gsd_and_scale(&coarse.tileset_meta).0;
        let fine_gsd = resolved_gsd_and_scale(&fine.tileset_meta).0;
        assert!(fine_gsd < coarse_gsd, "finer source must have smaller GSD");

        let mut composite = CompositeTileSource::new();
        composite.add_hexon_source(coarse);
        composite.add_hexon_source(fine);

        let frame = composite.reconcile_metric_frame().expect("frame present");
        assert!((frame.finest_gsd_m - fine_gsd).abs() < 1e-6);
    }

    #[test]
    fn reconcile_metric_frame_none_when_empty() {
        let composite = CompositeTileSource::new();
        assert!(composite.reconcile_metric_frame().is_none());
    }

    #[test]
    fn select_source_lods_differ_by_gsd_for_high_resolution_target() {
        let coarse = make_hexon_source_with_zoom(vec![], vec![], 5, 10);
        let fine = make_hexon_source_with_zoom(vec![], vec![], 12, 16);
        let coarse_gsd = resolved_gsd_and_scale(&coarse.tileset_meta).0;

        let mut composite = CompositeTileSource::new();
        composite.add_hexon_source(coarse);
        composite.add_hexon_source(fine);

        // Ask for a resolution much finer than the coarse source's native GSD;
        // the coarse source should be clamped to its own max_zoom, the fine
        // source can actually satisfy it at (or near) its max_zoom.
        let target = coarse_gsd / 8.0;
        let picks = composite.select_source_lods(target);
        assert_eq!(picks.len(), 2);
        assert_ne!(picks[0].zoom, 0);
        // Coarse source clamped at its own max_zoom since it cannot go finer.
        assert_eq!(picks[0].zoom, 10);
        // Fine source should pick a higher zoom than the coarse source did.
        assert!(picks[1].zoom >= picks[0].zoom);
    }
}
