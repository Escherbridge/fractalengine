//! Path-editor action handling: pure decision helpers + queueing into
//! `PendingPathOps`. fe-ui does no persistence I/O — the main binary's GPX
//! bridge drains the queue. Mirrors `actions::gpx`'s queue-for-main-binary
//! pattern and `actions::gis`'s `RawQuery` submission for track listing.
//! See `fe-ui/src/AGENTS.md` §path-editor.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::gis::{self, PathEditorState, PathPointRow};
use crate::node_manager::curve::{self, PenMode};
use crate::path_ops::{PathOp, PendingPathOps};

/// Runs the "track nodes" query for `petal_id`, routed through
/// `PathEditorState.tracks_pending` (not `GisPanelState.query_pending` —
/// see `PathEditorState`'s doc comment on why this is a separate flag).
pub(crate) fn query_tracks(db_sender: &DbCommandSender, path_state: &mut PathEditorState, petal_id: String) {
    let (sql, vars) = gis::track_query(&petal_id);
    path_state.tracks_pending = true;
    path_state.last_error = None;
    if db_sender.0.send(DbCommand::RawQuery { sql, vars }).is_err() {
        bevy::log::warn!("db_sender channel closed — Paths RawQuery not dispatched");
        path_state.tracks_pending = false;
        path_state.last_error = Some("DB channel closed".to_string());
    }
}

/// Starts editing `track_node_id` and kicks off the `gpx_points` read-back:
/// sets `points_pending` and issues `GetNodeProperties`. The reply is picked
/// up in `verse_manager::db_results`, gated on `editing_track_id` +
/// `points_pending` (see `fe-ui/src/AGENTS.md` §path-editor) so the
/// inspector's own `GetNodeProperties` replies can't stomp this buffer.
pub(crate) fn select_track(db_sender: &DbCommandSender, path_state: &mut PathEditorState, track_node_id: String) {
    path_state.start_editing(track_node_id.clone());
    path_state.points_pending = true;
    if db_sender.0.send(DbCommand::GetNodeProperties { node_id: track_node_id }).is_err() {
        bevy::log::warn!("db_sender channel closed — Paths gpx_points read-back not dispatched");
        path_state.points_pending = false;
        path_state.last_error = Some("DB channel closed".to_string());
    }
}

/// Queues `CreateTrack` and clears the name buffer. Pure validation split
/// out as `validate_track_name` so it's testable without a queue.
/// `correlation_id` is `Some` for the Pen auto-create (threaded to the bridge
/// so its `NodeCreated` echo is matched by the deferred pen flush), `None` for
/// the manual "New Path" button.
pub(crate) fn create_track(
    path_ops: &mut PendingPathOps,
    petal_id: String,
    name: String,
    correlation_id: Option<String>,
) -> Result<(), &'static str> {
    let name = validate_track_name(&name)?;
    path_ops.0.push(PathOp::CreateTrack { petal_id, name, correlation_id });
    Ok(())
}

/// A non-empty (post-trim) name, or an error message.
pub(crate) fn validate_track_name(name: &str) -> Result<String, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err("Enter a name for the new path")
    } else {
        Ok(trimmed.to_string())
    }
}

pub(crate) fn delete_track(path_ops: &mut PendingPathOps, track_node_id: String) {
    path_ops.0.push(PathOp::DeleteTrack { track_node_id });
}

/// Appends a point to both the local edit buffer (immediate UI feedback)
/// and the op queue (persistence intent) — mirrors the Layer Manager's
/// "local preview + queued persist" idiom (`layer_manager_card::preview_layer_edit`).
pub(crate) fn append_point(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    track_node_id: String,
    position: [f32; 3],
) {
    path_state.points.push(PathPointRow { position, time_seconds: None });
    path_ops.0.push(PathOp::AppendPoint { track_node_id, position, time_seconds: None });
}

