//! Tileset builder — downloads tiles from an online source and packages
//! them into `.hexon` archives for offline use.
//!
//! Requires the `fetch` feature.

#[cfg(feature = "fetch")]
mod inner {
    use std::sync::Arc;
    use std::time::Instant;

    use anyhow::{Context, Result};
    use tokio::sync::Mutex;

    use fe_format::license::License;
    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::HexonArchive;

    use crate::splat::bake::bake_splat_coverage;
    use crate::splat::format::{
        append_baked_splats_to_archive, encode_baked_splats, tile_world_size_m, BakedSplatBuffer,
    };
    use crate::splat::synth::synthesize_splats;
    use crate::tiles::elevation::{
        decode_png_pixels, ElevationDecoder, TerrainRgbDecoder, TerrariumDecoder,
    };
    use crate::tiles::source::{TileCoord, TileSource};

    use super::parse_cache_key;

    /// Decimation stride used when synthesizing the native splat field the
    /// bake step densifies (FR-1/FR-3). Build-time only — no per-frame budget
    /// constraint, so a denser starting field than the runtime default (see
    /// `render::SplatConfig::stride`) is fine and gives the fill less work.
    const BAKE_SPLAT_STRIDE: usize = 1;

    /// Shared, lock-guarded accumulator of `(cache_key, tile_bytes)` pairs
    /// collected across the sequential download loop in `download_tiles`
    /// (C10 feature-matrix gate: factored out of the call site's type
    /// annotation, which clippy's `type_complexity` flags at `fetch,render`).
    type DownloadedTiles = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// Result of a tileset build operation.
    #[derive(Debug, Clone)]
    pub struct TilesetBuildResult {
        pub tiles_downloaded: u32,
        pub tiles_failed: u32,
        pub total_bytes: u64,
        pub elapsed: std::time::Duration,
    }

    /// Progress callback signature: `(completed, total, last_key)`.
    pub type ProgressCallback = Box<dyn Fn(u32, u32, &str) + Send + Sync>;

    /// Downloads tiles from an online source and packages them into hexon archives.
    pub struct TilesetBuilder {
        region_name: String,
        bounds: [f64; 4],
        min_zoom: u8,
        max_zoom: u8,
        elevation_encoding: ElevationEncoding,
        source: Option<Box<dyn TileSource>>,
        progress: Option<ProgressCallback>,
    }

    impl TilesetBuilder {
        pub fn new(
            region_name: impl Into<String>,
            bounds: [f64; 4],
            min_zoom: u8,
            max_zoom: u8,
            elevation_encoding: ElevationEncoding,
        ) -> Self {
            Self {
                region_name: region_name.into(),
                bounds,
                min_zoom,
                max_zoom,
                elevation_encoding,
                source: None,
                progress: None,
            }
        }

        /// Set the online tile source to download from.
        pub fn set_source(&mut self, source: Box<dyn TileSource>) {
            self.source = Some(source);
        }

        /// Set an optional progress callback.
        pub fn set_progress(&mut self, callback: ProgressCallback) {
            self.progress = Some(callback);
        }

        /// Enumerate all tile coordinates within `bounds` × `zoom` range.
        fn enumerate_coords(&self) -> Vec<TileCoord> {
            let [min_lat, min_lon, max_lat, max_lon] = self.bounds;
            let mut coords = Vec::new();

            for zoom in self.min_zoom..=self.max_zoom {
                let nw = TileCoord::from_lat_lon(max_lat, min_lon, zoom);
                let se = TileCoord::from_lat_lon(min_lat, max_lon, zoom);

                for x in nw.x..=se.x {
                    for y in nw.y..=se.y {
                        coords.push(TileCoord::new(x, y, zoom));
                    }
                }
            }
            coords
        }

