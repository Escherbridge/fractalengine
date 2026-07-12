//! Path-editor action handling: pure decision helpers + queueing into
//! `PendingPathOps`. fe-ui does no persistence I/O — the main binary's GPX
//! bridge drains the queue. Mirrors `actions::gpx`'s queue-for-main-binary
//! pattern and `actions::gis`'s `RawQuery` submission for track listing.
//! See `fe-ui/src/AGENTS.md` §path-editor.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::gis::{self, PathEditorState, PathPointRow};
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

/// Queues `CreateTrack` and clears the name buffer. Pure validation split
/// out as `validate_track_name` so it's testable without a queue.
pub(crate) fn create_track(path_ops: &mut PendingPathOps, petal_id: String, name: String) -> Result<(), &'static str> {
    let name = validate_track_name(&name)?;
    path_ops.0.push(PathOp::CreateTrack { petal_id, name });
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
        let mut ops = PendingPathOps::default();
        let result = create_track(&mut ops, "petal-1".to_string(), "Ridge Loop".to_string());
        assert!(result.is_ok());
        assert_eq!(ops.0.len(), 1);
        match &ops.0[0] {
            PathOp::CreateTrack { petal_id, name } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(name, "Ridge Loop");
            }
            other => panic!("expected CreateTrack, got {other:?}"),
        }
    }

    #[test]
    fn create_track_rejects_empty_name_without_queuing() {
        let mut ops = PendingPathOps::default();
        let result = create_track(&mut ops, "petal-1".to_string(), "  ".to_string());
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
}
