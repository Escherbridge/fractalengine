//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").
//!
//! Also homes the Wave-1 contextual-controls (T4) verb handler stubs
//! (delete/duplicate/rename/promote/copy-API/report) — see the Wave-1
//! registration scaffold in `actions/mod.rs`. T4 fills these; delete/cascade/
//! promote route through T1's sync-safe ops (N-4). T4 edits no central file.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand};

use crate::actions::asset::StampInteractionState;
use crate::actions::UiManager;

/// Send `CreateNode` for an empty node at `position` in `petal_id`.
pub(crate) fn create_at(db_sender: &DbCommandSender, petal_id: String, position: [f32; 3]) {
    if db_sender
        .0
        .send(DbCommand::CreateNode {
            petal_id,
            name: "Empty Node".to_string(),
            position,
            correlation_id: None,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — CreateNode not sent");
    }
}

/// T4 FR-2 — delete an object via the spine's sync-safe tombstone; `cascade`
/// routes parent deletes through `CascadeTombstoneNode` (subtree, atomic). This
/// is the REAL remove-a-node path that fixes the "empty husk" bug — clearing
/// properties (see `node_props::handle_clear`) is a distinct verb that keeps the
/// node. Auth is `CallerAuth::Local`: the UI asserts NO role; the DB thread
/// resolves the local session's role and enforces Editor+ (N-5, N-4).
pub(crate) fn handle_delete(db_sender: &DbCommandSender, node_id: String, cascade: bool) {
    let cmd = if cascade {
        DbCommand::CascadeTombstoneNode {
            node_id,
            auth: CallerAuth::Local,
        }
    } else {
        DbCommand::TombstoneNode {
            node_id,
            auth: CallerAuth::Local,
        }
    };
    if db_sender.0.send(cmd).is_err() {
        bevy::log::warn!("db_sender channel closed — tombstone delete not sent");
    }
}

/// T4 FR-3 — duplicate a node. The action-routed entry: a faithful duplicate
/// needs the source node's petal/name/position (and ideally a spine
/// `DuplicateNode` op, which does not exist yet), none of which is derivable
/// from `node_id` alone here. The functional duplicate is therefore driven from
/// the Node Options surface, which has the hierarchy to resolve that data (see
/// `dialogs/node_options.rs`). Logged so an action-routed duplicate is never a
/// silent no-op (ui_ux §6). Open item: a `DuplicateNode` spine op / a
/// data-carrying payload would let this route directly.
pub(crate) fn handle_duplicate(_db_sender: &DbCommandSender, node_id: String) {
    bevy::log::warn!(
        "duplicate for node {node_id} arrived via the action seam; drive it from a \
         surface that carries the node's petal/name/position (contextual_controls FR-3)"
    );
}

/// T4 FR-3 — rename a node. The spine currently exposes only `RenameEntity`
/// (verse/fractal/petal); there is no scene-node rename op, so this cannot be
/// persisted yet. The verb is shown disabled-with-hint on its surfaces; this
/// logs (never a silent no-op) until a node-rename spine op lands. Open item.
pub(crate) fn handle_rename(_db_sender: &DbCommandSender, node_id: String, name: String) {
    bevy::log::warn!(
        "rename node {node_id} -> '{name}': no scene-node rename spine op exists yet \
         (RenameEntity covers only verse/fractal/petal) — contextual_controls FR-3 open item"
    );
}

/// T4 FR-3 / T2 FR-5 — promote an un-promoted stamp to a full addressable node
/// via the spine's `PromoteInstance` (T1 FR-5). That op needs `petal_id`
/// alongside `path_id` (= `track_node_id`) and `instance_index` (= `stamp_index`);
/// `petal_id` lives in T2's `StampInteractionState` (owner of stamp selection),
/// which is empty until T2 fills it. Logged so the verb is never a silent
/// no-op; T2 wires the `petal_id` resolution when it populates that state.
pub(crate) fn handle_promote_stamp(
    _db_sender: &DbCommandSender,
    _stamp_state: &mut StampInteractionState,
    track_node_id: String,
    stamp_index: usize,
) {
    bevy::log::warn!(
        "promote stamp {stamp_index} on path {track_node_id}: awaiting T2 \
         StampInteractionState.petal_id to emit PromoteInstance (T1 FR-5)"
    );
}

/// T4 FR-4 — copy the object's public API/egress string. Calls the
/// `endpoint_api_surface` (T5) seam and reports the outcome as a toast; the
/// clipboard write itself happens on the render surface (only egui's `ctx` can
/// touch the clipboard — no clipboard dep, `ui_ux` §egress). `None` (seam
/// absent / no endpoint for this object) surfaces a hint, never a silent
/// failure (ui_ux §6). Lights up automatically when T5's seam yields `Some`.
pub(crate) fn handle_copy_api(ui_mgr: &mut UiManager, node_id: String, now_secs: f64) {
    match crate::gis::egress_strings::api_string_for(&node_id) {
        Some(_) => ui_mgr.show_toast("API string copied to clipboard", now_secs),
        None => ui_mgr.show_toast("API endpoint not available yet", now_secs),
    }
}

/// T4 FR-4 — open/produce the object's report. Calls the `endpoint_api_surface`
/// (T5) `report_for` seam and reports the outcome as a toast (the report text is
/// placed on the clipboard by the render surface, mirroring copy-API). `None`
/// surfaces a hint, never a silent failure (ui_ux §6).
pub(crate) fn handle_report(ui_mgr: &mut UiManager, node_id: String, now_secs: f64) {
    match crate::gis::egress_strings::report_for(&node_id) {
        Some(_) => ui_mgr.show_toast("Report copied to clipboard", now_secs),
        None => ui_mgr.show_toast("Report not available yet", now_secs),
    }
}
