//! Handlers for GUI `RawQuery` replies (routed by correlation id) and DB
//! errors; see ../AGENTS.md §db-results and fe-ui/src/AGENTS.md §gis-query-ui.

use crate::gis::{GisPanelState, PathEditorState};
use crate::plugin::InspectorFormState;

/// Correlated `QueryResult`/`QueryFailed`: land the reply ONLY in the buffer
/// whose stored request id equals `id`, then clear that id + its pending flag.
/// Returns `false` when no GUI buffer claims it (an API reply, or a stale id
/// after the user re-submitted) so the caller forwards it to `try_deliver`.
pub(super) fn route_query_reply(
    id: &str,
    reply: Result<&[serde_json::Value], &str>,
    gis_panel: &mut GisPanelState,
    path_state: &mut PathEditorState,
    inspector: &mut InspectorFormState,
) -> bool {
    if gis_panel.query_request_id.as_deref() == Some(id) {
        gis_panel.query_request_id = None;
        gis_panel.query_pending = false;
        match reply {
            Ok(data) => gis_panel.results = crate::gis::parse_gis_rows(data),
            Err(error) => gis_panel.last_error = Some(error.to_string()),
        }
    } else if path_state.tracks_request_id.as_deref() == Some(id) {
        match reply {
            Ok(data) => {
                crate::actions::path::apply_track_rows(path_state, crate::gis::parse_gis_rows(data))
            }
            Err(error) => {
                path_state.tracks_request_id = None;
                path_state.tracks_pending = false;
                path_state.last_error = Some(error.to_string());
            }
        }
    } else if inspector.query_request_id.as_deref() == Some(id) {
        inspector.query_request_id = None;
        inspector.query_loading = false;
        inspector.query_result = Some(match reply {
            Ok(data) => {
                serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("Format error: {e}"))
            }
            Err(error) => format!("Error: {error}"),
        });
    } else {
        return false;
    }
    true
}

/// Legacy uncorrelated `QueryResult` (no current GUI sender): flag-priority
/// claim — GIS panel > Paths tab > ad-hoc Query tab.
pub(super) fn handle_query_result(
    data: &[serde_json::Value],
    gis_panel: &mut GisPanelState,
    path_state: &mut PathEditorState,
    inspector: &mut InspectorFormState,
) {
    if gis_panel.query_pending {
        gis_panel.results = crate::gis::parse_gis_rows(data);
        gis_panel.query_pending = false;
    } else if path_state.tracks_pending {
        crate::actions::path::apply_track_rows(path_state, crate::gis::parse_gis_rows(data));
    } else {
        let formatted =
            serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("Format error: {e}"));
        inspector.query_result = Some(formatted);
        inspector.query_loading = false;
    }
}

/// `Error`: log and clear a stranded pen create. Query buffers never take a
/// bare `Error` — their failures arrive correlated as `QueryFailed`.
pub(super) fn handle_error(msg: &str, path_state: &mut PathEditorState) {
    bevy::log::error!("DB error: {msg}");
    // HIGH-2: `DbResult::Error` carries only a message — no node id
    // or correlation id — so a failed `PathCreateTrack` (the DB
    // emits `Error`, never `NodeCreated`) can't be matched precisely.
    // A stranded `pending_pen_create` would leave the pen
    // permanently dead AND let a much-later unrelated create's echo
    // (if ids ever collided) flush onto the wrong node, so clear it
    // best-effort on ANY error while a pen create is in flight: a
    // failed create is the likely cause and leaving it stuck is
    // worse. The next Pen click simply starts a fresh auto-create.
    if path_state.clear_pending_pen_create() {
        bevy::log::warn!(
            "Pen auto-create likely failed (DB error while a pen create was pending) — \
             cleared pending pen state; click again to retry: {msg}"
        );
    }
}
