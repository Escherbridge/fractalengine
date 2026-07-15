//! Pending GPX track import ops queued for the main binary to resolve —
//! fe-ui has no filesystem/DB access, mirroring [`crate::asset_ops`].

use bevy::prelude::*;

/// GPX operation requested by the UI; drained by the main binary.
#[derive(Debug, Clone)]
pub enum GpxOp {
    /// Import the GPX file at `path` into `petal_id`.
    ImportFile {
        petal_id: String,
        path: std::path::PathBuf,
    },
}

/// Queue of pending GPX ops (fe-ui has no file-parsing/DB access).
#[derive(Debug, Default, Resource)]
pub struct PendingGpxOps(pub Vec<GpxOp>);

/// Outcome of the most recently drained GPX import, written by the main
/// binary's bridge system so the UI can surface success/failure. Contract
/// fixed by the bridge worker: counts, not a single track name, since a GPX
/// file may contain multiple tracks/waypoints.
#[derive(Debug, Clone, Default, Resource)]
pub struct GpxImportStatus {
    /// Petal ID the last completed/failed import was for.
    pub petal_id: Option<String>,
    /// Number of tracks imported (0 on failure).
    pub track_count: u32,
    /// Number of waypoints imported (0 on failure).
    pub waypoint_count: u32,
    /// Error message, if the import failed.
    pub error: Option<String>,
}

/// Surfaces the outcome of the most recently drained GPX import (written by
/// the main binary's GPX bridge) as a toast — mirrors
/// `asset_ops::surface_asset_download_status`.
pub fn surface_gpx_import_status(
    status: Res<GpxImportStatus>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    time: Res<Time>,
) {
    if !status.is_changed() || status.petal_id.is_none() {
        return;
    }
    let now = time.elapsed_secs_f64();
    if let Some(err) = &status.error {
        bevy::log::warn!("GPX import failed: {err}");
        ui_mgr.show_toast(format!("GPX import failed: {err}"), now);
    } else {
        bevy::log::info!(
            "GPX imported: {} track(s), {} waypoint(s)",
            status.track_count,
            status.waypoint_count
        );
        ui_mgr.show_toast(
            format!(
                "GPX imported: {} track(s), {} waypoint(s)",
                status.track_count, status.waypoint_count
            ),
            now,
        );
    }
}