        /// Download all tiles in the bounds × zoom range.
        pub async fn download_tiles(&self) -> Result<(TilesetBuildResult, Vec<(String, Vec<u8>)>)> {
            let source = self
                .source
                .as_ref()
                .context("no tile source set — call set_source() first")?;

            let coords = self.enumerate_coords();
            let total = coords.len() as u32;
            let start = Instant::now();

            let downloaded: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
            let failed: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
            let tiles: DownloadedTiles = Arc::new(Mutex::new(Vec::new()));

            // Download tiles sequentially to respect source's internal semaphore
            for coord in &coords {
                let key = coord.cache_key();
                match source.fetch_tile(*coord).await {
                    Ok(data) => {
                        tiles.lock().await.push((key.clone(), data));
                        let mut d = downloaded.lock().await;
                        *d += 1;
                        if let Some(ref cb) = self.progress {
                            cb(*d, total, &key);
                        }
                    }
                    Err(_) => {
                        let mut f = failed.lock().await;
                        *f += 1;
                    }
                }
            }

            let tiles = Arc::try_unwrap(tiles).unwrap().into_inner();
            let total_bytes: u64 = tiles.iter().map(|(_, d)| d.len() as u64).sum();
            let tiles_downloaded = *downloaded.lock().await;
            let tiles_failed = *failed.lock().await;

            Ok((
                TilesetBuildResult {
                    tiles_downloaded,
                    tiles_failed,
                    total_bytes,
                    elapsed: start.elapsed(),
                },
                tiles,
            ))
        }

