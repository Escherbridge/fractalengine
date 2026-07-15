//! P2P chunk distribution for terrain tilesets.
//!
//! This module provides the data layer for distributing chunked `.hexon`
//! tilesets between peers. The actual network transport lives in `fe-sync`;
//! this module handles:
//!
//! - Tileset advertisement DTOs (what a peer has available)
//! - Download progress tracking
//! - Chunk assembly (merging received chunks into a complete tileset)
//! - Partial-availability tile serving (serve tiles from chunks we have)

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing;

use fe_format::manifest::TilesetMeta;

use super::hexon_source::HexonTileSource;
use super::store::HexonStore;

// ---------------------------------------------------------------------------
// Advertisement — what a peer tells others it has
// ---------------------------------------------------------------------------

/// Describes a tileset that a peer is seeding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TilesetAdvertisement {
    /// The root tileset ID (not chunk-specific).
    pub tileset_id: String,
    /// Human-readable region name.
    pub region_name: String,
    /// Geographic bounds `[min_lat, min_lon, max_lat, max_lon]`.
    pub bounds: [f64; 4],
    /// Zoom range.
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// Total number of tiles across all chunks.
    pub tile_count: u32,
    /// Total number of chunks the tileset is split into.
    pub total_chunks: u32,
    /// Approximate total size in bytes.
    pub approx_size_bytes: u64,
    /// Which chunk sequences this peer has available (may be partial).
    pub available_chunks: Vec<u32>,
}

/// A batch of advertisements from a single peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAdvertisement {
    /// The peer's DID (`did:key:z6Mk…`).
    pub peer_id: String,
    /// Tilesets this peer is seeding.
    pub tilesets: Vec<TilesetAdvertisement>,
}

// ---------------------------------------------------------------------------
// Download progress
// ---------------------------------------------------------------------------

/// Status of a chunk download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkStatus {
    /// Not yet requested.
    Missing,
    /// Request sent, waiting for data.
    Requested,
    /// Chunk received and stored locally.
    Received,
    /// Chunk failed to download.
    Failed(String),
}

/// Tracks the download progress of a multi-chunk tileset.
#[derive(Debug, Clone)]
pub struct TilesetDownload {
    /// Root tileset ID.
    pub tileset_id: String,
    /// Total number of chunks expected.
    pub total_chunks: u32,
    /// Per-chunk status, keyed by chunk_seq.
    pub chunks: HashMap<u32, ChunkStatus>,
    /// Bytes received so far.
    pub bytes_received: u64,
    /// Estimated total bytes.
    pub total_bytes_estimate: u64,
    /// Whether the download has been cancelled.
    pub cancelled: bool,
}

impl TilesetDownload {
    /// Create a new download tracker for a tileset.
    pub fn new(tileset_id: String, total_chunks: u32, total_bytes_estimate: u64) -> Self {
        let mut chunks = HashMap::with_capacity(total_chunks as usize);
        for seq in 0..total_chunks {
            chunks.insert(seq, ChunkStatus::Missing);
        }
        Self {
            tileset_id,
            total_chunks,
            chunks,
            bytes_received: 0,
            total_bytes_estimate,
            cancelled: false,
        }
    }

    /// Number of chunks received.
    pub fn chunks_received(&self) -> u32 {
        self.chunks
            .values()
            .filter(|s| **s == ChunkStatus::Received)
            .count() as u32
    }

    /// Whether all chunks have been received.
    pub fn is_complete(&self) -> bool {
        self.chunks_received() == self.total_chunks
    }

    /// Return the next chunk sequence that needs downloading (Missing status).
    pub fn next_missing_chunk(&self) -> Option<u32> {
        let mut missing: Vec<u32> = self
            .chunks
            .iter()
            .filter(|(_, s)| **s == ChunkStatus::Missing)
            .map(|(seq, _)| *seq)
            .collect();
        missing.sort();
        missing.first().copied()
    }

    /// Mark a chunk as requested.
    pub fn mark_requested(&mut self, chunk_seq: u32) {
        self.chunks.insert(chunk_seq, ChunkStatus::Requested);
    }

    /// Mark a chunk as received and add its byte count.
    pub fn mark_received(&mut self, chunk_seq: u32, chunk_bytes: u64) {
        self.chunks.insert(chunk_seq, ChunkStatus::Received);
        self.bytes_received += chunk_bytes;
    }

    /// Mark a chunk as failed.
    pub fn mark_failed(&mut self, chunk_seq: u32, reason: String) {
        self.chunks.insert(chunk_seq, ChunkStatus::Failed(reason));
    }
}

// ---------------------------------------------------------------------------
// Chunk assembly
// ---------------------------------------------------------------------------

