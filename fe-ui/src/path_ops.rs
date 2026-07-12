//! Pending GPX path-editor ops queued for the main binary to resolve —
//! fe-ui has no filesystem/DB access, mirroring [`crate::gpx_ops`]. See
//! `fe-ui/src/AGENTS.md` §path-editor.

use bevy::prelude::*;

/// Path-editor operation requested by the UI; drained by the main binary's
/// GPX bridge (`fractalengine/src/gpx_bridge.rs`). fe-ui only ever produces
/// these — it never reads/writes `gpx_points` itself.
#[derive(Debug, Clone)]
pub enum PathOp {
    /// Create a new (empty) track node named `name` under `petal_id`.
    CreateTrack { petal_id: String, name: String },
    /// Delete the track node `track_node_id` (and its `gpx_points`).
    DeleteTrack { track_node_id: String },
    /// Append a point (petal-local meters) at the end of `track_node_id`'s
    /// point list. `time_seconds` is `None` for authored (non-imported)
    /// points — the bridge stores `[x, y, z, time_seconds.unwrap_or(0.0)]`.
    AppendPoint { track_node_id: String, position: [f32; 3], time_seconds: Option<f64> },
    /// Remove the point at `index` from `track_node_id`'s point list.
    RemovePoint { track_node_id: String, index: usize },
    /// Move the point at `index` to `position` (petal-local meters) in place —
    /// preferred over remove+append for viewport drags: no index churn.
    MovePoint { track_node_id: String, index: usize, position: [f32; 3] },
    /// Create a waypoint annotation node at the position of point `index` in
    /// `track_node_id`'s list, via the existing `gis.annotation.*` property
    /// contract (see `crate::actions::node_props`).
    AnnotatePoint {
        track_node_id: String,
        index: usize,
        title: String,
        body: String,
        color: String,
    },
    /// Export `track_node_id`'s points as a standard GPX 1.1 file (rfd save
    /// dialog is the bridge's responsibility — fe-ui just signals intent).
    ExportGpx { track_node_id: String },
}

/// Queue of pending path-editor ops (fe-ui has no file-parsing/DB access).
#[derive(Debug, Default, Resource)]
pub struct PendingPathOps(pub Vec<PathOp>);

/// Outcome of the most recently drained path-editor op, written by the main
/// binary's bridge system so the UI can surface success/failure. Mirrors
/// `gpx_ops::GpxImportStatus`'s shape.
#[derive(Debug, Clone, Default, Resource)]
pub struct PathEditStatus {
    /// Track node ID the last completed/failed op was for.
    pub track_node_id: Option<String>,
    /// Human-readable description of the op that completed (e.g. "Exported
    /// 12 points", "Point removed") — the bridge's choice of wording.
    pub message: Option<String>,
    /// Error message, if the op failed.
    pub error: Option<String>,
}

/// Surfaces the outcome of the most recently drained path-editor op (written
/// by the main binary's GPX bridge) as a toast — mirrors
/// `gpx_ops::surface_gpx_import_status`.
pub fn surface_path_edit_status(
    status: Res<PathEditStatus>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    time: Res<Time>,
) {
    if !status.is_changed() || status.track_node_id.is_none() {
        return;
    }
    let now = time.elapsed_secs_f64();
    if let Some(err) = &status.error {
        bevy::log::warn!("Path edit failed: {err}");
        ui_mgr.show_toast(format!("Path edit failed: {err}"), now);
    } else if let Some(msg) = &status.message {
        bevy::log::info!("Path edit: {msg}");
        ui_mgr.show_toast(msg.clone(), now);
    }
}