/// Removes point `index` from both the local buffer and queues the op.
/// No-op (with a warn) if `index` is out of range for the local buffer —
/// the op is still queued since the bridge is the source of truth for the
/// persisted list and may have a longer list than fe-ui's local echo.
pub(crate) fn remove_point(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    track_node_id: String,
    index: usize,
) {
    if index < path_state.points.len() {
        path_state.points.remove(index);
    } else {
        bevy::log::warn!("Paths: remove_point index {index} out of range for local buffer");
    }
    path_ops.0.push(PathOp::RemovePoint { track_node_id, index });
}

/// Moves point `index` to `position` in both the local buffer and the op
/// queue. Out-of-range index still queues the op (bridge is source of truth).
pub(crate) fn move_point(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    track_node_id: String,
    index: usize,
    position: [f32; 3],
) {
    if let Some(row) = path_state.points.get_mut(index) {
        row.position = position;
    } else {
        bevy::log::warn!("Paths: move_point index {index} out of range for local buffer");
    }
    path_ops.0.push(PathOp::MovePoint { track_node_id, index, position });
}

pub(crate) fn annotate_point(
    path_ops: &mut PendingPathOps,
    track_node_id: String,
    index: usize,
    title: String,
    body: String,
    color: String,
) {
    path_ops.0.push(PathOp::AnnotatePoint { track_node_id, index, title, body, color });
}

pub(crate) fn export_gpx(path_ops: &mut PendingPathOps, track_node_id: String) {
    path_ops.0.push(PathOp::ExportGpx { track_node_id });
}

/// track_styling_20260713 per-track style node-property keys. Kept as a fe-ui
/// local copy (fe-ui must NOT depend on fe-terrain — see
/// `fe-ui/src/verse_manager/AGENTS.md`); the canonical contract lives in
/// `fractalengine/src/gpx_bridge.rs`. Color persists as `#rrggbbaa` hex.
pub(crate) const TRACK_COLOR_KEY: &str = "gis.track.color";
pub(crate) const TRACK_WIDTH_KEY: &str = "gis.track.width";
pub(crate) const TRACK_VISIBLE_KEY: &str = "gis.track.visible";

/// Encode an sRGB `[f32; 4]` color as a `#rrggbbaa` hex string matching
/// `fe_terrain::iot::parse_track_color_hex`'s reader. Local to avoid an
/// fe-terrain dependency.
pub(crate) fn track_color_to_hex(color: [f32; 4]) -> String {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", c(color[0]), c(color[1]), c(color[2]), c(color[3]))
}

/// Pure builder: the `(key, value)` `SetNodeProperty` writes for a style edit.
/// Only the `Some` fields produce a write, so an untouched control doesn't
/// clobber a stored value. Testable without a `DbCommandSender`.
pub(crate) fn style_property_writes(
    color: Option<[f32; 4]>,
    width: Option<f32>,
    visible: Option<bool>,
) -> Vec<(&'static str, serde_json::Value)> {
    let mut writes = Vec::new();
    if let Some(color) = color {
        writes.push((TRACK_COLOR_KEY, serde_json::Value::String(track_color_to_hex(color))));
    }
    if let Some(width) = width {
        writes.push((TRACK_WIDTH_KEY, serde_json::json!(width)));
    }
    if let Some(visible) = visible {
        writes.push((TRACK_VISIBLE_KEY, serde_json::Value::Bool(visible)));
    }
    writes
}

/// Dispatches the `SetNodeProperty` writes for a per-track style edit directly
/// (mirrors `PathAssetApply`'s direct property write in `actions::mod`). The
/// gpx bridge's `NodePropertySet` arm re-reads the node and restyles the ribbon
/// live, so no `PathOp`/render round-trip is needed here.
pub(crate) fn set_style(
    db_sender: &DbCommandSender,
    track_node_id: String,
    color: Option<[f32; 4]>,
    width: Option<f32>,
    visible: Option<bool>,
) {
    for (key, value) in style_property_writes(color, width, visible) {
        if db_sender
            .0
            .send(DbCommand::SetNodeProperty { node_id: track_node_id.clone(), key: key.to_string(), value })
            .is_err()
        {
            bevy::log::warn!("db_sender channel closed — track-style SetNodeProperty not dispatched");
            return;
        }
    }
}