/// Assembles received chunks into a complete tileset and installs it.
///
/// Each chunk is a self-contained `.hexon` archive containing a subset of the
/// tileset's tiles with `ChunkIndex` metadata. Chunks can arrive in any order.
///
/// Returns the installed tileset's hexon_id on success.
pub fn assemble_chunks(
    store: &HexonStore,
    tileset_id: &str,
    chunk_archives: &[Vec<u8>],
) -> Result<String> {
    if chunk_archives.is_empty() {
        bail!("no chunk archives to assemble");
    }

    // Parse all chunks
    let mut all_elevation: HashMap<String, Vec<u8>> = HashMap::new();
    let mut all_satellite: HashMap<String, Vec<u8>> = HashMap::new();
    let mut base_meta: Option<TilesetMeta> = None;
    let mut seen_chunks: Vec<u32> = Vec::new();
    let mut expected_total: Option<u32> = None;

    for (i, archive_bytes) in chunk_archives.iter().enumerate() {
        let data = fe_format::HexonArchive::import(archive_bytes)
            .with_context(|| format!("failed to import chunk archive {i}"))?;

        let meta = data
            .tileset_meta
            .with_context(|| format!("chunk {i} missing tileset_meta"))?;

        // Validate chunk index
        let chunk_idx = meta
            .chunk_index
            .as_ref()
            .with_context(|| format!("chunk {i} missing chunk_index"))?;

        if chunk_idx.tileset_id != tileset_id {
            bail!(
                "chunk {i} tileset_id mismatch: expected '{}', got '{}'",
                tileset_id,
                chunk_idx.tileset_id
            );
        }

        if let Some(total) = expected_total {
            if chunk_idx.total_chunks != total {
                bail!(
                    "chunk {i} total_chunks mismatch: expected {total}, got {}",
                    chunk_idx.total_chunks
                );
            }
        } else {
            expected_total = Some(chunk_idx.total_chunks);
        }

        if seen_chunks.contains(&chunk_idx.chunk_seq) {
            tracing::warn!(
                chunk_seq = chunk_idx.chunk_seq,
                "duplicate chunk sequence, skipping"
            );
            continue;
        }
        seen_chunks.push(chunk_idx.chunk_seq);

        // Merge tiles
        for (key, bytes) in data.elevation_tiles {
            all_elevation.insert(key, bytes);
        }
        for (key, bytes) in data.satellite_tiles {
            all_satellite.insert(key, bytes);
        }

        // Use first chunk's meta as base (sans chunk_index)
        if base_meta.is_none() {
            let mut m = meta;
            m.chunk_index = None;
            // Update tile counts to reflect the full tileset
            m.tile_count = 0; // will be set after merge
            m.satellite_tile_count = 0;
            base_meta = Some(m);
        }
    }

    let total = expected_total.context("could not determine total_chunks")?;
    if seen_chunks.len() != total as usize {
        bail!(
            "incomplete tileset: got {} of {} chunks",
            seen_chunks.len(),
            total
        );
    }

    let mut meta = base_meta.context("no base meta extracted")?;
    meta.tile_count = all_elevation.len() as u32;
    meta.satellite_tile_count = all_satellite.len() as u32;

    // Build the composite manifest
    let manifest = fe_format::manifest::HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: tileset_id.to_string(),
        hexon_type: fe_format::manifest::HexonType::TerrainTileset,
        publisher_did: String::new(),
        publisher_name: None,
        version: "1.0.0".into(),
        build_id: None,
        name: format!("{} (assembled)", meta.region_name),
        description: None,
        tags: vec!["terrain".into(), "tileset".into(), "assembled".into()],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    };

    let elevation_vec: Vec<(String, Vec<u8>)> = all_elevation.into_iter().collect();
    let satellite_vec: Vec<(String, Vec<u8>)> = all_satellite.into_iter().collect();

    let archive_bytes = fe_format::HexonArchive::export_tileset(
        manifest,
        &meta,
        &elevation_vec,
        &satellite_vec,
        None, // no signing for assembled tilesets
    )
    .context("failed to export assembled tileset")?;

    let installed = store
        .install_tileset(&archive_bytes)
        .context("failed to install assembled tileset")?;

    tracing::info!(
        hexon_id = %installed.hexon_id,
        tiles = installed.tile_count,
        "Assembled and installed tileset from {} chunks",
        total
    );

    Ok(installed.hexon_id)
}

// ---------------------------------------------------------------------------
// Partial-availability source
// ---------------------------------------------------------------------------

