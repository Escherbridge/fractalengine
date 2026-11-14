//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").
//!
//! Also homes the contextual-controls verb handlers (delete/duplicate/rename/copy-API/report)
//! and the GUI scene write door (`submit_scene_write`; see `actions/AGENTS.md` §scene-write-door).

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use crossbeam::channel::{Sender, TrySendError};
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult};
use fe_runtime::scene_hook_gate::SceneCallerKind;
use fe_runtime::scene_write::{
    SceneAugments, SceneWrite, SceneWriteDoor, SceneWriteError, SceneWriteOutcome,
    SceneWriteTicket,
};

use crate::actions::UiManager;

/// How the door disposed of one GUI write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiSend {
    /// Sent to the DB thread.
    Sent,
    /// Queued behind a gate answer (its own or an earlier write's); the frame drain sends or refuses it.
    Waiting,
    /// The gate refused; the advisory text is queued for the player.
    Refused(String),
    /// A non-blocking submit found the DB channel full: not sent, nothing queued; the caller may submit again.
    Busy,
    /// The DB channel is closed.
    Closed,
}

impl GuiSend {
    /// True when the DB channel did not take the write and no queue owns it (`Busy` or `Closed`).
    pub fn channel_unavailable(&self) -> bool {
        matches!(self, GuiSend::Busy | GuiSend::Closed)
    }
}

/// Where a queued write stands.
enum Queued {
    /// The gate is still answering.
    Gating(SceneWriteTicket),
    /// Admitted, but the DB channel was full; retried next frame.
    Admitted(DbCommand, SceneAugments),
}

struct WaitingWrite {
    state: Queued,
    sender: Sender<DbCommand>,
    /// The owner's reply id; a later refusal or a closed channel answers it with `RequestFailed`.
    correlation_id: Option<String>,
}

/// A settled queue head, handled once the queue lock is released.
enum Settled {
    Sent(SceneAugments),
    Refused {
        message: String,
        correlation_id: Option<String>,
    },
    Closed {
        correlation_id: Option<String>,
    },
}

struct GuiDoorQueue {
    active_petal: Option<String>,
    /// FIFO: writes leave in submit order, so a later write never overtakes a gated one.
    waiting: VecDeque<WaitingWrite>,
    refusals: Vec<String>,
}

static GUI_DOOR: Mutex<GuiDoorQueue> = Mutex::new(GuiDoorQueue {
    active_petal: None,
    waiting: VecDeque::new(),
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
/// The petal is the command's own, else `petal_hint` (the caller's known petal), else the active one.
fn submit_with(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    command: DbCommand,
    blocking: bool,
    petal_hint: Option<&str>,
) -> GuiSend {
    let petal = carried_petal(&command)
        .or_else(|| petal_hint.map(str::to_owned))
        .or_else(|| door_queue().active_petal.clone());
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
    let correlation_id = write.correlation_id().map(str::to_owned);
    let ticket = door.submit(SceneCallerKind::Ui, write);
    let mut queue = door_queue();
    // Behind a queued write this one queues too, even when it settles at once (send order).
    let ticket = if queue.waiting.is_empty() {
        match ticket.poll(Instant::now()) {
            Ok(outcome) => {
                drop(queue);
                return settle(outcome, sender, blocking);
            }
            Err(ticket) => ticket,
        }
    } else {
        ticket
    };
    queue.waiting.push_back(WaitingWrite {
        state: Queued::Gating(ticket),
        sender: sender.clone(),
        correlation_id,
    });
    GuiSend::Waiting
}

fn send_plain(sender: &Sender<DbCommand>, command: DbCommand, blocking: bool) -> GuiSend {
    if blocking {
        return if sender.send(command).is_ok() { GuiSend::Sent } else { GuiSend::Closed };
    }
    match sender.try_send(command) {
        Ok(()) => GuiSend::Sent,
        Err(TrySendError::Full(_)) => GuiSend::Busy,
        Err(TrySendError::Disconnected(_)) => GuiSend::Closed,
    }
}

/// Send an admitted write at once, or queue the refusal advisory (the caller also gets it back).
fn settle(outcome: SceneWriteOutcome, sender: &Sender<DbCommand>, blocking: bool) -> GuiSend {
    match outcome {
        SceneWriteOutcome::Admitted(admitted) => {
            let (command, augments) = admitted.into_parts();
            let sent = send_plain(sender, command, blocking);
            if sent == GuiSend::Sent {
                stage_logged(augments);
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

/// A create's node id is unknown here, so a create augment is dropped with a warning.
fn stage_logged(augments: SceneAugments) {
    if augments.is_empty() {
        return;
    }
    for error in augments.stage(None) {
        bevy::log::warn!(%error, "scene hook augment was not staged");
    }
}

/// Move the queue head one step without blocking; `Err` keeps it at the head (gating, or channel full).
fn advance(write: WaitingWrite, now: Instant) -> Result<Settled, WaitingWrite> {
    let WaitingWrite {
        state,
        sender,
        correlation_id,
    } = write;
    let (command, augments) = match state {
        Queued::Gating(ticket) => match ticket.poll(now) {
            Err(ticket) => {
                return Err(WaitingWrite {
                    state: Queued::Gating(ticket),
                    sender,
                    correlation_id,
                });
            }
            Ok(SceneWriteOutcome::Refused(refusal)) => {
                return Ok(Settled::Refused {
                    message: refusal.message(),
                    correlation_id,
                });
            }
            Ok(SceneWriteOutcome::Admitted(admitted)) => admitted.into_parts(),
        },
        Queued::Admitted(command, augments) => (command, augments),
    };
    match sender.try_send(command) {
        Ok(()) => Ok(Settled::Sent(augments)),
        Err(TrySendError::Full(command)) => Err(WaitingWrite {
            state: Queued::Admitted(command, augments),
            sender,
            correlation_id,
        }),
        Err(TrySendError::Disconnected(_)) => Ok(Settled::Closed { correlation_id }),
    }
}

/// Submit through the installed process gate; blocks like `Sender::send` once admitted.
pub fn submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), &db_sender.0, command, true, None)
}

/// As [`submit_scene_write`] for frame systems that must not block on a full channel.
pub fn try_submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), &db_sender.0, command, false, None)
}