/// Replaces the currently-edited track's points with `new_points` via the
/// existing Remove/Append ops (clear-then-append), keeping the local buffer and
/// the op queue in lock-step. No-op (with a warn) if no track is being edited.
/// Pure enough to test through `PathEditorState` + `PendingPathOps` alone; the
/// gpx-bridge's `in_flight_points` path applies the ops without a schema change.
pub(crate) fn replace_points(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    new_points: Vec<[f32; 3]>,
) {
    let Some(track_node_id) = path_state.editing_track_id.clone() else {
        bevy::log::warn!("Paths: replace_points with no track being edited — ignored");
        return;
    };
    // Remove existing points high→low so each RemovePoint index stays valid.
    let existing = path_state.points.len();
    for index in (0..existing).rev() {
        remove_point(path_ops, path_state, track_node_id.clone(), index);
    }
    // Append the new list in order.
    for position in new_points {
        append_point(path_ops, path_state, track_node_id.clone(), position);
    }
}

/// Resamples the currently-edited track's control points via
/// `curve::resample` and replaces the track's points with the result. No-op if
/// no track is being edited or the track has `< 2` points (nothing to smooth).
pub(crate) fn smooth_current(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    mode: PenMode,
    tension: f32,
    samples_per_segment: usize,
) {
    if path_state.editing_track_id.is_none() || path_state.points.len() < 2 {
        return;
    }
    let control: Vec<[f32; 3]> = path_state.points.iter().map(|p| p.position).collect();
    let smoothed = curve::resample(&control, mode, tension, samples_per_segment);
    replace_points(path_ops, path_state, smoothed);
}

/// Appends pre-generated `shape_points` (from `curve::ellipse`/`circle`/…) to
/// the currently-edited track. No-op if no track is being edited.
pub(crate) fn append_shape(
    path_ops: &mut PendingPathOps,
    path_state: &mut PathEditorState,
    shape_points: Vec<[f32; 3]>,
) {
    let Some(track_node_id) = path_state.editing_track_id.clone() else {
        bevy::log::warn!("Paths: append_shape with no track being edited — ignored");
        return;
    };
    for position in shape_points {
        append_point(path_ops, path_state, track_node_id.clone(), position);
    }
}