/// A tile source that serves tiles from whichever chunks have been received,
/// even if the tileset download is incomplete.
///
/// Missing tiles return `None`, allowing `CompositeTileSource` to fall back
/// to an online source.
pub struct PartialTileSource {
    /// Loaded chunk sources, keyed by chunk_seq.
    chunks: HashMap<u32, HexonTileSource>,
}

impl Default for PartialTileSource {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialTileSource {
    /// Create an empty partial source.
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
        }
    }

    /// Add a received chunk. The chunk is parsed from raw `.hexon` bytes.
    pub fn add_chunk(&mut self, chunk_seq: u32, archive_bytes: &[u8]) -> Result<()> {
        let source = HexonTileSource::from_archive(archive_bytes)
            .with_context(|| format!("failed to parse chunk {chunk_seq}"))?;
        self.chunks.insert(chunk_seq, source);
        Ok(())
    }

    /// Look up an elevation tile across all loaded chunks.
    pub fn get_tile(&self, coord: super::source::TileCoord) -> Option<&[u8]> {
        for source in self.chunks.values() {
            if let Some(tile) = source.get_tile(coord) {
                return Some(tile);
            }
        }
        None
    }

    /// Look up a satellite tile across all loaded chunks.
    pub fn get_satellite_tile(&self, coord: super::source::TileCoord) -> Option<&[u8]> {
        for source in self.chunks.values() {
            if let Some(tile) = source.get_satellite_tile(coord) {
                return Some(tile);
            }
        }
        None
    }

    /// Number of chunks currently loaded.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tileset_download_progress() {
        let mut dl = TilesetDownload::new("test-ts".into(), 4, 1024 * 1024);
        assert_eq!(dl.chunks_received(), 0);
        assert!(!dl.is_complete());
        assert_eq!(dl.next_missing_chunk(), Some(0));

        dl.mark_requested(0);
        assert_eq!(dl.next_missing_chunk(), Some(1));

        dl.mark_received(0, 256 * 1024);
        assert_eq!(dl.chunks_received(), 1);
        assert_eq!(dl.bytes_received, 256 * 1024);

        dl.mark_received(1, 256 * 1024);
        dl.mark_received(2, 256 * 1024);
        dl.mark_received(3, 256 * 1024);
        assert!(dl.is_complete());
        assert_eq!(dl.next_missing_chunk(), None);
    }

    #[test]
    fn tileset_download_failed_chunk() {
        let mut dl = TilesetDownload::new("test-ts".into(), 2, 512);
        dl.mark_failed(0, "timeout".into());
        assert_eq!(dl.chunks_received(), 0);
        assert!(!dl.is_complete());
        // chunk 1 is still missing
        assert_eq!(dl.next_missing_chunk(), Some(1));
    }

    #[test]
    fn advertisement_serialization() {
        let ad = TilesetAdvertisement {
            tileset_id: "ts-001".into(),
            region_name: "Pacific NW".into(),
            bounds: [45.0, -125.0, 49.0, -116.0],
            min_zoom: 10,
            max_zoom: 14,
            tile_count: 500,
            total_chunks: 3,
            approx_size_bytes: 50 * 1024 * 1024,
            available_chunks: vec![0, 1, 2],
        };
        let json = serde_json::to_string(&ad).unwrap();
        let round: TilesetAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(round.tileset_id, "ts-001");
        assert_eq!(round.total_chunks, 3);
    }

    #[test]
    fn peer_advertisement_roundtrip() {
        let pa = PeerAdvertisement {
            peer_id: "did:key:z6Mk123".into(),
            tilesets: vec![TilesetAdvertisement {
                tileset_id: "ts-002".into(),
                region_name: "Rockies".into(),
                bounds: [35.0, -112.0, 45.0, -104.0],
                min_zoom: 8,
                max_zoom: 12,
                tile_count: 1000,
                total_chunks: 5,
                approx_size_bytes: 100 * 1024 * 1024,
                available_chunks: vec![0, 2, 4],
            }],
        };
        let json = serde_json::to_string(&pa).unwrap();
        let round: PeerAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(round.peer_id, "did:key:z6Mk123");
        assert_eq!(round.tilesets.len(), 1);
        assert_eq!(round.tilesets[0].available_chunks, vec![0, 2, 4]);
    }

    #[test]
    fn partial_tile_source_empty() {
        let source = PartialTileSource::new();
        assert_eq!(source.chunk_count(), 0);
        let coord = super::super::source::TileCoord::new(0, 0, 10);
        assert!(source.get_tile(coord).is_none());
    }

    #[test]
    fn assemble_rejects_empty() {
        let dir = std::env::temp_dir().join(format!(
            "assemble_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let store = HexonStore::with_dir(dir).unwrap();
        let result = assemble_chunks(&store, "ts-empty", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no chunk"));
    }
}
