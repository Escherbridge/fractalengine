//! Composite tile source with ordered fallback chain; see `src/AGENTS.md` §lazy-tiles.
//!
//! `CompositeTileSource` tries hexon sources first (by bounds check), then
//! streamed (not-installed, P2P) sources, then the disk cache, then online
//! sources. Hexon sources are shared `Arc`s owned by the `TilesetRegistry`;
//! resident lookups never touch the disk, `TileLoader` does. Streamed
//! sources never touch the disk either — a resident bundle is already fully
//! decoded in memory (§streamed-tilesets).

use std::collections::HashSet;
use std::sync::Arc;

use super::cache::DiskTileCache;
use super::entitlement::is_entitlement_refusal;
use super::hexon_source::{HexonTileSource, Probe, ResidentTile, TileLayer};
use super::source::TileCoord;
use super::streamed::StreamedTileSource;
use crate::config::TileSourceMode;
use crate::splat::format::BakedSplatBuffer;

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

/// Where a resolved layer's bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileOrigin {
    /// `hexon_sources[index]`.
    Hexon(usize),
    /// A resident bundle of a not-yet-installed `streamed_sources` entry
    /// (S7). Pinning/releasing a streamed-origin hit is coordinate-keyed
    /// (`CompositeTileSource::pin_streamed`/`release_streamed`), not
    /// index-keyed, so this variant carries no index.
    Streamed,
    /// The disk tile cache (Online / Hybrid fallback).
    DiskCache,
}

/// One resolved image layer of a tile.
#[derive(Clone)]
pub struct LayerHit {
    pub bytes: Arc<[u8]>,
    pub origin: TileOrigin,
}

/// One resolved baked-splat buffer and the hexon source it came from.
#[derive(Clone)]
pub struct SplatHit {
    pub splats: Arc<BakedSplatBuffer>,
    pub source: usize,
}

/// Every layer of one tile, resolved through the fallback chain.
#[derive(Clone, Default)]
pub struct LoadedTile {
    pub elevation: Option<LayerHit>,
    pub satellite: Option<LayerHit>,
    pub baked: Option<SplatHit>,
}

impl LoadedTile {
    /// Neither elevation nor satellite exists anywhere in the chain (a real failure).
    pub fn is_absent(&self) -> bool {
        self.elevation.is_none() && self.satellite.is_none()
    }

    /// Bytes this tile holds across its layers.
    pub fn byte_len(&self) -> u64 {
        let image = |hit: &Option<LayerHit>| hit.as_ref().map_or(0, |h| h.bytes.len() as u64);
        let splats = self.baked.as_ref().map_or(0, |hit| {
            ResidentTile::Splats(Arc::clone(&hit.splats)).byte_cost()
        });
        image(&self.elevation) + image(&self.satellite) + splats
    }
}

/// Disk-cache namespace per image layer.
fn cache_namespace(layer: TileLayer) -> &'static str {
    match layer {
        TileLayer::Satellite => "composite_sat",
        TileLayer::Elevation | TileLayer::BakedSplats => "composite",
    }
}

/// `Send + 'static` snapshot of a composite's lookup chain for an off-thread load.
#[derive(Clone)]
pub struct TileLoader {
    hexon_sources: Vec<Arc<HexonTileSource>>,
    cache: Option<Arc<DiskTileCache>>,
    mode: TileSourceMode,
}

impl TileLoader {
    /// Resolve every layer of `coord` with blocking reads (call off the main thread).
    pub fn load_blocking(&self, coord: TileCoord) -> LoadedTile {
        LoadedTile {
            elevation: self.load_image_blocking(TileLayer::Elevation, coord),
            satellite: self.load_image_blocking(TileLayer::Satellite, coord),
            baked: self.load_splats_blocking(coord),
        }
    }

