//! Node asset download action — fe-ui only queues the request; the main
//! binary (owns `BlobStoreHandle` + the DB channel) resolves it and writes
//! the outcome to `crate::asset_ops::AssetDownloadStatus`.
//!
//! Also homes the Wave-1 stamped-asset (T2) interaction-state stub +
//! per-verb handler stubs — see the Wave-1 registration scaffold in
//! `actions/mod.rs`. T2 fills these bodies; it edits no central file.

use bevy::prelude::Resource;
use fe_runtime::app::DbCommandSender;

use crate::asset_ops::{AssetOp, PendingAssetOps};

pub(crate) fn request_download(asset_ops: &mut PendingAssetOps, node_id: String) {
    bevy::log::info!("Asset: download requested for node {}", node_id);
    asset_ops.0.push(AssetOp::Download { node_id });
}

/// Wave 1 (T2 stamped_asset_nodes): per-frame stamp interaction state
/// (which stamp is individually selected, in-flight scale/rotate/slide
/// gesture, spatial pick cache). Body filled by T2; registered in `plugin.rs`.
#[derive(Resource, Default)]
pub(crate) struct StampInteractionState {
    // Wave 1: T2 fills fields (selected stamp identity, gesture deltas, …).
}

/// Wave 1: T2 — select a stamp individually (promotes on first select, T2 FR-2).
pub(crate) fn handle_select_stamp(
    _stamp_state: &mut StampInteractionState,
    _track_node_id: String,
    _stamp_index: usize,
) {
    // Wave 1: T2 stamped_asset_nodes fills this.
}

/// Wave 1: T2 — persist a stamp's per-node scale override (T2 FR-3).
pub(crate) fn handle_set_stamp_scale(
    _db_sender: &DbCommandSender,
    _stamp_state: &mut StampInteractionState,
    _track_node_id: String,
    _stamp_index: usize,
    _scale: [f32; 3],
) {
    // Wave 1: T2 stamped_asset_nodes fills this.
}

/// Wave 1: T2 — persist a stamp's per-node rotation override (T2 FR-3).
pub(crate) fn handle_set_stamp_rotation(
    _db_sender: &DbCommandSender,
    _stamp_state: &mut StampInteractionState,
    _track_node_id: String,
    _stamp_index: usize,
    _rotation: [f32; 4],
) {
    // Wave 1: T2 stamped_asset_nodes fills this.
    // (Arc-length "slide along path" is homed in `actions/path.rs` — curve domain.)
}
