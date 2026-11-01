//! Handlers for GUI `RawQuery` replies (routed by correlation id) and DB
//! errors; see ../AGENTS.md §db-results and fe-ui/src/AGENTS.md §gis-query-ui.

use crate::actions::UiManager;
use crate::gis::{GisPanelState, PathEditorState};
use crate::navigation_manager::NavigationManager;
use crate::notice::{active_verse_name, humanize_db_error, DenialContext};
use crate::plugin::{InspectorFormState, LocalUserRole};

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
    } else if inspector.query_results.request_id.as_deref() == Some(id) {
        match reply {
            Ok(data) => inspector.query_results.complete(data),
            Err(error) => inspector.query_results.fail(error),
        }
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
        // Uncorrelated replies never claim a correlated dock run.
        if !inspector.query_results.pending() { inspector.query_results.complete(data); }
    }
}

/// `Error`: log, clear a stranded pen create, and surface a humanized error
/// notice + status-bar history entry (X-01) — every bare `DbResult::Error`
/// reaches the user now instead of only the log, since it carries no
/// correlation id to route anywhere more specific.
pub(super) fn handle_error(
    msg: &str,
    path_state: &mut PathEditorState,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    nav: &NavigationManager,
    now: f64,
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
    let ctx = DenialContext::new(local_role, active_verse_name(nav));
    // D-06: a failed import has no typed reply, so its prefix is what lets
    // the Import dialog that asked unlock and say why, like Join does.
    if msg.starts_with(GLTF_IMPORT_FAILED_PREFIX) && ui_mgr.gltf_import_request.finish() {
        ui_mgr.gltf_import_request.error = Some(humanize_db_error(msg, &ctx));
        return;
    }
    ui_mgr.show_db_error(msg, &ctx, now);
}

/// Prefix of the uncorrelated `DbResult::Error` an `ImportGltf` failure
/// replies with (`fe-database/src/lib.rs`, the `ImportGltf` arm).
const GLTF_IMPORT_FAILED_PREFIX: &str = "GLTF import failed";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::ActiveDialog;

    /// D-06: the Import dialog's own failed import unlocks it with the reason
    /// inline; any other error leaves its request in flight and goes to the
    /// general notice instead.
    #[test]
    fn gltf_import_failure_unlocks_only_the_dialog_that_asked() {
        let cases = [
            ("GLTF import failed: file not found: a.glb", false, true),
            ("Load hierarchy failed: disk full", true, false),
        ];
        for (raw, still_pending, inline) in cases {
            let mut ui_mgr = UiManager::default();
            ui_mgr.open_dialog(ActiveDialog::GltfImport {
                file_path_buf: "a.glb".into(),
                name_buf: String::new(),
                position: [0.0; 3],
            });
            ui_mgr.gltf_import_request.start(1.0);
            handle_error(
                raw,
                &mut PathEditorState::default(),
                &mut ui_mgr,
                &LocalUserRole::default(),
                &NavigationManager::default(),
                2.0,
            );
            assert_eq!(ui_mgr.gltf_import_request.pending, still_pending, "{raw}");
            assert_eq!(
                ui_mgr
                    .gltf_import_request
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("file not found")),
                inline,
                "{raw}"
            );
            assert_eq!(ui_mgr.latest_unacknowledged_error().is_some(), !inline, "{raw}");
            assert!(matches!(ui_mgr.active_dialog, ActiveDialog::GltfImport { .. }));
        }
    }
}
