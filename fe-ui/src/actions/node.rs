//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").
//!
//! Also homes the Wave-1 contextual-controls (T4) verb handler stubs
//! (delete/duplicate/rename/promote/copy-API/report) — see the Wave-1
//! registration scaffold in `actions/mod.rs`. T4 fills these; delete/cascade/
//! promote route through T1's sync-safe ops (N-4). T4 edits no central file.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

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

/// Wave 1: T4 — delete an object via T1's sync-safe tombstone; `cascade`
/// routes parent deletes through T1's cascade+confirm (T4 FR-2). Fixes the
/// husk bug. Filled by T4 once T1's delete op-log variant lands.
pub(crate) fn handle_delete(_db_sender: &DbCommandSender, _node_id: String, _cascade: bool) {
    // Wave 1: T4 contextual_controls fills this (wire to T1 tombstone delete).
}

/// Wave 1: T4 — duplicate an object (T4 FR-3).
pub(crate) fn handle_duplicate(_db_sender: &DbCommandSender, _node_id: String) {
    // Wave 1: T4 contextual_controls fills this.
}

/// Wave 1: T4 — rename an object (T4 FR-3).
pub(crate) fn handle_rename(_db_sender: &DbCommandSender, _node_id: String, _name: String) {
    // Wave 1: T4 contextual_controls fills this.
}

/// Wave 1: T4/T2 — promote an un-promoted stamp to a full node via T1 FR-5
/// (T4 FR-3 / T2 FR-5).
pub(crate) fn handle_promote_stamp(
    _db_sender: &DbCommandSender,
    _stamp_state: &mut StampInteractionState,
    _track_node_id: String,
    _stamp_index: usize,
) {
    // Wave 1: T4/T2 fills this (T1 lazy-promotion).
}

/// Wave 1 / needs T5: copy the object's public API/egress string. No-op until
/// `endpoint_api_surface` (T5) lands the address→string seam; T4 shows the verb
/// disabled-with-hint until then (T4 FR-4, ui_ux §6).
pub(crate) fn handle_copy_api(_ui_mgr: &mut UiManager, _node_id: String, _now_secs: f64) {
    // Wave 1 / needs T5.
}

/// Wave 1 / needs T5: open the object's report/query view. No-op until T5's
/// report seam lands; T4 shows the verb disabled-with-hint until then.
pub(crate) fn handle_report(_ui_mgr: &mut UiManager, _node_id: String, _now_secs: f64) {
    // Wave 1 / needs T5.
}
