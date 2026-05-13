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

    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::license::License;
    use fe_format::HexonArchive;

    use crate::tiles::source::{TileCoord, TileSource};

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
            let tiles: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));

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

        /// Download tiles and package into a single `.hexon` archive.
        pub async fn package_hexon(
            &self,
            publisher_did: &str,
            signing_key: Option<&ed25519_dalek::SigningKey>,
        ) -> Result<Vec<u8>> {
            let (result, tiles) = self.download_tiles().await?;

            let mut manifest = HexonManifest {
                schema_version: "1.0.0".into(),
                hexon_id: format!("tileset-{}", self.region_name.to_lowercase().replace(' ', "-")),
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
                signature: None,
            };

            if let Some(key) = signing_key {
                let json = serde_json::to_vec(&manifest)?;
                let sig = fe_format::sign_manifest(&json, key)?;
                manifest.signature = Some(sig);
            }

            let meta = TilesetMeta {
                bounds: self.bounds,
                min_zoom: self.min_zoom,
                max_zoom: self.max_zoom,
                tile_size: 256,
                elevation_encoding: self.elevation_encoding.clone(),
                has_satellite: false,
                tile_count: result.tiles_downloaded,
                satellite_tile_count: 0,
                region_name: self.region_name.clone(),
                parent_tileset: None,
                chunk_index: None,
            };

            HexonArchive::export_tileset(manifest, &meta, &tiles, &[], Some(License::default()))
        }

        /// Download tiles and package into multiple `.hexon` chunk archives.
        ///
        /// Each chunk is at most `max_chunk_size_mb` megabytes.
        pub async fn package_chunked(
            &self,
            publisher_did: &str,
            signing_key: Option<&ed25519_dalek::SigningKey>,
            max_chunk_size_mb: u32,
        ) -> Result<Vec<Vec<u8>>> {
            let (_result, all_tiles) = self.download_tiles().await?;

            let max_bytes = max_chunk_size_mb as u64 * 1024 * 1024;

            // Split tiles into chunks by size
            let mut chunks: Vec<Vec<(String, Vec<u8>)>> = Vec::new();
            let mut current_chunk: Vec<(String, Vec<u8>)> = Vec::new();
            let mut current_size: u64 = 0;

            for (key, data) in all_tiles {
                let len = data.len() as u64;
                if !current_chunk.is_empty() && current_size + len > max_bytes {
                    chunks.push(std::mem::take(&mut current_chunk));
                    current_size = 0;
                }
                current_size += len;
                current_chunk.push((key, data));
            }
            if !current_chunk.is_empty() {
                chunks.push(current_chunk);
            }

            let total_chunks = chunks.len() as u32;
            let tileset_id = format!("tileset-{}", self.region_name.to_lowercase().replace(' ', "-"));

            let mut archives = Vec::with_capacity(chunks.len());
            for (seq, chunk_tiles) in chunks.into_iter().enumerate() {
                let tile_count = chunk_tiles.len() as u32;
                let chunk_bytes: u64 = chunk_tiles.iter().map(|(_, d)| d.len() as u64).sum();

                let mut manifest = HexonManifest {
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
                    created_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    source_peer_did: None,
                    approx_size_bytes: Some(chunk_bytes),
                    min_engine_version: None,
                    homepage_url: None,
                    dependencies: vec![],
                    platforms: vec![],
                    address: None,
                    signature: None,
                };

                if let Some(key) = signing_key {
                    let json = serde_json::to_vec(&manifest)?;
                    let sig = fe_format::sign_manifest(&json, key)?;
                    manifest.signature = Some(sig);
                }

                let meta = TilesetMeta {
                    bounds: self.bounds,
                    min_zoom: self.min_zoom,
                    max_zoom: self.max_zoom,
                    tile_size: 256,
                    elevation_encoding: self.elevation_encoding.clone(),
                    has_satellite: false,
                    tile_count,
                    satellite_tile_count: 0,
                    region_name: self.region_name.clone(),
                    parent_tileset: None,
                    chunk_index: Some(fe_format::manifest::ChunkIndex {
                        tileset_id: tileset_id.clone(),
                        chunk_seq: seq as u32,
                        total_chunks,
                        chunk_bounds: self.bounds,
                    }),
                };

                let archive = HexonArchive::export_tileset(
                    manifest,
                    &meta,
                    &chunk_tiles,
                    &[],
                    Some(License::default()),
                )?;
                archives.push(archive);
            }

            Ok(archives)
        }
    }
}

#[cfg(feature = "fetch")]
pub use inner::*;
