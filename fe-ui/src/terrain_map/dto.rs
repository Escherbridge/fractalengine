//! Hexon Manager DTOs — installed/available tileset rows and download progress.
//! See `fe-ui/src/AGENTS.md` §terrain-map.

use std::collections::HashMap;

/// Which tab of the Hexon Manager dialog is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexonManagerTab {
    Installed,
    Available,
    Downloads,
}

/// A tileset already installed in the local hexon store.
#[derive(Debug, Clone)]
pub struct InstalledTilesetDto {
    pub hexon_id: String,
    pub region_name: String,
    pub bounds: [f64; 4],
    pub zoom_range: (u8, u8),
    pub tile_count: u32,
    pub size_bytes: u64,
    pub seeding_enabled: bool,
    pub installed_at: String,
}

/// A tileset advertised by a peer but not yet installed locally.
#[derive(Debug, Clone)]
pub struct AvailableTilesetDto {
    pub hexon_id: String,
    pub region_name: String,
    pub bounds: [f64; 4],
    pub zoom_range: (u8, u8),
    pub tile_count: u32,
    pub approx_size_bytes: u64,
    pub peer_count: u32,
    pub already_installed: bool,
}

#[derive(Debug, Clone)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Verifying,
    Complete,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub tileset_id: String,
    pub chunks_received: u32,
    pub total_chunks: u32,
    pub bytes_received: u64,
    pub total_bytes_estimate: u64,
    pub status: DownloadStatus,
}

/// Aggregate local hexon storage stats shown in the Hexon Manager footer.
#[derive(Debug, Clone)]
pub struct StorageInfoDto {
    pub base_dir: String,
    pub total_bytes: u64,
    pub count: u32,
}

/// A single download's progress, keyed by tileset ID.
pub type DownloadProgressMap = HashMap<String, DownloadProgress>;