    fn load_image_blocking(&self, layer: TileLayer, coord: TileCoord) -> Option<LayerHit> {
        if self.mode != TileSourceMode::Online {
            for (index, src) in self.hexon_sources.iter().enumerate() {
                if !src.covers(coord) {
                    continue;
                }
                match src.load_layer_blocking(layer, coord) {
                    Ok(Some(ResidentTile::Bytes(bytes))) => {
                        return Some(LayerHit {
                            bytes,
                            origin: TileOrigin::Hexon(index),
                        })
                    }
                    Ok(_) => {}
                    Err(err) if is_entitlement_refusal(&err) => tracing::debug!(
                        tile = %coord.cache_key(),
                        ?layer,
                        "hexon tile needs an entitlement; trying the next source"
                    ),
                    Err(err) => tracing::warn!(
                        tile = %coord.cache_key(),
                        ?layer,
                        error = %err,
                        "hexon tile read failed; trying the next source"
                    ),
                }
            }
        }
        // NFR-4: Offline never reads the disk cache (it may hold online-origin tiles).
        if self.mode == TileSourceMode::Offline {
            return None;
        }
        let bytes = self
            .cache
            .as_ref()?
            .get(cache_namespace(layer), &coord.cache_key())?;
        Some(LayerHit {
            bytes: Arc::from(bytes),
            origin: TileOrigin::DiskCache,
        })
    }

    fn load_splats_blocking(&self, coord: TileCoord) -> Option<SplatHit> {
        self.hexon_sources
            .iter()
            .enumerate()
            .filter(|(_, src)| src.covers(coord))
            .find_map(|(source, src)| match src.load_baked_splats_blocking(coord) {
                Ok(splats) => splats.map(|splats| SplatHit { splats, source }),
                Err(err) if is_entitlement_refusal(&err) => None,
                Err(err) => {
                    tracing::warn!(tile = %coord.cache_key(), error = %err, "baked splat read failed");
                    None
                }
            })
    }
}

/// Ordered fallback tile source.
///
/// Lookup order:
/// 1. Hexon sources (checked by `covers()` — bounds + zoom range)
/// 2. Streamed (not-installed, P2P) sources, resident bundles only (S7)
/// 3. Disk cache
/// 4. Online sources (results cached to disk)
pub struct CompositeTileSource {
    hexon_sources: Vec<Arc<HexonTileSource>>,
    cache: Option<Arc<DiskTileCache>>,
    /// Not-yet-installed tilesets streamed bundle-by-bundle over P2P (S7).
    streamed_sources: Vec<StreamedTileSource>,
    /// tileset_ids with no installed hexon and no resolved streamed source
    /// yet (awaiting `SyncEvent::TilesetMetaReceived`) — see
    /// `streamed_pending`, `src/AGENTS.md` §streamed-tilesets.
    pending_streamed_uris: HashSet<String>,
    #[cfg(feature = "fetch")]
    online_sources: Vec<Box<dyn super::source::TileSource>>,
    tile_source_mode: TileSourceMode,
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

impl Default for CompositeTileSource {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeTileSource {
    pub fn new() -> Self {
        Self {
            hexon_sources: Vec::new(),
            cache: None,
            streamed_sources: Vec::new(),
            pending_streamed_uris: HashSet::new(),
            #[cfg(feature = "fetch")]
            online_sources: Vec::new(),
            tile_source_mode: TileSourceMode::Hybrid,
        }
    }

    /// Set the tile resolution mode.
    pub fn set_tile_source_mode(&mut self, mode: TileSourceMode) {
        self.tile_source_mode = mode;
    }

    /// The mode tile lookups currently honour.
    pub fn tile_source_mode(&self) -> &TileSourceMode {
        &self.tile_source_mode
    }

    /// Check if demo mode is active (no hexons loaded + Hybrid mode + FE_DEMO_TILE_URL set).
    pub fn is_demo_mode(&self) -> bool {
        self.tile_source_mode == TileSourceMode::Hybrid
            && self.hexon_sources.is_empty()
            && std::env::var("FE_DEMO_TILE_URL").is_ok()
    }

