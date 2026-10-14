//! Streamed (not-installed) tile source: bundle-by-bundle P2P tiles; see
//! `src/AGENTS.md` §streamed-tilesets.

use std::collections::HashMap;
use std::sync::Mutex;

use fe_format::manifest::TilesetBundleIndex;

use super::hexon_source::{HexonTileSource, ResidentTile, TileLayer};
use super::source::TileCoord;

/// Not-yet-installed tileset served bundle-by-bundle over P2P (D-74 tile
/// pointer-set). A tile resolves to a bundle sequence via
/// `TilesetBundleIndex::bundle_for_tile` (critic C4: the one tile→bundle
/// lookup — no second table here); only a RESIDENT bundle's bytes are ever
/// served. A covering-but-not-resident coordinate is pending, never a
/// failure — see `CompositeTileSource::streamed_pending`.
///
/// Each resident bundle is a fully-decoded, in-memory `HexonTileSource`
/// (`from_archive`, `Backing::Memory`), so once present it serves with zero
/// further I/O. `state` is a `Mutex` only so pin/release/insert stay `&self`
/// — the same interior-mutability shape `HexonTileSource::cache` already
/// uses — so `CompositeTileSource::pin_tile`/`release_tile` (both `&self`)
/// need no signature change to reach it.
pub struct StreamedTileSource {
    tileset_id: String,
    index: TilesetBundleIndex,
    state: Mutex<StreamedState>,
}

#[derive(Default)]
struct StreamedState {
    resident: HashMap<u32, HexonTileSource>,
    /// Live spawned-chunk refcount per bundle — the exit-ring eviction unit
    /// is the BUNDLE (many tiles), unlike the per-tile `ByteLru` an installed
    /// `HexonTileSource` uses.
    pins: HashMap<u32, u32>,
}

impl StreamedTileSource {
    /// A resolved index (from `SyncEvent::TilesetMetaReceived`), no bundles yet.
    pub fn new(tileset_id: String, index: TilesetBundleIndex) -> Self {
        Self {
            tileset_id,
            index,
            state: Mutex::new(StreamedState::default()),
        }
    }

    /// Parse a `SyncEvent::TilesetMetaReceived.meta_json` (the opaque
    /// `TilesetBundleIndex` JSON fe-sync never parses itself) and build a
    /// source from it. The one entry point a host binary needs for a
    /// resolved streamed index, so it never needs a production `fe_format`
    /// dependency just to deserialize it — the same reasoning as
    /// `assemble_bundles_from_index_json` (`tiles/bundle.rs`, S4).
    ///
    /// S8b: the index must name `tileset_id` — a peer answering for X with Y's
    /// index is refused, so a streamed source never serves another tileset.
    pub fn from_meta_json(tileset_id: String, meta_json: &str) -> anyhow::Result<Self> {
        let index: TilesetBundleIndex = serde_json::from_str(meta_json)?;
        if index.tileset_id != tileset_id {
            anyhow::bail!(
                "streamed index declares tileset {:?}, but {:?} was requested",
                index.tileset_id,
                tileset_id
            );
        }
        Ok(Self::new(tileset_id, index))
    }

    pub fn tileset_id(&self) -> &str {
        &self.tileset_id
    }

    /// The bundle index this source serves (evidence/diagnostics only).
    pub fn index(&self) -> &TilesetBundleIndex {
        &self.index
    }

    fn state(&self) -> std::sync::MutexGuard<'_, StreamedState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Geographic + zoom coverage — the same extent rule installed sources
    /// use (`HexonTileSource::covers`), over the index's whole-tileset meta
    /// (deliberately duplicated rather than reaching into `hexon_source.rs`'s
    /// private helpers; see `src/AGENTS.md` §streamed-tilesets).
    pub fn covers(&self, coord: TileCoord) -> bool {
        tile_in_extent(
            self.index.tileset_meta.bounds,
            (
                self.index.tileset_meta.min_zoom,
                self.index.tileset_meta.max_zoom,
            ),
            coord,
        )
    }

    /// The bundle sequence `coord` belongs to, per the index.
    pub fn bundle_for(&self, coord: TileCoord) -> Option<u32> {
        self.index.bundle_for_tile(coord.zoom, coord.x, coord.y)
    }

    /// Whether bundle `seq` is already resident.
    pub fn has_resident_bundle(&self, seq: u32) -> bool {
        self.state().resident.contains_key(&seq)
    }

