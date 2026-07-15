//! GPX track import action — fe-ui only queues the request; the main
//! binary resolves the file and imports it as a node/track for the petal.
//! Mirrors `actions::asset`'s queue-for-main-binary pattern.

use std::path::PathBuf;

use crate::gpx_ops::{GpxOp, PendingGpxOps};

pub(crate) fn request_import(gpx_ops: &mut PendingGpxOps, petal_id: String, path: PathBuf) {
    bevy::log::info!(
        "GPX: import requested for petal {petal_id}: {}",
        path.display()
    );
    gpx_ops.0.push(GpxOp::ImportFile { petal_id, path });
}