    /// Worst-case resident ceiling per composite (p2p_asset_streaming S6,
    /// critic AC-21b): `TILE_BYTE_BUDGET` (256 MiB) is enforced PER
    /// `HexonTileSource`, not globally across every source a process holds
    /// (making it truly global would need a cross-source LRU, a much larger
    /// change than this slice's scope — tracked as a deferral, not silently
    /// dropped). The chosen mitigation instead bounds N: a petal's
    /// `tileset_hexon_uris` realistically cascades a regional tileset over at
    /// most a couple of broader fallback bases (`src/AGENTS.md`
    /// §petal-map/§lazy-tiles), so capping one composite at 4 sources caps
    /// its worst case at 4 x 256 MiB = 1 GiB — the same order of magnitude a
    /// single tileset's budget already accepted. A source past the cap is
    /// refused (logged), never silently dropped from the chain for an
    /// EXISTING coordinate — see `add_hexon_source`.
    pub const MAX_HEXON_SOURCES: usize = 4;

    /// Add a hexon tile source (checked first, by bounds); pass the registry's `Arc` to share it.
    ///
    /// Refuses (with a `warn!`) past [`Self::MAX_HEXON_SOURCES`] — see that
    /// constant's doc comment for the residency-budget rationale. The caller
    /// (`petal_binding`) still renders every source it added before the cap;
    /// only sources past the limit never reach the lookup chain.
    pub fn add_hexon_source(&mut self, source: impl Into<Arc<HexonTileSource>>) {
        if self.hexon_sources.len() >= Self::MAX_HEXON_SOURCES {
            tracing::warn!(
                limit = Self::MAX_HEXON_SOURCES,
                "composite already holds the max hexon sources; refusing another"
            );
            return;
        }
        self.hexon_sources.push(source.into());
    }

    /// The hexon sources, in lookup order.
    pub fn hexon_sources(&self) -> &[Arc<HexonTileSource>] {
        &self.hexon_sources
    }

    // -----------------------------------------------------------------------
    // Streamed (not-installed) tilesets — S7, src/AGENTS.md §streamed-tilesets
    // -----------------------------------------------------------------------

    /// Mark `tileset_id` as needing a streamed source once its index
    /// resolves (`petal_binding` found no installed hexon for this uri). A
    /// no-op if a streamed source for it already exists.
    pub fn mark_streamed_pending(&mut self, tileset_id: impl Into<String>) {
        let id = tileset_id.into();
        if !self.streamed_sources.iter().any(|s| s.tileset_id() == id) {
            self.pending_streamed_uris.insert(id);
        }
    }

    /// tileset_ids awaiting a `TilesetMetaReceived` to become a streamed source.
    pub fn pending_streamed_uris(&self) -> impl Iterator<Item = &str> {
        self.pending_streamed_uris.iter().map(String::as_str)
    }

    /// Install a resolved streamed source (its index arrived); clears the
    /// pending marker. Replaces any existing source for the same tileset_id
    /// (a re-resolution, e.g. after a petal re-entry).
    pub fn add_streamed_source(&mut self, source: StreamedTileSource) {
        self.pending_streamed_uris.remove(source.tileset_id());
        self.streamed_sources
            .retain(|s| s.tileset_id() != source.tileset_id());
        self.streamed_sources.push(source);
    }

    /// What fetched bundle `seq` of streamed `tileset_id` must prove before it
    /// is decoded (`None`: untracked tileset or unknown seq).
    pub fn streamed_bundle_check(
        &self,
        tileset_id: &str,
        seq: u32,
    ) -> Option<super::streamed::BundleCheck> {
        self.streamed_sources
            .iter()
            .find(|s| s.tileset_id() == tileset_id)?
            .bundle_check(seq)
    }

    /// A verified bundle arrived for `tileset_id`; feed it to the matching
    /// streamed source. A no-op (logged) if no such source is tracked — the
    /// petal may have switched away since the fetch was requested.
    pub fn insert_streamed_bundle(&mut self, tileset_id: &str, seq: u32, source: HexonTileSource) {
        match self
            .streamed_sources
            .iter()
            .find(|s| s.tileset_id() == tileset_id)
        {
            Some(streamed) => streamed.insert_bundle(seq, source),
            None => tracing::debug!(
                tileset_id,
                seq,
                "streamed bundle arrived for an untracked tileset; dropping"
            ),
        }
    }

    /// Drop streamed bundles that arrived but were never pinned within
    /// `max_idle` (the ring left before they landed); returns how many.
    pub fn evict_idle_streamed_bundles(
        &self,
        max_idle: std::time::Duration,
        now: std::time::Instant,
    ) -> usize {
        self.streamed_sources
            .iter()
            .map(|s| s.evict_idle(max_idle, now))
            .sum()
    }

