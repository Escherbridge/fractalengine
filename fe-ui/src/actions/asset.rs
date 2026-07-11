//! Node asset download action — fe-ui only queues the request; the main
//! binary (owns `BlobStoreHandle` + the DB channel) resolves it and writes
//! the outcome to `crate::asset_ops::AssetDownloadStatus`.

use crate::asset_ops::{AssetOp, PendingAssetOps};

pub(crate) fn request_download(asset_ops: &mut PendingAssetOps, node_id: String) {
    bevy::log::info!("Asset: download requested for node {}", node_id);
    asset_ops.0.push(AssetOp::Download { node_id });
}