    /// Feed a verified, fetched bundle's decoded archive in (its per-chunk
    /// BLAKE3 was already checked by `TilesetTransfer` before `ChunkReceived`
    /// — see fe-sync `AGENTS.md` §tileset-transfer). A duplicate insert (a
    /// re-fetch after eviction) just replaces it.
    pub fn insert_bundle(&self, seq: u32, source: HexonTileSource) {
        self.state().resident.insert(seq, source);
    }

    /// Resident bytes for `layer` at `coord` (no I/O — the bundle is either
    /// already in memory or this returns `None`).
    pub fn get_layer(&self, layer: TileLayer, coord: TileCoord) -> Option<ResidentTile> {
        let seq = self.bundle_for(coord)?;
        self.state().resident.get(&seq)?.get_layer(layer, coord)
    }

    /// Pin the bundle covering `coord` for one live chunk.
    pub fn pin(&self, coord: TileCoord) {
        if let Some(seq) = self.bundle_for(coord) {
            *self.state().pins.entry(seq).or_insert(0) += 1;
        }
    }

    /// Release one live chunk's hold; the bundle's bytes drop from memory
    /// once its last pin goes (exit-ring eviction). The disk copy stays in
    /// the download dir as a cache (fe-sync `AGENTS.md` §tileset-transfer) —
    /// re-entry re-demands the bundle but does not re-fetch it over the
    /// network if `TilesetTransfer` still holds it verified on disk.
    pub fn release(&self, coord: TileCoord) {
        let Some(seq) = self.bundle_for(coord) else {
            return;
        };
        let mut state = self.state();
        if let Some(count) = state.pins.get_mut(&seq) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.pins.remove(&seq);
                state.resident.remove(&seq);
            }
        }
    }

    /// Bundles currently resident (evidence/tests, fe-ui residency readout).
    pub fn resident_bundle_count(&self) -> usize {
        self.state().resident.len()
    }

    /// Decoded tile bytes held by the resident bundles (fe-ui residency readout).
    pub fn resident_bytes(&self) -> u64 {
        self.state()
            .resident
            .values()
            .map(HexonTileSource::resident_bytes)
            .sum()
    }
}

/// Geographic+zoom extent test shared in spirit with
/// `HexonTileSource::covers` (duplicated rather than exposed there — small
/// and self-contained; see `src/AGENTS.md` §streamed-tilesets).
fn tile_in_extent(bounds: [f64; 4], zoom_range: (u8, u8), coord: TileCoord) -> bool {
    let (min_z, max_z) = zoom_range;
    if coord.zoom < min_z || coord.zoom > max_z {
        return false;
    }
    let (nw_lat, nw_lon) = coord.to_lat_lon();
    let (se_lat, se_lon) = TileCoord::new(coord.x + 1, coord.y + 1, coord.zoom).to_lat_lon();
    let tile_min_lat = se_lat.min(nw_lat);
    let tile_max_lat = se_lat.max(nw_lat);
    let tile_min_lon = nw_lon.min(se_lon);
    let tile_max_lon = nw_lon.max(se_lon);
    let [min_lat, min_lon, max_lat, max_lon] = bounds;
    tile_min_lat <= max_lat
        && tile_max_lat >= min_lat
        && tile_min_lon <= max_lon
        && tile_max_lon >= min_lon
}

#[cfg(feature = "render")]
mod render_demand {
    use std::collections::HashSet;

    use bevy::prelude::Resource;

    /// Per-frame tile-bundle pointer-set (D-74 tile pointer-set, the tile
    /// half — `ResidencyLedger` in fe-ui's S8 is the node/stamp half): the
    /// distinct `(tileset_id, bundle_seq)` pairs the current ring wants from
    /// streamed sources whose bundle isn't resident yet. Rebuilt every frame
    /// by `fetch_and_spawn_terrain_chunks` (`clear()` then re-demanded), so
    /// it always reflects what the ring wants NOW — bounded over-fetch at a
    /// ring edge (D-73 evidence §3: at most one bundle), never stale demand
    /// from a camera position the ring has since left.
    #[derive(Resource, Default)]
    pub struct TileBundleDemand {
        wanted: HashSet<(String, u32)>,
    }

    impl TileBundleDemand {
        /// Drop last frame's demand — called once at the top of the ring pass.
        pub fn clear(&mut self) {
            self.wanted.clear();
        }

        /// Record that `tileset_id`'s bundle `seq` is wanted this frame.
        pub fn demand(&mut self, tileset_id: &str, seq: u32) {
            self.wanted.insert((tileset_id.to_string(), seq));
        }

        /// The distinct `(tileset_id, bundle_seq)` pairs currently wanted.
        pub fn wanted(&self) -> impl Iterator<Item = &(String, u32)> {
            self.wanted.iter()
        }