    /// Whether `coord` is in a window a streamed source would eventually
    /// resolve but can't yet: an unresolved index (conservatively, only when
    /// no OTHER source already covers `coord`), or a resolved index whose
    /// covering bundle hasn't arrived. The ring loop must treat this as
    /// pending and skip the coordinate entirely — never record it in
    /// `FailedTiles` (NFR-1, `src/AGENTS.md` §streamed-tilesets).
    pub fn streamed_pending(&self, coord: TileCoord) -> bool {
        if !self.pending_streamed_uris.is_empty() {
            let covered_elsewhere = self.hexon_sources.iter().any(|s| s.covers(coord))
                || self.streamed_sources.iter().any(|s| s.covers(coord));
            if !covered_elsewhere {
                return true;
            }
        }
        self.streamed_sources.iter().any(|s| {
            s.covers(coord)
                && s.bundle_for(coord)
                    .is_some_and(|seq| !s.has_resident_bundle(seq))
        })
    }

    /// Pin the streamed bundle(s) covering `coord` for one live chunk
    /// (exit-ring eviction unit = the bundle, not the tile). A no-op when no
    /// streamed source covers `coord`.
    pub fn pin_streamed(&self, coord: TileCoord) {
        for src in self.streamed_sources.iter().filter(|s| s.covers(coord)) {
            src.pin(coord);
        }
    }

    /// Release one live chunk's hold on its streamed bundle(s); the last
    /// release drops the bundle's bytes from memory.
    pub fn release_streamed(&self, coord: TileCoord) {
        for src in self.streamed_sources.iter().filter(|s| s.covers(coord)) {
            src.release(coord);
        }
    }

    /// Record `coord`'s desired-but-missing bundle into `demand` (the tile
    /// half of the D-74 pointer-set): every streamed source covering `coord`
    /// without a resident bundle contributes its `(tileset_id, bundle_seq)`.
    /// Called from `fetch_and_spawn_terrain_chunks` at the same point
    /// `streamed_pending` gates the ring loop (`fe-terrain/src/AGENTS.md`
    /// §streamed-tilesets).
    #[cfg(feature = "render")]
    pub fn record_bundle_demand(
        &self,
        coord: TileCoord,
        demand: &mut super::streamed::TileBundleDemand,
    ) {
        for src in self.streamed_sources.iter().filter(|s| s.covers(coord)) {
            if let Some(seq) = src.bundle_for(coord) {
                if !src.has_resident_bundle(seq) {
                    demand.demand(src.tileset_id(), seq);
                }
            }
        }
    }

    /// Set the disk cache layer.
    pub fn set_cache(&mut self, cache: DiskTileCache) {
        self.cache = Some(Arc::new(cache));
    }

    /// Read-only access to each source's [`fe_format::manifest::TilesetMeta`],
    /// retained per-source for FR-4 reconciliation (in source-add order).
    pub fn source_metas(&self) -> Vec<&fe_format::manifest::TilesetMeta> {
        self.hexon_sources.iter().map(|s| &s.tileset_meta).collect()
    }

    /// Resident baked splat buffer for a tile from the first covering hexon (FR-4); no I/O.
    pub fn get_baked_splats_sync(&self, coord: TileCoord) -> Option<Arc<BakedSplatBuffer>> {
        self.hexon_sources
            .iter()
            .filter(|src| src.covers(coord))
            .find_map(|src| src.get_baked_splats(coord))
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
                SourceLodPick {
                    source_index: index,
                    zoom,
                }
            })
            .collect()
    }

    /// Add an online tile source (checked last; results written to cache).
    #[cfg(feature = "fetch")]
    pub fn add_online_source(&mut self, source: Box<dyn super::source::TileSource>) {
        self.online_sources.push(source);
    }

    /// Resident elevation bytes from the hexon chain; never touches the disk (NFR-2).
    ///
    /// Disk-cache tiles are reachable only through [`load_blocking`](Self::load_blocking).
    pub fn get_tile_sync(&self, coord: TileCoord) -> Option<Vec<u8>> {
        self.resident_image(TileLayer::Elevation, coord)
            .map(|bytes| bytes.to_vec())
    }

