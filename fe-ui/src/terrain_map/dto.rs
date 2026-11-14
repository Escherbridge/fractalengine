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

/// What an installed tileset lets this peer load, as shown in the Hexon Manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TilesetAccess {
    /// Unlicensed or free: every tile loads.
    #[default]
    Installed,
    /// Licensed without a token: free tiles only.
    InstalledPreview,
    /// Licensed without a token and no free tiles.
    EntitlementRequired,
    /// Licensed with a verified token: every tile loads.
    Entitled,
}

impl TilesetAccess {
    /// Parse a gate status label; an unknown label reads as `Installed`.
    pub fn from_label(label: &str) -> Self {
        match label {
            "installed-preview" => Self::InstalledPreview,
            "entitlement-required" => Self::EntitlementRequired,
            "entitled" => Self::Entitled,
            _ => Self::Installed,
        }
    }
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
    /// The sealed archive's publisher `did:key` (`None` = unsealed); "Set
    /// petal map" authors it as the petal's RP-4b publisher pin.
    pub publisher_did: Option<String>,
    /// Load access under the archive's licence (`Installed` when unlicensed).
    pub access: TilesetAccess,
    /// Publisher's licence page; shown as a plain hyperlink, never fetched.
    pub verification_url: Option<String>,
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
