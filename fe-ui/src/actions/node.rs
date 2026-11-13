//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").
//!
//! Also homes the contextual-controls verb handlers (delete/duplicate/rename/copy-API/report)
//! and the GUI scene write door (`submit_scene_write`; see `actions/AGENTS.md` §scene-write-door).

use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use crossbeam::channel::Sender;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand};
use fe_runtime::scene_hook_gate::SceneCallerKind;
use fe_runtime::scene_write::{
    SceneWrite, SceneWriteDoor, SceneWriteError, SceneWriteOutcome, SceneWriteTicket,
};

use crate::actions::UiManager;

/// How the door disposed of one GUI write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiSend {
    /// Sent to the DB thread.
    Sent,
    /// A bound extension is still answering; the frame drain sends or refuses it.
    Waiting,
    /// The gate refused; the advisory text is queued for the player.
    Refused(String),
    /// The DB channel is closed.
    Closed,
}

struct WaitingWrite {
    ticket: SceneWriteTicket,
    sender: Sender<DbCommand>,
}

struct GuiDoorQueue {
    active_petal: Option<String>,
    waiting: Vec<WaitingWrite>,
    refusals: Vec<String>,
}

static GUI_DOOR: Mutex<GuiDoorQueue> = Mutex::new(GuiDoorQueue {
    active_petal: None,
    waiting: Vec::new(),
    refusals: Vec::new(),
});

fn door_queue() -> MutexGuard<'static, GuiDoorQueue> {
    GUI_DOOR.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The petal a command names itself; `None` means the active petal authorizes it.
fn carried_petal(command: &DbCommand) -> Option<String> {
    match command {
        DbCommand::CreateNode { petal_id, .. }
        | DbCommand::CreateNodeWithAsset { petal_id, .. }
        | DbCommand::ImportGltf { petal_id, .. }
        | DbCommand::CreateObject { petal_id, .. }
        | DbCommand::PromoteInstance { petal_id, .. } => Some(petal_id.clone()),
        DbCommand::PromoteCheckedStamp { capture, .. } => Some(capture.petal_id.clone()),
        DbCommand::CommitCheckedEditorBatch { request, .. } => {
            Some(request.capture.petal_id.clone())
        }
        _ => None,
    }
}

/// Submit one scene write through `door` as caller `Ui`; admitted writes go to `sender`.
fn submit_with(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    command: DbCommand,
    blocking: bool,
) -> GuiSend {
    let petal = carried_petal(&command).or_else(|| door_queue().active_petal.clone());
    let Some(petal) = petal else {
        // No petal to authorize against: nothing bound can resolve, so the write is plain.
        return send_plain(sender, command, blocking);
    };
    let write = match SceneWrite::new(petal, command.clone()) {
        Ok(write) => write,
        Err(SceneWriteError::NotASceneWrite) => return send_plain(sender, command, blocking),
        Err(error) => {
            let message = format!("Edit was not saved: {error}");
            door_queue().refusals.push(message.clone());
            return GuiSend::Refused(message);
        }
    };
    match door.submit(SceneCallerKind::Ui, write).poll(Instant::now()) {
        Ok(outcome) => settle(outcome, sender, blocking),
        Err(ticket) => {
            door_queue().waiting.push(WaitingWrite {
                ticket,
                sender: sender.clone(),
            });
            GuiSend::Waiting
        }
    }
}

fn send_plain(sender: &Sender<DbCommand>, command: DbCommand, blocking: bool) -> GuiSend {
    let sent = if blocking {
        sender.send(command).is_ok()
    } else {
        sender.try_send(command).is_ok()
    };
    if sent { GuiSend::Sent } else { GuiSend::Closed }
}

/// Send an admitted write, or queue the refusal advisory.
fn settle(outcome: SceneWriteOutcome, sender: &Sender<DbCommand>, blocking: bool) -> GuiSend {
    match outcome {
        SceneWriteOutcome::Admitted(admitted) => {
            let (command, augments) = admitted.into_parts();
            let sent = send_plain(sender, command, blocking);
            if sent == GuiSend::Sent {
                // A create's node id is unknown here, so a create augment is dropped with a warning.
                for error in augments.stage(None) {
                    bevy::log::warn!(%error, "scene hook augment was not staged");
                }
            }
            sent
        }
        SceneWriteOutcome::Refused(refusal) => {
            let message = refusal.message();
            door_queue().refusals.push(message.clone());
            GuiSend::Refused(message)
        }
    }
}

/// Submit through the installed process gate; blocks like `Sender::send` once admitted.
pub fn submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), &db_sender.0, command, true)
}

/// As [`submit_scene_write`] for frame systems that must not block on a full channel.
pub fn try_submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), &db_sender.0, command, false)
}

/// As [`try_submit_scene_write`] for doors that hold the raw sender (dialogs).
pub fn try_submit_scene_write_on(sender: &Sender<DbCommand>, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), sender, command, false)
}

/// Frame drain: track the active petal, settle waiting writes, show refusals as advisories.
pub fn drain_gui_scene_writes(
    nav: bevy::prelude::Res<crate::navigation_manager::NavigationManager>,
    clock: bevy::prelude::Res<crate::notice::NoticeClock>,
    mut ui_mgr: bevy::prelude::ResMut<UiManager>,
) {
    let settled = {
        let mut queue = door_queue();
        queue.active_petal = nav.active_petal_id.clone();
        let mut settled = Vec::new();
        for waiting in std::mem::take(&mut queue.waiting) {
            let WaitingWrite { ticket, sender } = waiting;
            match ticket.poll(Instant::now()) {
                Ok(outcome) => settled.push((outcome, sender)),
                Err(ticket) => queue.waiting.push(WaitingWrite { ticket, sender }),
            }
        }
        settled
    };
    for (outcome, sender) in settled {
        settle(outcome, &sender, false);
    }
    let refusals = std::mem::take(&mut door_queue().refusals);
    for message in refusals {
        ui_mgr.show_advisory(message, clock.elapsed_secs_f64());
    }
}

/// Send `CreateNode` for an empty node at `position` in `petal_id`.
pub(crate) fn create_at(db_sender: &DbCommandSender, petal_id: String, position: [f32; 3]) {
    let command = DbCommand::CreateNode {
        petal_id,
        name: "Empty Node".to_string(),
        position,
        correlation_id: None,
    };
    if submit_scene_write(db_sender, command) == GuiSend::Closed {
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
    if submit_scene_write(db_sender, cmd) == GuiSend::Closed {
        bevy::log::warn!("db_sender channel closed — tombstone delete not sent");
    }
}

/// T4 FR-3 — duplicate a node via the spine's `DuplicateNode` (full row copy,
/// " (copy)" name, +1m x/z, path-binding keys stripped — fe-database owns the
/// semantics). Replies with `NodeCreated`, which fe-ui already applies. Auth is
/// `CallerAuth::Local` (N-5).
pub(crate) fn handle_duplicate(db_sender: &DbCommandSender, node_id: String) {
    let command = DbCommand::DuplicateNode {
        node_id,
        auth: CallerAuth::Local,
    };
    if submit_scene_write(db_sender, command) == GuiSend::Closed {
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
    let command = DbCommand::RenameNode {
        node_id,
        new_name,
        auth: CallerAuth::Local,
    };
    if submit_scene_write(db_sender, command) == GuiSend::Closed {
        bevy::log::warn!("db_sender channel closed — RenameNode not sent");
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