    /// Resident satellite bytes from the hexon chain; never touches the disk (NFR-2).
    pub fn get_satellite_tile_sync(&self, coord: TileCoord) -> Option<Vec<u8>> {
        self.resident_image(TileLayer::Satellite, coord)
            .map(|bytes| bytes.to_vec())
    }

    fn resident_image(&self, layer: TileLayer, coord: TileCoord) -> Option<Arc<[u8]>> {
        if self.tile_source_mode == TileSourceMode::Online {
            return None;
        }
        self.hexon_sources
            .iter()
            .filter(|src| src.covers(coord))
            .find_map(|src| {
                src.get_layer(layer, coord)
                    .and_then(ResidentTile::into_bytes)
            })
            .or_else(|| {
                self.streamed_sources
                    .iter()
                    .filter(|src| src.covers(coord))
                    .find_map(|src| {
                        src.get_layer(layer, coord)
                            .and_then(ResidentTile::into_bytes)
                    })
            })
    }

    /// I/O-free full resolution of `coord`; `None` when any layer still needs a read.
    pub fn resident_tile(&self, coord: TileCoord) -> Option<LoadedTile> {
        Some(LoadedTile {
            elevation: self.resident_layer(TileLayer::Elevation, coord)?,
            satellite: self.resident_layer(TileLayer::Satellite, coord)?,
            baked: self.resident_splats(coord)?,
        })
    }

    /// `Some(resolved)` when the image layer resolves without I/O, `None` when a read is needed.
    fn resident_layer(&self, layer: TileLayer, coord: TileCoord) -> Option<Option<LayerHit>> {
        if self.tile_source_mode != TileSourceMode::Online {
            for (index, src) in self.hexon_sources.iter().enumerate() {
                if !src.covers(coord) {
                    continue;
                }
                match src.probe(layer, coord) {
                    Probe::Hit(ResidentTile::Bytes(bytes)) => {
                        return Some(Some(LayerHit {
                            bytes,
                            origin: TileOrigin::Hexon(index),
                        }))
                    }
                    Probe::Hit(ResidentTile::Splats(_)) | Probe::Absent => {}
                    Probe::Miss => return None,
                }
            }
            // Streamed sources (S7): checked after installed hexon sources,
            // before the disk cache, in EVERY mode including Offline — the
            // bytes are hexon-origin (just not-yet-arrived over P2P instead
            // of not-yet-read from disk), so NFR-4 (Offline never reads the
            // disk cache) still holds. A covering-but-not-resident bundle
            // here falls through to the terminal `match` below; the ring
            // loop never reaches that case in practice because
            // `streamed_pending` gates it first (src/AGENTS.md
            // §streamed-tilesets).
            for src in self.streamed_sources.iter().filter(|s| s.covers(coord)) {
                if let Some(ResidentTile::Bytes(bytes)) = src.get_layer(layer, coord) {
                    return Some(Some(LayerHit {
                        bytes,
                        origin: TileOrigin::Streamed,
                    }));
                }
            }
        }
        match (&self.tile_source_mode, &self.cache) {
            (TileSourceMode::Offline, _) | (_, None) => Some(None),
            // Only a disk read can say whether the cache holds it.
            (_, Some(_)) => None,
        }
    }

    /// `Some(resolved)` when the baked-splat layer resolves without I/O.
    fn resident_splats(&self, coord: TileCoord) -> Option<Option<SplatHit>> {
        for (source, src) in self.hexon_sources.iter().enumerate() {
            if !src.covers(coord) {
                continue;
            }
            match src.probe(TileLayer::BakedSplats, coord) {
                Probe::Hit(ResidentTile::Splats(splats)) => {
                    return Some(Some(SplatHit { splats, source }))
                }
                Probe::Hit(ResidentTile::Bytes(_)) | Probe::Absent => {}
                Probe::Miss => return None,
            }
        }
        Some(None)
    }

    /// Snapshot of the lookup chain for an off-thread [`TileLoader::load_blocking`].
    pub fn loader(&self) -> TileLoader {
        TileLoader {
            hexon_sources: self.hexon_sources.clone(),
            cache: self.cache.clone(),
            mode: self.tile_source_mode.clone(),
        }
    }

