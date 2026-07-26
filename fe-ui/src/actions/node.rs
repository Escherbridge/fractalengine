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

/// T4 FR-3 — duplicate a node via the spine's `DuplicateNode` (full row copy,
/// " (copy)" name, +1m x/z, path-binding keys stripped — fe-database owns the
/// semantics). Replies with `NodeCreated`, which fe-ui already applies. Auth is
/// `CallerAuth::Local` (N-5).
pub(crate) fn handle_duplicate(db_sender: &DbCommandSender, node_id: String) {
    if db_sender
        .0
        .send(DbCommand::DuplicateNode {
            node_id,
            auth: CallerAuth::Local,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — DuplicateNode not sent");
    }
}

/// T4 FR-3 — rename a node's `display_name` via the spine's `RenameNode`.
/// Empty names are refused loudly (N-8); the tree updates on the `NodeRenamed`
/// result, never optimistically. Auth is `CallerAuth::Local` (N-5).
pub(crate) fn handle_rename(db_sender: &DbCommandSender, node_id: String, name: String) {
    let new_name = name.trim().to_string();
    if new_name.is_empty() {
        bevy::log::warn!("rename node {node_id} skipped: empty name (N-8)");
        return;
    }
    if db_sender
        .0
        .send(DbCommand::RenameNode {
            node_id,
            new_name,
            auth: CallerAuth::Local,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — RenameNode not sent");
    }
}

/// T4 FR-3 / T2 FR-5 — promote an un-promoted stamp to a full addressable node
/// via the spine's `PromoteInstance` (T1 FR-5). `petal_id` is the ACTIVE petal
/// (stamps only materialize there — the dispatcher threads
/// `NavigationManager.active_petal_id`); a missing petal warns, never a silent
/// no-op (N-8). Auth is `CallerAuth::Local` — the UI asserts no role (N-5).
/// Already-promoted stamps are skipped (idempotent, N-9).
pub(crate) fn handle_promote_stamp(
    db_sender: &DbCommandSender,
    stamp_state: &mut StampInteractionState,
    active_petal_id: Option<&str>,
    track_node_id: String,
    stamp_index: usize,
) {
    let stamp = crate::actions::asset::StampRef {
        track_node_id,
        stamp_index,
    };
    if stamp_state.is_promoted(&stamp) {
        return; // already a full node — nothing to send (N-9)
    }
    let Some(petal_id) = active_petal_id else {
        bevy::log::warn!(
            "promote stamp {}::{} skipped: no active petal to scope PromoteInstance (N-8)",
            stamp.track_node_id,
            stamp.stamp_index
        );
        return;
    };
    if db_sender
        .0
        .send(DbCommand::PromoteInstance {
            petal_id: petal_id.to_string(),
            path_id: stamp.track_node_id.clone(),
            instance_index: stamp.stamp_index as u32,
            auth: CallerAuth::Local,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — PromoteInstance not sent");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> (DbCommandSender, crossbeam::channel::Receiver<DbCommand>) {
        let (tx, rx) = crossbeam::channel::bounded(8);
        (DbCommandSender(tx), rx)
    }

    #[test]
    fn duplicate_routes_the_spine_op_with_local_auth() {
        let (tx, rx) = sender();
        handle_duplicate(&tx, "n1".into());
        match rx.try_recv() {
            Ok(DbCommand::DuplicateNode { node_id, auth }) => {
                assert_eq!(node_id, "n1");
                assert!(matches!(auth, CallerAuth::Local), "N-5: UI asserts no role");
            }
            other => panic!("expected DuplicateNode, got {other:?}"),
        }
    }

    #[test]
    fn rename_routes_the_spine_op_trimmed_with_local_auth() {
        let (tx, rx) = sender();
        handle_rename(&tx, "n1".into(), "  New Name  ".into());
        match rx.try_recv() {
            Ok(DbCommand::RenameNode {
                node_id,
                new_name,
                auth,
            }) => {
                assert_eq!(node_id, "n1");
                assert_eq!(new_name, "New Name");
                assert!(matches!(auth, CallerAuth::Local));
            }
            other => panic!("expected RenameNode, got {other:?}"),
        }
    }

    #[test]
    fn rename_refuses_an_empty_name_without_sending() {
        let (tx, rx) = sender();
        handle_rename(&tx, "n1".into(), "   ".into());
        assert!(rx.try_recv().is_err(), "empty rename must not reach the DB");
    }
}
