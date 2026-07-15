//! Handlers for the shared untagged raw-query result channel and DB errors.
//! GIS panel > Paths tab > ad-hoc Query tab claim priority; see ../AGENTS.md §db-results.

use crate::gis::{GisPanelState, PathEditorState};
use crate::plugin::InspectorFormState;

/// `QueryResult`: route to whichever caller is actually waiting.
pub(super) fn handle_query_result(
    data: &[serde_json::Value],
    gis_panel: &mut GisPanelState,
    path_state: &mut PathEditorState,
    inspector: &mut InspectorFormState,
) {
    // `RawQuery`'s result carries no request-id to correlate against,
    // so whichever caller is actually waiting claims it: the GIS
    // panel's queries take priority since they're the newer/rarer
    // caller — the ad-hoc Query tab falls back to its own buffer.
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

/// `Error`: log, clear a stranded pen create, and route to the pending caller.
pub(super) fn handle_error(
    msg: &str,
    gis_panel: &mut GisPanelState,
    path_state: &mut PathEditorState,
    inspector: &mut InspectorFormState,
) {
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
    // GIS panel queries take priority over the ad-hoc Query tab
    // when both happen to be in flight — see `handle_query_result`
    // for why these two share one untagged result channel.
    if gis_panel.query_pending {
        gis_panel.last_error = Some(msg.to_string());
        gis_panel.query_pending = false;
    } else if path_state.tracks_pending {
        path_state.last_error = Some(msg.to_string());
        path_state.tracks_pending = false;
    } else if inspector.query_loading {
        inspector.query_result = Some(format!("Error: {msg}"));
        inspector.query_loading = false;
    }
}