        /// Bake coverage-filled splat buffers for every downloaded elevation
        /// tile (FR-3). Runs the FR-1 hole fill once per tile at build time;
        /// tiles that fail to decode or synthesize no splats are simply
        /// skipped (they fall back to live synthesis at runtime, FR-5).
        /// Build-time budget is unconstrained (offline) — tiles are baked
        /// sequentially, no per-frame limit applies here.
        fn bake_splat_tiles(&self, tiles: &[(String, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
            let decoder: Option<Box<dyn ElevationDecoder>> = match self.elevation_encoding {
                ElevationEncoding::TerrainRgb => Some(Box::new(TerrainRgbDecoder)),
                ElevationEncoding::Terrarium => Some(Box::new(TerrariumDecoder)),
                // Raw16 tiles aren't PNG-encoded pixel-triplets like the other
                // two encodings; baking isn't wired for this format yet — skip
                // (every tile falls back to live synthesis at runtime, FR-5).
                ElevationEncoding::Raw16 => None,
            };
            let Some(decoder) = decoder else {
                return Vec::new();
            };

            let mut out = Vec::new();
            for (cache_key, png_bytes) in tiles {
                let Some(coord) = parse_cache_key(cache_key) else {
                    continue;
                };
                let Ok((pixels, w, h)) = decode_png_pixels(png_bytes) else {
                    continue;
                };
                if w < 2 || h < 2 {
                    continue;
                }
                let elevations = decoder.decode(&pixels, w, h);

                let (nw_lat, _) = coord.to_lat_lon();
                let (s_lat, _) =
                    TileCoord::new(coord.x, coord.y.saturating_add(1), coord.zoom).to_lat_lon();
                let tile_size = tile_world_size_m((nw_lat + s_lat) / 2.0, coord.zoom);

                let native = synthesize_splats(
                    &elevations,
                    w as usize,
                    h as usize,
                    None,
                    tile_size,
                    1.0,
                    BAKE_SPLAT_STRIDE,
                );
                if native.is_empty() {
                    continue;
                }
                let baked = bake_splat_coverage(&native).unwrap_or(native);
                let record: BakedSplatBuffer = (&baked).into();
                let Ok(bytes) = encode_baked_splats(&record) else {
                    continue;
                };
                out.push((cache_key.clone(), bytes));
            }
            out
        }

        /// Builds a fresh `TilesetMeta` for one of this file's three
        /// construction sites (whole-archive export, per-chunk export, outer
        /// container export). Never overrides `crs` — `..Default::default()`
        /// already stamps `fe_format::manifest::DEFAULT_TILESET_CRS`, the
        /// house rule in `fe-format/src/AGENTS.md` §scale. One helper means
        /// one test (`a_freshly_packaged_tileset_records_epsg_4326_in_its_raw_archive_meta`)
        /// covers every site a future `crs: None` regression could creep
        /// back into.
        fn fresh_tileset_meta(
            &self,
            bounds: [f64; 4],
            min_zoom: u8,
            max_zoom: u8,
            tile_count: u32,
        ) -> TilesetMeta {
            TilesetMeta {
                bounds,
                min_zoom,
                max_zoom,
                tile_size: 256,
                elevation_encoding: self.elevation_encoding.clone(),
                tile_count,
                region_name: self.region_name.clone(),
                ..Default::default()
            }
        }

        /// Download tiles and package into a single `.hexon` archive.
        pub async fn package_hexon(
            &self,
            publisher_did: &str,
            signing_key: Option<&ed25519_dalek::SigningKey>,
        ) -> Result<Vec<u8>> {
            let (result, tiles) = self.download_tiles().await?;

            let mut manifest = HexonManifest {
                schema_version: "1.0.0".into(),
                hexon_id: format!(
                    "tileset-{}",
                    self.region_name.to_lowercase().replace(' ', "-")
                ),
                hexon_type: HexonType::TerrainTileset,
                publisher_did: publisher_did.into(),
                publisher_name: None,
                version: "0.1.0".into(),
                build_id: None,
                name: format!("Terrain Tileset — {}", self.region_name),
                description: Some(format!(
                    "Elevation tileset for {} ({} tiles, zoom {}-{})",
                    self.region_name, result.tiles_downloaded, self.min_zoom, self.max_zoom
                )),
                tags: vec!["terrain".into(), "tileset".into()],
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                source_peer_did: None,
                approx_size_bytes: Some(result.total_bytes),
                min_engine_version: None,
                homepage_url: None,
                dependencies: vec![],
                platforms: vec![],
                address: None,
                integrity: None,
                signature: None,
            };

            if let Some(key) = signing_key {
                let json = serde_json::to_vec(&manifest)?;
                let sig = fe_format::sign_manifest(&json, key)?;
                manifest.signature = Some(sig);
            }

            let meta = self.fresh_tileset_meta(
                self.bounds,
                self.min_zoom,
                self.max_zoom,
                result.tiles_downloaded,
            );

            let archive_bytes = HexonArchive::export_tileset(
                manifest,
                &meta,
                &tiles,
                &[],
                Some(License::default()),
            )?;

            // FR-3: bake coverage-filled splat buffers and embed them alongside
            // the tiles. Baking is best-effort — if every tile fails to bake
            // (e.g. unsupported encoding) the archive still exports fine and
            // the runtime falls back to live synthesis (FR-5).
            let splat_entries = self.bake_splat_tiles(&tiles);
            crate::splat::format::append_baked_splats_to_archive(&archive_bytes, &splat_entries)
        }

        /// Package already-downloaded tiles into a SEALED chunked container
        /// (campaign plan p2p_asset_streaming S6 step 4): a pure function of
        /// `tiles` (no network I/O), so tests and re-packaging callers never
        /// need a `TileSource`. Replaces the old per-call signing of each
        /// chunk's own legacy manifest (D-76: "a hash committed under one
        /// signed manifest is equivalent trust" — one outer seal, not N).
        ///
        /// Splitting goes through the shared `bundle::split_tiles_into_bundles`
        /// (Hilbert ordering + greedy size-fill), the same grouping
        /// `bundle_tileset` uses. Each chunk is its own unsealed `.hexon`
        /// (splat-baked); the OUTER container (`HexonArchive::export_chunked`)
        /// is sealed LAST, after every chunk (hence every splat append) is
        /// final — fe-format AGENTS.md §integrity "seal is always the last
        /// transform". `signing_key: None` leaves the container unsealed,
        /// matching `package_hexon`'s existing optional-signing convention —
        /// an unsealed container still installs (flagged `verified: false`).
        pub fn package_tiles_chunked(
            &self,
            tiles: &[(String, Vec<u8>)],
            publisher_did: &str,
            signing_key: Option<&ed25519_dalek::SigningKey>,
            max_chunk_size_mb: u32,
        ) -> Result<Vec<u8>> {
            let max_bytes = max_chunk_size_mb as u64 * 1024 * 1024;
            let slices = crate::tiles::bundle::split_tiles_into_bundles(tiles, &[], max_bytes)?;
            let total_chunks = slices.len() as u32;
            let tileset_id = format!(
                "tileset-{}",
                self.region_name.to_lowercase().replace(' ', "-")
            );
            // One timestamp for the whole build, not one per chunk — chunks
            // built in the same call are the same "build", so they share a
            // `created_at`/`updated_at` instead of drifting across the loop.
            let now = chrono::Utc::now().to_rfc3339();

            let mut chunk_inputs = Vec::with_capacity(slices.len());
            for slice in slices {
                let seq = slice.seq;
                let chunk_tiles = slice.elevation_tiles;
                let tile_count = chunk_tiles.len() as u32;
                let chunk_bytes: u64 = chunk_tiles.iter().map(|(_, d)| d.len() as u64).sum();
                let chunk_bounds = crate::tiles::bundle::bounds_for_tile_range(
                    slice.zoom,
                    slice.x_range,
                    slice.y_range,
                );

                let chunk_manifest = HexonManifest {
                    schema_version: "1.0.0".into(),
                    hexon_id: format!("{tileset_id}-chunk-{seq}"),
                    hexon_type: HexonType::TerrainTileset,
                    publisher_did: publisher_did.into(),
                    publisher_name: None,
                    version: "0.1.0".into(),
                    build_id: None,
                    name: format!(
                        "Terrain Tileset — {} (chunk {}/{})",
                        self.region_name,
                        seq + 1,
                        total_chunks
                    ),
                    description: None,
                    tags: vec!["terrain".into(), "tileset".into(), "chunk".into()],
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    source_peer_did: None,
                    approx_size_bytes: Some(chunk_bytes),
                    min_engine_version: None,
                    homepage_url: None,
                    dependencies: vec![],
                    platforms: vec![],
                    address: None,
                    integrity: None,
                    // Chunks are not individually sealed in this slice (only
                    // the outer container is) — see the function doc comment.
                    signature: None,
                };

                let chunk_meta =
                    self.fresh_tileset_meta(chunk_bounds, slice.zoom, slice.zoom, tile_count);

                let chunk_archive = HexonArchive::export_tileset(
                    chunk_manifest,
                    &chunk_meta,
                    &chunk_tiles,
                    &[],
                    Some(License::default()),
                )
                .with_context(|| format!("failed to export chunk {seq}"))?;
                let splat_entries = self.bake_splat_tiles(&chunk_tiles);
                let chunk_archive = append_baked_splats_to_archive(&chunk_archive, &splat_entries)
                    .with_context(|| format!("failed to append splats to chunk {seq}"))?;

                chunk_inputs.push(fe_format::ChunkInput {
                    seq,
                    tile_count,
                    zoom: slice.zoom,
                    x_range: slice.x_range,
                    y_range: slice.y_range,
                    hilbert_range: slice.hilbert_range,
                    archive_bytes: chunk_archive,
                });
            }

            let outer_manifest = HexonManifest {
                schema_version: "1.0.0".into(),
                hexon_id: tileset_id.clone(),
                hexon_type: HexonType::TerrainTileset,
                publisher_did: publisher_did.into(),
                publisher_name: None,
                version: "0.1.0".into(),
                build_id: None,
                name: format!("Terrain Tileset — {}", self.region_name),
                description: Some(format!(
                    "Elevation tileset for {} ({} tiles across {} chunks, zoom {}-{})",
                    self.region_name,
                    tiles.len(),
                    total_chunks,
                    self.min_zoom,
                    self.max_zoom
                )),
                tags: vec!["terrain".into(), "tileset".into(), "container".into()],
                created_at: now.clone(),
                updated_at: now,
                source_peer_did: None,
                approx_size_bytes: Some(tiles.iter().map(|(_, d)| d.len() as u64).sum()),
                min_engine_version: None,
                homepage_url: None,
                dependencies: vec![],
                platforms: vec![],
                address: None,
                integrity: None,
                signature: None,
            };
            let outer_meta = self.fresh_tileset_meta(
                self.bounds,
                self.min_zoom,
                self.max_zoom,
                tiles.len() as u32,
            );
            let container_bytes =
                HexonArchive::export_chunked(outer_manifest, &outer_meta, &chunk_inputs)
                    .context("failed to export chunked container")?;

            match signing_key {
                Some(key) => {
                    fe_format::seal(&container_bytes, key).context("failed to seal container")
                }
                None => Ok(container_bytes),
            }
        }

        /// Download then package into a sealed chunked container — the
        /// network-fetching convenience wrapper over
        /// [`Self::package_tiles_chunked`] (step 1: `download_tiles`, step 4's
        /// pure packaging). Returns ONE `.hexon` container instead of the old
        /// `Vec<Vec<u8>>` of separately unsigned chunk archives, since a
        /// container's chunks are independently recoverable via
        /// `fe_format::HexonArchiveIndex::chunk` without flattening.
        pub async fn package_chunked(
            &self,
            publisher_did: &str,
            signing_key: Option<&ed25519_dalek::SigningKey>,
            max_chunk_size_mb: u32,
        ) -> Result<Vec<u8>> {
            let (_result, tiles) = self.download_tiles().await?;
            self.package_tiles_chunked(&tiles, publisher_did, signing_key, max_chunk_size_mb)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ed25519_dalek::SigningKey;
        use fe_format::{verify_archive, HexonArchiveIndex, IntegrityPolicy, IntegrityStatus};

        /// S6: `package_tiles_chunked` produces a container whose OUTER seal
        /// survives — unlike the legacy per-chunk-manifest signing this
        /// replaces, there is one signature to check, over one committed
        /// `chunks/index.json`.
        #[test]
        fn package_tiles_chunked_produces_a_container_that_verifies_sealed() {
            let key = SigningKey::from_bytes(&[41u8; 32]);
            let builder = TilesetBuilder::new(
                "Flow Region",
                [45.0, -122.0, 46.0, -121.0],
                10,
                10,
                ElevationEncoding::TerrainRgb,
            );
            // 6 tiles x 300 KiB = ~1.8 MiB raw, well over the 1 MiB cap below,
            // so the greedy size-fill must split this into more than one chunk.
            let tiles: Vec<(String, Vec<u8>)> = (0..6u32)
                .map(|x| (format!("10/{x}/0"), vec![x as u8; 300 * 1024]))
                .collect();

            let container = builder
                .package_tiles_chunked(&tiles, "did:key:z6Mkbuilder", Some(&key), 1)
                .expect("package_tiles_chunked failed");

            let status = verify_archive(&container, &IntegrityPolicy::default())
                .expect("container must verify");
            assert_eq!(
                status,
                IntegrityStatus::Sealed {
                    publisher_did: fe_format::verifying_key_to_did(&key.verifying_key())
                }
            );

            let path = std::env::temp_dir().join(format!(
                "fe_builder_chunked_test_{}.hexon",
                std::process::id()
            ));
            std::fs::write(&path, &container).unwrap();
            let index = HexonArchiveIndex::open(&path).expect("open container");
            assert!(matches!(
                index.layout(),
                fe_format::Layout::Container { chunk_count } if chunk_count > 1
            ));
            // Every original tile is reachable through SOME chunk.
            for (key, _) in &tiles {
                let (z, x, y) = {
                    let mut p = key.split('/');
                    (
                        p.next().unwrap().parse::<u8>().unwrap(),
                        p.next().unwrap().parse::<u32>().unwrap(),
                        p.next().unwrap().parse::<u32>().unwrap(),
                    )
                };
                let bundle_index: fe_format::manifest::TilesetBundleIndex = serde_json::from_slice(
                    &index
                        .read_member(fe_format::integrity::CHUNK_INDEX_PATH)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                assert!(
                    bundle_index.bundle_for_tile(z, x, y).is_some(),
                    "tile {key} must be covered by some chunk"
                );
            }
            let _ = std::fs::remove_file(&path);
        }

        /// FEAPI-BUILDER-CRS: a struct-level assert on `TilesetMeta.crs` is a
        /// tautology — serde fills an absent key back in as
        /// `Some("EPSG:4326")` on deserialize, so it would pass whether or
        /// not the builder ever wrote the key. This reads the OUTER and every
        /// per-chunk `terrain/tileset_meta.json` as raw bytes and asserts the
        /// key is actually present in the archive. Mutation check:
        /// reintroduce `crs: None` at any of the three builder.rs sites and
        /// the written key goes missing, so this goes red.
        #[test]
        fn a_freshly_packaged_tileset_records_epsg_4326_in_its_raw_archive_meta() {
            let key = SigningKey::from_bytes(&[42u8; 32]);
            let builder = TilesetBuilder::new(
                "CRS Region",
                [45.0, -122.0, 46.0, -121.0],
                10,
                10,
                ElevationEncoding::TerrainRgb,
            );
            // Same shape as the sealed-container test: big enough that the
            // greedy size-fill must split into more than one chunk, so the
            // per-chunk assertion below actually exercises >1 iteration.
            let tiles: Vec<(String, Vec<u8>)> = (0..6u32)
                .map(|x| (format!("10/{x}/0"), vec![x as u8; 300 * 1024]))
                .collect();

            let container = builder
                .package_tiles_chunked(&tiles, "did:key:z6Mkbuilder", Some(&key), 1)
                .expect("package_tiles_chunked failed");

            let path = std::env::temp_dir().join(format!(
                "fe_builder_crs_test_{}.hexon",
                std::process::id()
            ));
            std::fs::write(&path, &container).unwrap();
            let index = HexonArchiveIndex::open(&path).expect("open container");

            let outer_raw = index
                .read_member("terrain/tileset_meta.json")
                .expect("read outer tileset_meta.json")
                .expect("outer tileset_meta.json must be present");
            let outer_value: serde_json::Value =
                serde_json::from_slice(&outer_raw).expect("outer tileset_meta.json must be JSON");
            assert_eq!(
                outer_value["crs"],
                fe_format::manifest::DEFAULT_TILESET_CRS,
                "outer tileset_meta.json must carry the crs key"
            );

            let chunk_count = match index.layout() {
                fe_format::Layout::Container { chunk_count } => chunk_count,
                fe_format::Layout::Monolithic => panic!("expected a chunked container"),
            };
            assert!(chunk_count > 1, "fixture must produce more than one chunk");
            for seq in 0..chunk_count {
                let chunk_bytes = index
                    .chunk(seq)
                    .expect("read chunk")
                    .unwrap_or_else(|| panic!("chunk {seq} must be present"));
                let chunk_meta_raw = fe_format::HexonArchive::read_member(
                    &chunk_bytes,
                    "terrain/tileset_meta.json",
                    fe_format::limits::MAX_JSON_ENTRY_BYTES,
                )
                .expect("read chunk tileset_meta.json")
                .unwrap_or_else(|| panic!("chunk {seq} tileset_meta.json must be present"));
                let chunk_value: serde_json::Value = serde_json::from_slice(&chunk_meta_raw)
                    .expect("chunk tileset_meta.json must be JSON");
                assert_eq!(
                    chunk_value["crs"],
                    fe_format::manifest::DEFAULT_TILESET_CRS,
                    "chunk {seq} tileset_meta.json must carry the crs key"
                );
            }

            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(feature = "fetch")]
mod cache_key_parse {
    use crate::tiles::source::TileCoord;

    /// Parse a `{zoom}/{x}/{y}` cache key back into a [`TileCoord`] (inverse of
    /// `TileCoord::cache_key`) for the bake step, which needs the coordinate to
    /// compute tile world size.
    pub(super) fn parse_cache_key(key: &str) -> Option<TileCoord> {
        let mut parts = key.splitn(3, '/');
        let zoom: u8 = parts.next()?.parse().ok()?;
        let x: u32 = parts.next()?.parse().ok()?;
        let y: u32 = parts.next()?.parse().ok()?;
        Some(TileCoord::new(x, y, zoom))
    }
}

#[cfg(feature = "fetch")]
use cache_key_parse::parse_cache_key;

#[cfg(feature = "fetch")]
pub use inner::*;