    /// Resolve every layer of `coord` with blocking reads (off the main thread only).
    pub fn load_blocking(&self, coord: TileCoord) -> LoadedTile {
        self.loader().load_blocking(coord)
    }

    /// Pin a spawned chunk's hexon-origin layers so the exit ring, not the LRU, frees them.
    ///
    /// A streamed-origin hit (`TileOrigin::Streamed`) is pinned too, via the
    /// unconditional `pin_streamed` call below — streamed bundles are
    /// evicted as a whole bundle, not per `(layer, coord)` key, so there is
    /// nothing per-layer to do in the loop above for them (S7).
    pub fn pin_tile(&self, coord: TileCoord, tile: &LoadedTile) {
        for (layer, hit) in [
            (TileLayer::Elevation, &tile.elevation),
            (TileLayer::Satellite, &tile.satellite),
        ] {
            if let Some(LayerHit {
                bytes,
                origin: TileOrigin::Hexon(index),
            }) = hit
            {
                if let Some(src) = self.hexon_sources.get(*index) {
                    src.pin(layer, coord, &ResidentTile::Bytes(Arc::clone(bytes)));
                }
            }
        }
        if let Some(hit) = &tile.baked {
            if let Some(src) = self.hexon_sources.get(hit.source) {
                src.pin(
                    TileLayer::BakedSplats,
                    coord,
                    &ResidentTile::Splats(Arc::clone(&hit.splats)),
                );
            }
        }
        self.pin_streamed(coord);
    }

    /// Release a despawned chunk's tile bytes from every covering source (exit ring).
    pub fn release_tile(&self, coord: TileCoord) {
        for src in self.hexon_sources.iter().filter(|src| src.covers(coord)) {
            src.release_tile(coord);
        }
        self.release_streamed(coord);
    }

    /// Release every pin this composite placed (it is being replaced).
    pub fn release_all_pins(&self) {
        for src in &self.hexon_sources {
            src.release_all_pins();
        }
    }

    /// Resident bundles across the streamed (not-installed) sources.
    pub fn streamed_resident_bundles(&self) -> usize {
        self.streamed_sources
            .iter()
            .map(StreamedTileSource::resident_bundle_count)
            .sum()
    }

    /// Tile bytes held by the streamed sources' resident bundles.
    pub fn streamed_resident_bytes(&self) -> u64 {
        self.streamed_sources
            .iter()
            .map(StreamedTileSource::resident_bytes)
            .sum()
    }

