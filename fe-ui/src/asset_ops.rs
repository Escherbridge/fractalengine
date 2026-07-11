//! Pending node-asset ops queued for the main binary to resolve — fe-ui has
//! no `BlobStoreHandle`/DB access, mirroring [`crate::terrain_map::HexonOp`].

use bevy::prelude::*;

/// Asset operation requested by the UI; drained by the main binary.
#[derive(Debug, Clone)]
pub enum AssetOp {
    /// Download the given node's asset to local storage.
    Download { node_id: String },
}

/// Queue of pending asset ops (fe-ui has no `BlobStoreHandle` access).
#[derive(Debug, Default, Resource)]
pub struct PendingAssetOps(pub Vec<AssetOp>);

/// Outcome of the most recently drained asset download, written by the main
/// binary's bridge system so the UI can surface success/failure. Mirrors the
/// `HexonOpenStorageDir` precedent of exposing bridge-side file outcomes to
/// the UI layer.
#[derive(Debug, Clone, Default, Resource)]
pub struct AssetDownloadStatus {
    /// Node ID the last completed/failed download was for.
    pub node_id: Option<String>,
    /// Local path the asset was saved to, if the download succeeded.
    pub saved_path: Option<String>,
    /// Error message, if the download failed.
    pub error: Option<String>,
}