/// Applies parsed track rows from a `DbResult::QueryResult` reply into
/// `PathEditorState.tracks` — pure enough to test via `gis::parse_gis_rows`
/// directly; this just assigns + clears the pending flag.
pub(crate) fn apply_track_rows(path_state: &mut PathEditorState, rows: Vec<crate::gis::GisResultRow>) {
    path_state.tracks = rows;
    path_state.tracks_pending = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_track_name_rejects_empty() {
        assert_eq!(validate_track_name(""), Err("Enter a name for the new path"));
        assert_eq!(validate_track_name("   "), Err("Enter a name for the new path"));
    }

    #[test]
    fn validate_track_name_trims() {
        assert_eq!(validate_track_name("  Ridge Loop  "), Ok("Ridge Loop".to_string()));
    }

    #[test]
    fn create_track_pushes_op_on_valid_name() {
        // Manual "New Path" button path: correlation_id None (bridge generates
        // its own id, unchanged behavior).
        let mut ops = PendingPathOps::default();
        let result = create_track(&mut ops, "petal-1".to_string(), "Ridge Loop".to_string(), None);
        assert!(result.is_ok());
        assert_eq!(ops.0.len(), 1);
        match &ops.0[0] {
            PathOp::CreateTrack { petal_id, name, correlation_id } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(name, "Ridge Loop");
                assert!(correlation_id.is_none(), "manual create carries no correlation id");
            }
            other => panic!("expected CreateTrack, got {other:?}"),
        }
    }

    #[test]
    fn create_track_threads_pen_correlation_id() {
        // Pen auto-create path: the fe-ui id rides through to the PathOp so the
        // bridge echoes it back and the deferred flush can match by it.
        let mut ops = PendingPathOps::default();
        let result = create_track(
            &mut ops,
            "petal-1".to_string(),
            "New Path".to_string(),
            Some("pen-track:5".to_string()),
        );
        assert!(result.is_ok());
        match &ops.0[0] {
            PathOp::CreateTrack { correlation_id, .. } => {
                assert_eq!(correlation_id.as_deref(), Some("pen-track:5"));
            }
            other => panic!("expected CreateTrack, got {other:?}"),
        }
    }

    #[test]
    fn create_track_rejects_empty_name_without_queuing() {
        let mut ops = PendingPathOps::default();
        let result = create_track(&mut ops, "petal-1".to_string(), "  ".to_string(), None);
        assert!(result.is_err());
        assert!(ops.0.is_empty());
    }

    #[test]
    fn append_point_updates_local_buffer_and_queues_op() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [1.0, 2.0, 3.0]);
        assert_eq!(state.points.len(), 1);
        assert_eq!(state.points[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(ops.0.len(), 1);
        match &ops.0[0] {
            PathOp::AppendPoint { track_node_id, position, time_seconds } => {
                assert_eq!(track_node_id, "track-1");
                assert_eq!(*position, [1.0, 2.0, 3.0]);
                assert!(time_seconds.is_none());
            }
            other => panic!("expected AppendPoint, got {other:?}"),
        }
    }

    #[test]
    fn remove_point_removes_from_local_buffer_and_queues_op() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [1.0, 0.0, 1.0]);
        append_point(&mut ops, &mut state, "track-1".to_string(), [2.0, 0.0, 2.0]);
        remove_point(&mut ops, &mut state, "track-1".to_string(), 0);
        assert_eq!(state.points.len(), 1);
        assert_eq!(state.points[0].position, [2.0, 0.0, 2.0]);
        assert_eq!(ops.0.len(), 3);
        assert!(matches!(ops.0[2], PathOp::RemovePoint { index: 0, .. }));
    }

    #[test]
    fn remove_point_out_of_range_still_queues_op() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        remove_point(&mut ops, &mut state, "track-1".to_string(), 5);
        assert!(state.points.is_empty());
        assert_eq!(ops.0.len(), 1);
        assert!(matches!(ops.0[0], PathOp::RemovePoint { index: 5, .. }));
    }

    #[test]
    fn move_point_updates_local_buffer_and_queues_op() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [1.0, 0.0, 1.0]);
        move_point(&mut ops, &mut state, "track-1".to_string(), 0, [5.0, 0.0, 5.0]);
        assert_eq!(state.points[0].position, [5.0, 0.0, 5.0]);
        assert_eq!(ops.0.len(), 2);
        match &ops.0[1] {
            PathOp::MovePoint { track_node_id, index, position } => {
                assert_eq!(track_node_id, "track-1");
                assert_eq!(*index, 0);
                assert_eq!(*position, [5.0, 0.0, 5.0]);
            }
            other => panic!("expected MovePoint, got {other:?}"),
        }
    }

    #[test]
    fn move_point_out_of_range_still_queues_op() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        move_point(&mut ops, &mut state, "track-1".to_string(), 9, [5.0, 0.0, 5.0]);
        assert!(state.points.is_empty());
        assert_eq!(ops.0.len(), 1);
        assert!(matches!(ops.0[0], PathOp::MovePoint { index: 9, .. }));
    }

    #[test]
    fn export_gpx_queues_op() {
        let mut ops = PendingPathOps::default();
        export_gpx(&mut ops, "track-1".to_string());
        assert_eq!(ops.0.len(), 1);
        assert!(matches!(ops.0[0], PathOp::ExportGpx { .. }));
    }

    #[test]
    fn annotate_point_queues_op_with_fields() {
        let mut ops = PendingPathOps::default();
        annotate_point(
            &mut ops,
            "track-1".to_string(),
            2,
            "Camp".to_string(),
            "Overnight stop".to_string(),
            "#00ff00".to_string(),
        );
        assert_eq!(ops.0.len(), 1);
        match &ops.0[0] {
            PathOp::AnnotatePoint { track_node_id, index, title, body, color } => {
                assert_eq!(track_node_id, "track-1");
                assert_eq!(*index, 2);
                assert_eq!(title, "Camp");
                assert_eq!(body, "Overnight stop");
                assert_eq!(color, "#00ff00");
            }
            other => panic!("expected AnnotatePoint, got {other:?}"),
        }
    }

    #[test]
    fn delete_track_queues_op() {
        let mut ops = PendingPathOps::default();
        delete_track(&mut ops, "track-1".to_string());
        assert_eq!(ops.0.len(), 1);
        assert!(matches!(ops.0[0], PathOp::DeleteTrack { .. }));
    }

    #[test]
    fn track_color_to_hex_emits_rrggbbaa() {
        assert_eq!(track_color_to_hex([1.0, 0.0, 0.0, 1.0]), "#ff0000ff");
        assert_eq!(track_color_to_hex([0.0, 0.8, 1.0, 1.0]), "#00ccffff");
        // Clamps out-of-range channels.
        assert_eq!(track_color_to_hex([2.0, -1.0, 0.5019608, 1.0]), "#ff0080ff");
    }

    #[test]
    fn style_property_writes_only_includes_changed_fields() {
        // Only the width is set → exactly one write, for the width key.
        let writes = style_property_writes(None, Some(3.5), None);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, TRACK_WIDTH_KEY);
        assert_eq!(writes[0].1, serde_json::json!(3.5));
    }

    #[test]
    fn style_property_writes_maps_all_three_keys_and_types() {
        let writes = style_property_writes(Some([0.0, 0.8, 1.0, 1.0]), Some(4.0), Some(false));
        assert_eq!(writes.len(), 3);
        // Color → hex string.
        assert_eq!(writes[0], (TRACK_COLOR_KEY, serde_json::Value::String("#00ccffff".to_string())));
        // Width → number.
        assert_eq!(writes[1], (TRACK_WIDTH_KEY, serde_json::json!(4.0)));
        // Visible → bool.
        assert_eq!(writes[2], (TRACK_VISIBLE_KEY, serde_json::Value::Bool(false)));
    }

    #[test]
    fn style_property_writes_empty_when_nothing_changed() {
        assert!(style_property_writes(None, None, None).is_empty());
    }

    #[test]
    fn set_style_dispatches_one_command_per_changed_field() {
        let (tx, rx) = crossbeam::channel::bounded(8);
        let db_sender = DbCommandSender(tx);
        set_style(&db_sender, "track-1".to_string(), Some([1.0, 0.0, 0.0, 1.0]), Some(2.0), None);
        // Two writes: color + width (visible is None).
        let mut keys = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                DbCommand::SetNodeProperty { node_id, key, .. } => {
                    assert_eq!(node_id, "track-1");
                    keys.push(key);
                }
                other => panic!("expected SetNodeProperty, got {other:?}"),
            }
        }
        assert_eq!(keys, vec![TRACK_COLOR_KEY.to_string(), TRACK_WIDTH_KEY.to_string()]);
    }

    #[test]
    fn replace_points_clears_then_appends_and_syncs_buffer() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [0.0, 0.0, 0.0]);
        append_point(&mut ops, &mut state, "track-1".to_string(), [1.0, 0.0, 1.0]);
        ops.0.clear(); // isolate the replace ops below
        replace_points(&mut ops, &mut state, vec![[5.0, 0.0, 5.0], [6.0, 0.0, 6.0], [7.0, 0.0, 7.0]]);
        // Local buffer reflects the new list exactly.
        assert_eq!(state.points.len(), 3);
        assert_eq!(state.points[0].position, [5.0, 0.0, 5.0]);
        assert_eq!(state.points[2].position, [7.0, 0.0, 7.0]);
        // 2 removes (high→low) + 3 appends.
        assert_eq!(ops.0.len(), 5);
        assert!(matches!(ops.0[0], PathOp::RemovePoint { index: 1, .. }));
        assert!(matches!(ops.0[1], PathOp::RemovePoint { index: 0, .. }));
        assert!(matches!(ops.0[2], PathOp::AppendPoint { .. }));
    }

    #[test]
    fn replace_points_no_track_is_noop() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        replace_points(&mut ops, &mut state, vec![[1.0, 0.0, 1.0]]);
        assert!(ops.0.is_empty());
        assert!(state.points.is_empty());
    }

    #[test]
    fn smooth_current_resamples_and_replaces() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [0.0, 0.0, 0.0]);
        append_point(&mut ops, &mut state, "track-1".to_string(), [2.0, 0.0, 0.0]);
        smooth_current(&mut ops, &mut state, PenMode::CatmullRom, 0.5, 4);
        // Catmull with 1 segment * 4 samples + 1 start = 5 resampled points.
        assert_eq!(state.points.len(), 5);
        // First and last preserved (curve passes through control endpoints).
        assert_eq!(state.points[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(state.points.last().unwrap().position, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn smooth_current_polyline_is_identity_length() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [0.0, 0.0, 0.0]);
        append_point(&mut ops, &mut state, "track-1".to_string(), [1.0, 0.0, 1.0]);
        append_point(&mut ops, &mut state, "track-1".to_string(), [2.0, 0.0, 0.0]);
        smooth_current(&mut ops, &mut state, PenMode::Polyline, 0.5, 8);
        assert_eq!(state.points.len(), 3, "polyline resample preserves point count");
    }

    #[test]
    fn smooth_current_under_two_points_is_noop() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        append_point(&mut ops, &mut state, "track-1".to_string(), [0.0, 0.0, 0.0]);
        ops.0.clear();
        smooth_current(&mut ops, &mut state, PenMode::CatmullRom, 0.5, 8);
        assert_eq!(state.points.len(), 1, "one point can't be smoothed");
        assert!(ops.0.is_empty());
    }

    #[test]
    fn append_shape_appends_all_points() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        state.start_editing("track-1".to_string());
        let ring = curve::circle([0.0, 0.0, 0.0], 3.0, 8);
        append_shape(&mut ops, &mut state, ring.clone());
        assert_eq!(state.points.len(), 8);
        assert_eq!(ops.0.len(), 8);
        assert_eq!(state.points[0].position, ring[0]);
    }

    #[test]
    fn append_shape_no_track_is_noop() {
        let mut ops = PendingPathOps::default();
        let mut state = PathEditorState::default();
        append_shape(&mut ops, &mut state, vec![[1.0, 0.0, 1.0]]);
        assert!(ops.0.is_empty());
        assert!(state.points.is_empty());
    }

    #[test]
    fn select_track_starts_editing_and_sets_points_pending() {
        let (tx, rx) = crossbeam::channel::bounded(8);
        let db_sender = DbCommandSender(tx);
        let mut state = PathEditorState::default();
        state.points.push(PathPointRow { position: [9.0, 0.0, 9.0], time_seconds: None });
        select_track(&db_sender, &mut state, "track-1".to_string());
        assert_eq!(state.editing_track_id.as_deref(), Some("track-1"));
        assert!(state.points.is_empty(), "start_editing clears the stale local buffer");
        assert!(state.points_pending);
        match rx.try_recv() {
            Ok(DbCommand::GetNodeProperties { node_id }) => assert_eq!(node_id, "track-1"),
            other => panic!("expected GetNodeProperties, got {other:?}"),
        }
    }
}