    /// Tile bytes resident in RAM across the distinct hexon sources.
    pub fn resident_bytes(&self) -> u64 {
        self.hexon_sources
            .iter()
            .enumerate()
            .filter(|(i, src)| {
                !self.hexon_sources[..*i]
                    .iter()
                    .any(|earlier| Arc::ptr_eq(earlier, src))
            })
            .map(|(_, src)| src.resident_bytes())
            .sum()
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
        let local = self.load_blocking(coord).elevation;
        if let Some(hit) = local {
            return Ok(hit.bytes.to_vec());
        }
        if self.tile_source_mode == TileSourceMode::Offline {
            anyhow::bail!(
                "tile {} not found in hexon sources (offline mode)",
                coord.cache_key()
            );
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
            "tile {} not found in any source ({:?} mode)",
            coord.cache_key(),
            self.tile_source_mode
        )
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

    fn test_manifest(hexon_id: &str) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: hexon_id.into(),
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
            integrity: None,
            signature: None,
        }
    }

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
            crs: None, // deliberately legacy — differs from Default's Some("EPSG:4326")
            ..Default::default()
        };
        let manifest = test_manifest("test");
        let bytes =
            HexonArchive::export_tileset(manifest, &meta, &tiles, &satellite, None).unwrap();
        HexonTileSource::from_archive(&bytes).unwrap()
    }

    /// S6 (critic AC-21b residency-budget mitigation): a composite refuses a
    /// hexon source past `MAX_HEXON_SOURCES`, bounding the worst-case
    /// resident ceiling; sources added before the cap are unaffected.
    #[test]
    fn add_hexon_source_refuses_past_the_cap() {
        let mut composite = CompositeTileSource::new();
        for i in 0..CompositeTileSource::MAX_HEXON_SOURCES + 2 {
            let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
            let source = make_hexon_source(vec![(coord.cache_key(), vec![i as u8; 4])], vec![]);
            composite.add_hexon_source(source);
        }
        assert_eq!(
            composite.hexon_sources().len(),
            CompositeTileSource::MAX_HEXON_SOURCES
        );
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

        // A licensed, gateless source: the free tile loads, the non-free one is a silent miss.
        let free_coord = coord;
        let sealed_coord = TileCoord::new(coord.x + 1, coord.y, coord.zoom);
        let license = fe_format::License {
            license_type: fe_format::LicenseType::Paid,
            payment_verification_url: Some("https://publisher.example/pay".into()),
            free_entries: vec![format!("terrain/tiles/{}.png", free_coord.cache_key())],
            ..fe_format::License::default()
        };
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 2,
            region_name: "Test".into(),
            crs: None,
            ..Default::default()
        };
        let tiles = vec![
            (free_coord.cache_key(), vec![1u8; 16]),
            (sealed_coord.cache_key(), vec![2u8; 16]),
        ];
        let archive = HexonArchive::export_tileset(
            test_manifest("licensed-test"),
            &meta,
            &tiles,
            &[],
            Some(license),
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!(
            "fe_composite_entitlement_{}_{}",
            std::process::id(),
            line!()
        ));
        let store = super::super::store::HexonStore::with_dir(dir.clone()).unwrap();
        store.install_tileset(&archive).unwrap();
        let licensed = store.load_tileset("licensed-test").unwrap();
        let refusal = licensed
            .load_layer_blocking(TileLayer::Elevation, sealed_coord)
            .expect_err("a non-free tile of a gateless licensed source is refused");
        assert!(is_entitlement_refusal(&refusal), "{refusal:#}");
        assert!(!is_entitlement_refusal(&anyhow::anyhow!("disk full")));
        let mut licensed_composite = CompositeTileSource::new();
        licensed_composite.add_hexon_source(licensed);
        assert_eq!(
            licensed_composite.get_tile_sync(free_coord).unwrap(),
            vec![1u8; 16]
        );
        assert!(licensed_composite.get_tile_sync(sealed_coord).is_none());
        let _ = std::fs::remove_dir_all(&dir);
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

        assert_eq!(
            composite.get_satellite_tile_sync(coord).unwrap(),
            vec![7u8; 8]
        );
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

        assert_eq!(
            composite.get_satellite_tile_sync(coord).unwrap(),
            vec![9u8; 4]
        );
    }

    #[test]
    fn only_the_off_thread_loader_reads_the_disk_cache_and_never_in_offline_mode() {
        let dir = std::env::temp_dir().join(format!(
            "fe_composite_nfr4_{}_{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let coord = TileCoord::new(99, 99, 5);
        let cache = DiskTileCache::new(&dir, 1024 * 1024);
        cache.put("composite", &coord.cache_key(), b"disk").unwrap();
        let mut composite = CompositeTileSource::new();
        composite.set_cache(cache);

        // (mode, what the loader resolves, whether resolution can skip I/O)
        let cases: [(TileSourceMode, Option<&[u8]>, bool); 3] = [
            (TileSourceMode::Offline, None, true),
            (TileSourceMode::Hybrid, Some(b"disk"), false),
            (TileSourceMode::Online, Some(b"disk"), false),
        ];
        for (mode, want, io_free) in cases {
            composite.set_tile_source_mode(mode.clone());
            let loaded = composite.load_blocking(coord).elevation;
            assert_eq!(loaded.as_ref().map(|hit| &*hit.bytes), want, "{mode:?}");
            if let Some(hit) = loaded {
                assert_eq!(hit.origin, TileOrigin::DiskCache);
            }
            assert!(
                composite.get_tile_sync(coord).is_none(),
                "{mode:?}: resident lookup never reads disk"
            );
            assert_eq!(
                composite.resident_tile(coord).is_some(),
                io_free,
                "{mode:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
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