        /// Number of distinct bundles currently wanted (evidence/readout).
        pub fn len(&self) -> usize {
            self.wanted.len()
        }

        /// True when nothing is currently demanded.
        pub fn is_empty(&self) -> bool {
            self.wanted.is_empty()
        }
    }
}

#[cfg(feature = "render")]
pub use render_demand::TileBundleDemand;

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{BundleEntry, ElevationEncoding, TilesetMeta};

    /// A 2-bundle index over zoom 10: bundle 0 covers x in [0,1], bundle 1
    /// covers x in [2,3] (y fixed at 0) — enough to exercise bundle_for /
    /// covers / pin / release without a real archive.
    fn two_bundle_index() -> TilesetBundleIndex {
        TilesetBundleIndex {
            schema: 1,
            tileset_id: "streamed-test".into(),
            tileset_meta: TilesetMeta {
                bounds: [-85.0, -180.0, 85.0, 180.0],
                min_zoom: 10,
                max_zoom: 10,
                tile_size: 256,
                elevation_encoding: ElevationEncoding::TerrainRgb,
                tile_count: 4,
                region_name: "Streamed Test".into(),
                ..Default::default()
            },
            bundles: vec![
                BundleEntry {
                    seq: 0,
                    blake3_hex: "a".into(),
                    byte_len: 10,
                    tile_count: 2,
                    zoom: 10,
                    x_range: [0, 1],
                    y_range: [0, 0],
                    hilbert_range: [0, 1],
                },
                BundleEntry {
                    seq: 1,
                    blake3_hex: "b".into(),
                    byte_len: 10,
                    tile_count: 2,
                    zoom: 10,
                    x_range: [2, 3],
                    y_range: [0, 0],
                    hilbert_range: [2, 3],
                },
            ],
        }
    }

    fn manifest(id: &str) -> fe_format::manifest::HexonManifest {
        fe_format::manifest::HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: fe_format::manifest::HexonType::TerrainTileset,
            publisher_did: "did:key:ztest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Streamed bundle".into(),
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

    /// A bundle archive carrying one elevation tile at `(10, x, 0)`.
    fn bundle_archive(x: u32, byte: u8) -> Vec<u8> {
        let meta = TilesetMeta {
            bounds: [-85.0, -180.0, 85.0, 180.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 1,
            region_name: "bundle".into(),
            ..Default::default()
        };
        let tiles = vec![(format!("10/{x}/0"), vec![byte; 8])];
        fe_format::HexonArchive::export_tileset(manifest("bundle"), &meta, &tiles, &[], None)
            .expect("export_tileset")
    }

    #[test]
    fn bundle_for_and_covers_resolve_through_the_index() {
        let src = StreamedTileSource::new("ts".into(), two_bundle_index());
        let in_bundle_0 = TileCoord::new(0, 0, 10);
        let in_bundle_1 = TileCoord::new(2, 0, 10);
        let outside = TileCoord::new(0, 0, 11);

        assert!(src.covers(in_bundle_0));
        assert_eq!(src.bundle_for(in_bundle_0), Some(0));
        assert_eq!(src.bundle_for(in_bundle_1), Some(1));
        assert!(!src.covers(outside));
        assert!(!src.has_resident_bundle(0));
    }

    /// (critic-style) The demand set equals exactly the bundles the covered
    /// ring coordinates map to — a strict subset of all bundles in the
    /// index, never the whole tileset (S7 flow test
    /// `a_petal_on_an_uninstalled_tileset_demands_only_ring_bundles`).
    /// `TileBundleDemand`/`record_bundle_demand` are render-gated (Bevy
    /// `Resource`), so this test is too.
    #[test]
    #[cfg(feature = "render")]
    fn a_petal_on_an_uninstalled_tileset_demands_only_ring_bundles() {
        let mut composite = super::super::composite::CompositeTileSource::new();
        composite.add_streamed_source(StreamedTileSource::new("ts".into(), two_bundle_index()));

        let mut demand = TileBundleDemand::default();
        // The "ring" only ever visits bundle 0's tiles this frame.
        for coord in [TileCoord::new(0, 0, 10), TileCoord::new(1, 0, 10)] {
            assert!(composite.streamed_pending(coord), "not yet resident");
            composite.record_bundle_demand(coord, &mut demand);
        }

        let wanted: Vec<(String, u32)> = demand.wanted().cloned().collect();
        assert_eq!(wanted, vec![("ts".to_string(), 0)], "{wanted:?}");
    }

    /// Moving the camera off bundle 0's tiles and onto bundle 1's both
    /// releases bundle 0 (exit-ring eviction) and demands the new bundle
    /// (S7 flow test `moving_the_camera_demands_new_bundles_and_releases_old_ones`).
    #[test]
    #[cfg(feature = "render")]
    fn moving_the_camera_demands_new_bundles_and_releases_old_ones() {
        let mut composite = super::super::composite::CompositeTileSource::new();
        composite.add_streamed_source(StreamedTileSource::new("ts".into(), two_bundle_index()));

        let coord0 = TileCoord::new(0, 0, 10);
        let coord1 = TileCoord::new(2, 0, 10);

        // Bundle 0 arrives and gets pinned by one spawned chunk.
        composite.insert_streamed_bundle(
            "ts",
            0,
            HexonTileSource::from_archive(&bundle_archive(0, 1)).unwrap(),
        );
        assert!(!composite.streamed_pending(coord0), "resident now");
        assert_eq!(composite.streamed_resident_bundles(), 1);
        let resident_bytes = composite.streamed_resident_bytes();
        assert!(resident_bytes > 0, "S8 readout sees the bundle's bytes");
        composite.pin_streamed(coord0);

        // The camera moves to bundle 1's tiles: bundle 1 is still pending
        // and gets demanded; bundle 0's chunk despawns and releases it.
        let mut demand = TileBundleDemand::default();
        assert!(composite.streamed_pending(coord1));
        composite.record_bundle_demand(coord1, &mut demand);
        let wanted: Vec<(String, u32)> = demand.wanted().cloned().collect();
        assert_eq!(wanted, vec![("ts".to_string(), 1)], "{wanted:?}");

        composite.release_streamed(coord0);
        assert!(
            composite.streamed_pending(coord0),
            "bundle 0 left memory with the chunk that pinned it"
        );
        assert_eq!(composite.streamed_resident_bundles(), 0);
        assert_eq!(
            composite.streamed_resident_bytes(),
            0,
            "readout drops to zero"
        );
    }

    /// S8b: a meta reply whose index names another tileset never becomes a
    /// streamed source for the requested one; a matching index does.
    #[test]
    fn a_streamed_index_must_name_the_requested_tileset() {
        let json = serde_json::to_string(&two_bundle_index()).unwrap();
        let err = StreamedTileSource::from_meta_json("other-tileset".into(), &json)
            .err()
            .expect("a mismatched index is refused");
        assert!(err.to_string().contains("requested"), "{err:#}");
        let ok = StreamedTileSource::from_meta_json("streamed-test".into(), &json)
            .expect("a matching index parses");
        assert_eq!(ok.tileset_id(), "streamed-test");
    }

    /// S8b: a streamed bundle that inflates past the decode budget is refused
    /// (zip-bomb bound) while an honest bundle decodes and serves its tile.
    #[test]
    fn a_streamed_bundle_over_the_decode_budget_is_refused() {
        let honest = HexonTileSource::from_streamed_bundle(&bundle_archive(0, 3)).unwrap();
        assert!(honest.resident_bytes() > 0);

        let meta = TilesetMeta {
            bounds: [-85.0, -180.0, 85.0, 180.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 2,
            region_name: "bomb".into(),
            ..Default::default()
        };
        let half = (super::super::hexon_source::STREAMED_BUNDLE_DECODE_BUDGET / 2 + 1) as usize;
        let tiles = vec![
            ("10/0/0".to_string(), vec![0u8; half]),
            ("10/1/0".to_string(), vec![0u8; half]),
        ];
        let bomb =
            fe_format::HexonArchive::export_tileset(manifest("bomb"), &meta, &tiles, &[], None)
                .expect("export_tileset");
        assert!(
            HexonTileSource::from_streamed_bundle(&bomb).is_err(),
            "over the streamed decode budget"
        );
    }

    #[test]
    fn pin_and_release_evict_a_bundle_only_once_its_last_pin_drops() {
        let src = StreamedTileSource::new("ts".into(), two_bundle_index());
        let coord = TileCoord::new(0, 0, 10);
        src.insert_bundle(
            0,
            HexonTileSource::from_archive(&bundle_archive(0, 7)).unwrap(),
        );

        src.pin(coord);
        src.pin(coord); // two chunks reference the same bundle
        src.release(coord);
        assert!(src.has_resident_bundle(0), "one pin still holds it");
        src.release(coord);
        assert!(!src.has_resident_bundle(0), "last pin released it");
        assert_eq!(src.resident_bundle_count(), 0);
    }
}