/// As [`try_submit_scene_write`] for doors that hold the raw sender (dialogs).
pub fn try_submit_scene_write_on(sender: &Sender<DbCommand>, command: DbCommand) -> GuiSend {
    submit_with(&SceneWriteDoor::installed(), sender, command, false, None)
}

/// As [`try_submit_scene_write_on`] through an explicit door (an app-provided `SceneWriteDoor` resource),
/// authorized against `petal_id` when the command names no petal itself.
pub fn try_submit_scene_write_through(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    petal_id: &str,
    command: DbCommand,
) -> GuiSend {
    submit_with(door, sender, command, false, Some(petal_id))
}

/// Frame drain: track the active petal, send or refuse queued writes in submit order, show refusals.
/// A queued write's owner learns a refusal or a closed channel as `RequestFailed` on its id.
pub fn drain_gui_scene_writes(
    nav: bevy::prelude::Res<crate::navigation_manager::NavigationManager>,
    clock: bevy::prelude::Res<crate::notice::NoticeClock>,
    mut ui_mgr: bevy::prelude::ResMut<UiManager>,
    mut results: Option<bevy::prelude::ResMut<bevy::prelude::Messages<DbResult>>>,
) {
    let now = Instant::now();
    let mut settled = Vec::new();
    let refusals = {
        let mut queue = door_queue();
        // Compare first: cloning every frame would allocate on the hot path.
        if queue.active_petal != nav.active_petal_id {
            queue.active_petal.clone_from(&nav.active_petal_id);
        }
        while let Some(head) = queue.waiting.pop_front() {
            match advance(head, now) {
                Ok(done) => settled.push(done),
                Err(held) => {
                    queue.waiting.push_front(held);
                    break;
                }
            }
        }
        std::mem::take(&mut queue.refusals)
    };
    let now_secs = clock.elapsed_secs_f64();
    for message in refusals {
        ui_mgr.show_advisory(message, now_secs);
    }
    for done in settled {
        let (error, correlation_id) = match done {
            Settled::Sent(augments) => {
                stage_logged(augments);
                continue;
            }
            Settled::Refused {
                message,
                correlation_id,
            } => {
                ui_mgr.show_advisory(message.clone(), now_secs);
                (message, correlation_id)
            }
            Settled::Closed { correlation_id } => {
                bevy::log::warn!("db_sender channel closed — a queued scene write was not sent");
                (
                    "Database connection closed; edit was not saved".to_owned(),
                    correlation_id,
                )
            }
        };
        if let (Some(correlation_id), Some(results)) = (correlation_id, results.as_mut()) {
            results.write(DbResult::RequestFailed {
                error,
                correlation_id,
            });
        }
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
    if submit_scene_write(db_sender, command).channel_unavailable() {
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
    if submit_scene_write(db_sender, cmd).channel_unavailable() {
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
    if submit_scene_write(db_sender, command).channel_unavailable() {
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
    if submit_scene_write(db_sender, command).channel_unavailable() {
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
