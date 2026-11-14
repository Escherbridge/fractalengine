//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").
//!
//! Also homes the contextual-controls verb handlers (delete/duplicate/rename/copy-API/report)
//! and the GUI scene write door (`submit_scene_write`, the retrying `submit_scene_write_retrying`
//! and its dead-letter list; see `actions/AGENTS.md` §scene-write-door).

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam::channel::{Sender, TrySendError};
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult, NodeMutation};
use fe_runtime::scene_hook_gate::SceneCallerKind;
use fe_runtime::scene_write::{
    SceneAugments, SceneWrite, SceneWriteDoor, SceneWriteError, SceneWriteOutcome,
    SceneWriteTicket,
};

use crate::actions::UiManager;

/// Retries after the first failed send before a queued write is dead-lettered.
const RETRY_ATTEMPTS: u32 = 8;
/// Backoff before the first retry; doubles per attempt up to [`MAX_RETRY_DELAY`].
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Longest wait between two retries (total budget about 7.2 s, below undo's save timeout).
const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Dead letters kept for the session; past this the oldest is dropped with a warning.
const DEAD_LETTER_CAP: usize = 32;
/// Reason shown when the DB channel stayed full through every retry.
const STAYED_BUSY: &str = "Database stayed busy";
/// Reason shown when the DB channel closed under a queued write.
const CONNECTION_CLOSED: &str = "Database connection closed";

/// How the door disposed of one GUI write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiSend {
    /// Sent to the DB thread.
    Sent,
    /// Queued behind a gate answer or a full channel; the frame drain sends, refuses or dead-letters it.
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

/// What a non-blocking submit does when the DB channel is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FullChannel {
    /// Return `GuiSend::Busy`; nothing is queued.
    Report,
    /// Queue the write with bounded backoff and return `GuiSend::Waiting`.
    Retry,
}

/// Where a queued write stands.
enum Queued {
    /// The gate is still answering.
    Gating(SceneWriteTicket),
    /// Admitted, but the DB channel was full; retried after its backoff.
    Admitted(DbCommand, SceneAugments),
}

/// What a queued write keeps so it can be retried, dead-lettered and re-gated.
struct WriteOrigin {
    door: SceneWriteDoor,
    sender: Sender<DbCommand>,
    /// The petal the write was judged against (`None` for a plain write).
    petal: Option<String>,
    /// The command as submitted, before gating; a dead-letter Retry re-gates this.
    original: DbCommand,
    label: &'static str,
    /// The owner's reply id; a refusal or a dead letter answers it with `RequestFailed`.
    correlation_id: Option<String>,
}

/// The borrowed half of a [`WriteOrigin`], turned owned only when the write is queued.
struct WriteSource<'a> {
    door: &'a SceneWriteDoor,
    sender: &'a Sender<DbCommand>,
    petal: Option<String>,
    correlation_id: Option<String>,
}

impl WriteSource<'_> {
    fn into_origin(self, original: DbCommand) -> WriteOrigin {
        WriteOrigin {
            door: self.door.clone(),
            sender: self.sender.clone(),
            petal: self.petal,
            label: write_label(&original),
            original,
            correlation_id: self.correlation_id,
        }
    }
}

struct WaitingWrite {
    state: Queued,
    origin: WriteOrigin,
    /// Failed sends so far (0 while gating).
    attempts: u32,
    /// The head is not retried before this instant.
    next_attempt: Option<Instant>,
}

/// A write the door gave up on; kept for the session so the player can retry or discard it.
struct DeadLetter {
    id: u64,
    reason: String,
    origin: WriteOrigin,
}

/// One dead letter as the "Unsaved edits" list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterView {
    pub id: u64,
    pub label: &'static str,
    pub reason: String,
}

/// Session-only dead letters, oldest first, at most [`DEAD_LETTER_CAP`].
struct DeadLetters {
    letters: VecDeque<DeadLetter>,
    next_id: u64,
}

impl DeadLetters {
    /// Keep `origin` as a dead letter and return the owner's notice.
    fn push(&mut self, origin: WriteOrigin, reason: &'static str) -> Settled {
        let notice = Settled::DeadLettered {
            label: origin.label,
            reason,
            correlation_id: origin.correlation_id.clone(),
        };
        if self.letters.len() >= DEAD_LETTER_CAP {
            if let Some(dropped) = self.letters.pop_front() {
                bevy::log::warn!(
                    "unsaved edit list is full; dropped the oldest ({}: {})",
                    dropped.origin.label,
                    dropped.reason
                );
            }
        }
        self.next_id += 1;
        self.letters.push_back(DeadLetter {
            id: self.next_id,
            reason: reason.to_owned(),
            origin,
        });
        notice
    }
}

/// A settled queue head, handled once the queue lock is released.
enum Settled {
    Sent(SceneAugments),
    Refused {
        message: String,
        correlation_id: Option<String>,
    },
    /// Retries ran out or the channel closed; the write is in the dead-letter list.
    DeadLettered {
        label: &'static str,
        reason: &'static str,
        correlation_id: Option<String>,
    },
}

struct GuiDoorQueue {
    active_petal: Option<String>,
    /// FIFO: writes leave in submit order, so a later write never overtakes a gated or backing-off one.
    waiting: VecDeque<WaitingWrite>,
    refusals: Vec<String>,
    dead: DeadLetters,
}

impl GuiDoorQueue {
    const fn new() -> Self {
        Self {
            active_petal: None,
            waiting: VecDeque::new(),
            refusals: Vec::new(),
            dead: DeadLetters {
                letters: VecDeque::new(),
                next_id: 0,
            },
        }
    }
}

static GUI_DOOR: Mutex<GuiDoorQueue> = Mutex::new(GuiDoorQueue::new());

fn lock(doors: &Mutex<GuiDoorQueue>) -> MutexGuard<'_, GuiDoorQueue> {
    doors.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn door_queue() -> MutexGuard<'static, GuiDoorQueue> {
    lock(&GUI_DOOR)
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

/// The reply id a command carries (mirror of `SceneWrite::correlation_id`, for plain writes).
fn command_correlation_id(command: &DbCommand) -> Option<String> {
    match command {
        DbCommand::CreateNode { correlation_id, .. }
        | DbCommand::CreateNodeWithAsset { correlation_id, .. }
        | DbCommand::CreateObject { correlation_id, .. }
        | DbCommand::SetNodeProperty { correlation_id, .. }
        | DbCommand::DeleteNodeProperty { correlation_id, .. }
        | DbCommand::CommitNodeMutation { correlation_id, .. }
        | DbCommand::SetNodeView { correlation_id, .. } => correlation_id.clone(),
        DbCommand::PromoteCheckedStamp { correlation_id, .. }
        | DbCommand::CommitCheckedNodeMutation { correlation_id, .. }
        | DbCommand::CommitCheckedEditorBatch { correlation_id, .. }
        | DbCommand::CommitStampOverrides { correlation_id, .. } => Some(correlation_id.clone()),
        _ => None,
    }
}

/// A short human name for a write, as the "Unsaved edits" list and its error notice show it.
fn write_label(command: &DbCommand) -> &'static str {
    match command {
        DbCommand::PromoteCheckedStamp { .. } => "Stamp promotion",
        DbCommand::CommitStampOverrides { .. } => "Stamp override",
        DbCommand::CommitCheckedNodeMutation { mutation, .. }
        | DbCommand::CommitNodeMutation { mutation, .. } => match mutation {
            NodeMutation::FullTransform { .. } => "Move",
            NodeMutation::PropertySet { key, .. } if key == "gpx_points" => "Point edit",
            NodeMutation::PropertySet { .. } | NodeMutation::PropertyDeleted { .. } => "Property",
        },
        DbCommand::UpdateNodeTransform { .. } => "Move",
        DbCommand::SetNodeProperty { .. }
        | DbCommand::DeleteNodeProperty { .. }
        | DbCommand::ClearNodeProperties { .. } => "Property",
        DbCommand::CommitCheckedEditorBatch { .. } => "Batch edit",
        DbCommand::CreateNode { .. }
        | DbCommand::CreateNodeWithAsset { .. }
        | DbCommand::CreateObject { .. }
        | DbCommand::ImportGltf { .. } => "New object",
        _ => "Edit",
    }
}

/// Exponential backoff after `attempts` failed sends: 50 ms doubling, capped at 2 s.
fn retry_delay(attempts: u32) -> Duration {
    let doublings = attempts.saturating_sub(1).min(16);
    FIRST_RETRY_DELAY
        .saturating_mul(1u32 << doublings)
        .min(MAX_RETRY_DELAY)
}

/// Submit one scene write through `door` as caller `Ui`; admitted writes go to `sender`.
/// The petal is the command's own, else `petal_hint` (the caller's known petal), else the active one.
#[allow(clippy::too_many_arguments)]
fn submit_with(
    doors: &Mutex<GuiDoorQueue>,
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    command: DbCommand,
    blocking: bool,
    petal_hint: Option<&str>,
    full: FullChannel,
    now: Instant,
) -> GuiSend {
    let petal = carried_petal(&command)
        .or_else(|| petal_hint.map(str::to_owned))
        .or_else(|| lock(doors).active_petal.clone());
    let plain = |command: DbCommand| {
        let source = WriteSource {
            door,
            sender,
            petal: None,
            correlation_id: None,
        };
        send_unqueued(doors, source, command, blocking, full, now)
    };
    let Some(petal) = petal else {
        // No petal to authorize against: nothing bound can resolve, so the write is plain.
        return plain(command);
    };
    let write = match SceneWrite::new(petal.clone(), command.clone()) {
        Ok(write) => write,
        Err(SceneWriteError::NotASceneWrite) => return plain(command),
        Err(error) => {
            let message = format!("Edit was not saved: {error}");
            lock(doors).refusals.push(message.clone());
            return GuiSend::Refused(message);
        }
    };
    let source = WriteSource {
        door,
        sender,
        petal: Some(petal),
        correlation_id: write.correlation_id().map(str::to_owned),
    };
    let ticket = door.submit(SceneCallerKind::Ui, write);
    let mut queue = lock(doors);
    // Behind a queued write this one queues too, even when it settles at once (send order).
    let ticket = if queue.waiting.is_empty() {
        match ticket.poll(now) {
            Ok(outcome) => {
                drop(queue);
                return settle(doors, outcome, source, command, blocking, full, now);
            }
            Err(ticket) => ticket,
        }
    } else {
        ticket
    };
    queue.waiting.push_back(WaitingWrite {
        state: Queued::Gating(ticket),
        origin: source.into_origin(command),
        attempts: 0,
        next_attempt: None,
    });
    GuiSend::Waiting
}

/// A send that did not settle because the channel was full hands the command back.
enum Delivery {
    Done(GuiSend),
    Full(DbCommand),
}

fn send_plain(sender: &Sender<DbCommand>, command: DbCommand, blocking: bool) -> Delivery {
    if blocking {
        return Delivery::Done(if sender.send(command).is_ok() {
            GuiSend::Sent
        } else {
            GuiSend::Closed
        });
    }
    match sender.try_send(command) {
        Ok(()) => Delivery::Done(GuiSend::Sent),
        Err(TrySendError::Full(command)) => Delivery::Full(command),
        Err(TrySendError::Disconnected(_)) => Delivery::Done(GuiSend::Closed),
    }
}

/// Send a write the gate never judges (no petal, or not a scene write).
fn send_unqueued(
    doors: &Mutex<GuiDoorQueue>,
    mut source: WriteSource<'_>,
    command: DbCommand,
    blocking: bool,
    full: FullChannel,
    now: Instant,
) -> GuiSend {
    match send_plain(source.sender, command, blocking) {
        Delivery::Done(sent) => sent,
        Delivery::Full(_) if full == FullChannel::Report => GuiSend::Busy,
        Delivery::Full(command) => {
            source.correlation_id = command_correlation_id(&command);
            let original = command.clone();
            queue_retry(doors, source, original, command, SceneAugments::default(), now)
        }
    }
}

/// Send an admitted write at once, or queue the refusal advisory (the caller also gets it back).
fn settle(
    doors: &Mutex<GuiDoorQueue>,
    outcome: SceneWriteOutcome,
    source: WriteSource<'_>,
    original: DbCommand,
    blocking: bool,
    full: FullChannel,
    now: Instant,
) -> GuiSend {
    match outcome {
        SceneWriteOutcome::Admitted(admitted) => {
            let (command, augments) = admitted.into_parts();
            match send_plain(source.sender, command, blocking) {
                Delivery::Done(sent) => {
                    if sent == GuiSend::Sent {
                        stage_logged(augments);
                    }
                    sent
                }
                Delivery::Full(_) if full == FullChannel::Report => GuiSend::Busy,
                Delivery::Full(command) => {
                    queue_retry(doors, source, original, command, augments, now)
                }
            }
        }
        SceneWriteOutcome::Refused(refusal) => {
            let message = refusal.message();
            lock(doors).refusals.push(message.clone());
            GuiSend::Refused(message)
        }
    }
}

/// Queue an admitted write that met a full channel: first retry after [`FIRST_RETRY_DELAY`].
fn queue_retry(
    doors: &Mutex<GuiDoorQueue>,
    source: WriteSource<'_>,
    original: DbCommand,
    command: DbCommand,
    augments: SceneAugments,
    now: Instant,
) -> GuiSend {
    lock(doors).waiting.push_back(WaitingWrite {
        state: Queued::Admitted(command, augments),
        origin: source.into_origin(original),
        attempts: 1,
        next_attempt: Some(now + retry_delay(1)),
    });
    GuiSend::Waiting
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

/// Move the queue head one step without blocking; `Err` keeps it at the head
/// (gating, or backing off after a full channel). Retries past the bound and a closed
/// channel dead-letter the write; a gate refusal is final and never dead-lettered.
fn advance(
    write: WaitingWrite,
    now: Instant,
    dead: &mut DeadLetters,
) -> Result<Settled, WaitingWrite> {
    if write.next_attempt.is_some_and(|at| now < at) {
        return Err(write);
    }
    let WaitingWrite {
        state,
        origin,
        attempts,
        next_attempt,
    } = write;
    let (command, augments) = match state {
        Queued::Gating(ticket) => match ticket.poll(now) {
            Err(ticket) => {
                return Err(WaitingWrite {
                    state: Queued::Gating(ticket),
                    origin,
                    attempts,
                    next_attempt,
                });
            }
            Ok(SceneWriteOutcome::Refused(refusal)) => {
                return Ok(Settled::Refused {
                    message: refusal.message(),
                    correlation_id: origin.correlation_id,
                });
            }
            Ok(SceneWriteOutcome::Admitted(admitted)) => admitted.into_parts(),
        },
        Queued::Admitted(command, augments) => (command, augments),
    };
    match origin.sender.try_send(command) {
        Ok(()) => Ok(Settled::Sent(augments)),
        Err(TrySendError::Full(command)) => {
            let attempts = attempts + 1;
            if attempts > RETRY_ATTEMPTS {
                return Ok(dead.push(origin, STAYED_BUSY));
            }
            Err(WaitingWrite {
                state: Queued::Admitted(command, augments),
                origin,
                attempts,
                next_attempt: Some(now + retry_delay(attempts)),
            })
        }
        Err(TrySendError::Disconnected(_)) => Ok(dead.push(origin, CONNECTION_CLOSED)),
    }
}

/// Settle queue heads in submit order until one is held; dead letters land in the list here.
fn pump_locked(queue: &mut GuiDoorQueue, now: Instant, settled: &mut Vec<Settled>) {
    let GuiDoorQueue { waiting, dead, .. } = queue;
    while let Some(head) = waiting.pop_front() {
        match advance(head, now, dead) {
            Ok(done) => settled.push(done),
            Err(held) => {
                waiting.push_front(held);
                break;
            }
        }
    }
}

/// Submit through the installed process gate; blocks like `Sender::send` once admitted.
pub fn submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    let door = SceneWriteDoor::installed();
    submit_with(&GUI_DOOR, &door, &db_sender.0, command, true, None, FullChannel::Report, Instant::now())
}

/// Blocking submit authorized against `petal_id`: follow-up writes created from a `DbResult` name the node's own petal.
pub fn submit_scene_write_in(db_sender: &DbCommandSender, petal_id: &str, command: DbCommand) -> GuiSend {
    submit_scene_write_in_through(&SceneWriteDoor::installed(), &db_sender.0, petal_id, command)
}

/// As [`submit_scene_write_in`] through an explicit door and raw sender.
pub(crate) fn submit_scene_write_in_through(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    petal_id: &str,
    command: DbCommand,
) -> GuiSend {
    submit_in(&GUI_DOOR, door, sender, petal_id, command)
}

/// Blocking, petal-hinted submit on an explicit queue (the test journey passes its own).
fn submit_in(
    doors: &Mutex<GuiDoorQueue>,
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    petal_id: &str,
    command: DbCommand,
) -> GuiSend {
    submit_with(doors, door, sender, command, true, Some(petal_id), FullChannel::Report, Instant::now())
}

/// As [`submit_scene_write`] for frame systems that must not block on a full channel.
pub fn try_submit_scene_write(db_sender: &DbCommandSender, command: DbCommand) -> GuiSend {
    let door = SceneWriteDoor::installed();
    submit_with(&GUI_DOOR, &door, &db_sender.0, command, false, None, FullChannel::Report, Instant::now())
}

/// As [`try_submit_scene_write`] for doors that hold the raw sender (dialogs).
pub fn try_submit_scene_write_on(sender: &Sender<DbCommand>, command: DbCommand) -> GuiSend {
    let door = SceneWriteDoor::installed();
    submit_with(&GUI_DOOR, &door, sender, command, false, None, FullChannel::Report, Instant::now())
}

/// As [`try_submit_scene_write_on`] through an explicit door (an app-provided `SceneWriteDoor` resource),
/// authorized against `petal_id` when the command names no petal itself.
pub fn try_submit_scene_write_through(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    petal_id: &str,
    command: DbCommand,
) -> GuiSend {
    submit_with(&GUI_DOOR, door, sender, command, false, Some(petal_id), FullChannel::Report, Instant::now())
}

/// As [`try_submit_scene_write_through`], but a full channel queues the write with bounded backoff
/// (`Waiting`, never `Busy`); a write that exhausts its retries is dead-lettered and its owner gets
/// `RequestFailed` on its correlation id.
pub fn submit_scene_write_retrying(
    door: &SceneWriteDoor,
    sender: &Sender<DbCommand>,
    petal_hint: Option<&str>,
    command: DbCommand,
) -> GuiSend {
    submit_with(&GUI_DOOR, door, sender, command, false, petal_hint, FullChannel::Retry, Instant::now())
}

/// (writes gating or backing off, dead letters) in the GUI door; allocation-free.
pub fn scene_door_counts() -> (usize, usize) {
    door_counts(&GUI_DOOR)
}

/// The "Unsaved edits" list, oldest first; call only while it is shown.
pub fn dead_letters() -> Vec<DeadLetterView> {
    dead_letter_views(&GUI_DOOR)
}

/// Re-submit a dead letter's original command (re-gated, fresh retry budget, same correlation id).
/// An unknown id is `Refused`; a closed channel keeps the letter listed.
pub fn retry_dead_letter(id: u64) -> GuiSend {
    retry_dead_letter_in(&GUI_DOOR, id, Instant::now())
}

/// Drop a dead letter for good; false when no letter has that id.
pub fn discard_dead_letter(id: u64) -> bool {
    discard_dead_letter_in(&GUI_DOOR, id)
}

/// True while a write with `correlation_id` is gating or retrying; false once sent or dead-lettered.
pub fn scene_write_in_door(correlation_id: &str) -> bool {
    write_in_door(&GUI_DOOR, correlation_id)
}

fn door_counts(doors: &Mutex<GuiDoorQueue>) -> (usize, usize) {
    let queue = lock(doors);
    (queue.waiting.len(), queue.dead.letters.len())
}

fn dead_letter_views(doors: &Mutex<GuiDoorQueue>) -> Vec<DeadLetterView> {
    lock(doors)
        .dead
        .letters
        .iter()
        .map(|letter| DeadLetterView {
            id: letter.id,
            label: letter.origin.label,
            reason: letter.reason.clone(),
        })
        .collect()
}

fn write_in_door(doors: &Mutex<GuiDoorQueue>, correlation_id: &str) -> bool {
    lock(doors)
        .waiting
        .iter()
        .any(|write| write.origin.correlation_id.as_deref() == Some(correlation_id))
}

fn discard_dead_letter_in(doors: &Mutex<GuiDoorQueue>, id: u64) -> bool {
    let mut queue = lock(doors);
    let Some(index) = queue.dead.letters.iter().position(|letter| letter.id == id) else {
        return false;
    };
    queue.dead.letters.remove(index).is_some()
}

fn retry_dead_letter_in(doors: &Mutex<GuiDoorQueue>, id: u64, now: Instant) -> GuiSend {
    let taken = {
        let mut queue = lock(doors);
        let index = queue.dead.letters.iter().position(|letter| letter.id == id);
        index.and_then(|index| Some((index, queue.dead.letters.remove(index)?)))
    };
    let Some((index, mut letter)) = taken else {
        return GuiSend::Refused("That unsaved edit is no longer listed".to_owned());
    };
    let origin = &letter.origin;
    let sent = submit_with(
        doors,
        &origin.door,
        &origin.sender,
        origin.original.clone(),
        false,
        origin.petal.as_deref(),
        FullChannel::Retry,
        now,
    );
    if sent == GuiSend::Closed {
        // Nothing took the write: keep it listed where it was.
        letter.reason = CONNECTION_CLOSED.to_owned();
        let mut queue = lock(doors);
        let index = index.min(queue.dead.letters.len());
        queue.dead.letters.insert(index, letter);
    }
    sent
}

/// Frame drain: track the active petal, send, refuse or dead-letter queued writes in submit order,
/// show refusals and dead letters. The owner learns a refusal or a dead letter as `RequestFailed`.
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
        pump_locked(&mut queue, now, &mut settled);
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
            Settled::DeadLettered {
                label,
                reason,
                correlation_id,
            } => {
                bevy::log::warn!("a queued scene write was dead-lettered ({label}: {reason})");
                ui_mgr.show_error(
                    format!("{label} was not saved: {reason}. It is listed under Unsaved edits."),
                    now_secs,
                );
                (format!("{label} was not saved: {reason}"), correlation_id)
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

/// Test seam: the drain's head loop at an explicit instant, on an explicit queue.
#[cfg(test)]
fn pump_scene_door_at(doors: &Mutex<GuiDoorQueue>, now: Instant) -> Vec<Settled> {
    let mut settled = Vec::new();
    pump_locked(&mut lock(doors), now, &mut settled);
    settled
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

    fn property_write(correlation_id: &str) -> DbCommand {
        DbCommand::SetNodeProperty {
            node_id: format!("door-journey-{correlation_id}"),
            key: "door_journey".into(),
            value: serde_json::Value::from(1),
            correlation_id: Some(correlation_id.to_owned()),
        }
    }

    fn received_id(rx: &crossbeam::channel::Receiver<DbCommand>) -> Option<String> {
        rx.try_recv().ok().as_ref().and_then(command_correlation_id)
    }

    /// Own queue + explicit instants: no shared state with parallel tests, no timing races.
    #[test]
    fn scene_write_door_retry_and_dead_letter_journey() {
        let doors = Mutex::new(GuiDoorQueue::new());
        let door = SceneWriteDoor::default();
        let petal = Some("door-journey-petal");
        let start = Instant::now();
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(DbCommand::Ping).unwrap();

        // Report contract: a plain non-blocking submit on a full channel is Busy and queues nothing.
        let busy = submit_with(&doors, &door, &tx, property_write("busy"), false, petal, FullChannel::Report, start);
        assert_eq!(busy, GuiSend::Busy);
        assert_eq!(door_counts(&doors), (0, 0));

        // Opt-in retry: Waiting, held until its first backoff passes, then sent.
        let first = submit_with(&doors, &door, &tx, property_write("first"), false, petal, FullChannel::Retry, start);
        assert_eq!(first, GuiSend::Waiting);
        assert!(write_in_door(&doors, "first"));
        assert!(pump_scene_door_at(&doors, start + Duration::from_millis(10)).is_empty());
        assert!(write_in_door(&doors, "first"), "a head backing off stays queued");
        assert!(matches!(rx.try_recv(), Ok(DbCommand::Ping)));
        let settled = pump_scene_door_at(&doors, start + Duration::from_millis(60));
        assert!(matches!(settled.as_slice(), [Settled::Sent(_)]));
        assert_eq!(received_id(&rx).as_deref(), Some("first"));
        assert!(!write_in_door(&doors, "first"));
        assert_eq!(door_counts(&doors), (0, 0));

        // A channel that stays full through every retry dead-letters the write.
        tx.send(DbCommand::Ping).unwrap();
        let second_at = start + Duration::from_secs(1);
        let second = submit_with(&doors, &door, &tx, property_write("second"), false, petal, FullChannel::Retry, second_at);
        assert_eq!(second, GuiSend::Waiting);
        let mut dead = Vec::new();
        for step in 1..=RETRY_ATTEMPTS {
            assert!(write_in_door(&doors, "second"), "still retrying before pump {step}");
            dead = pump_scene_door_at(&doors, second_at + Duration::from_secs(10 * u64::from(step)));
        }
        match dead.as_slice() {
            [Settled::DeadLettered { label, reason, correlation_id }] => {
                assert_eq!((*label, *reason), ("Property", STAYED_BUSY));
                assert_eq!(correlation_id.as_deref(), Some("second"));
            }
            _ => panic!("the ninth failed send must dead-letter the write"),
        }
        assert!(!write_in_door(&doors, "second"));
        assert_eq!(door_counts(&doors), (0, 1));
        let listed = dead_letter_views(&doors);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].label, listed[0].reason.as_str()), ("Property", "Database stayed busy"));

        // Retry re-submits the original command with the same correlation id once the channel drains.
        assert!(matches!(rx.try_recv(), Ok(DbCommand::Ping)));
        let retried = retry_dead_letter_in(&doors, listed[0].id, second_at + Duration::from_secs(100));
        assert_eq!(retried, GuiSend::Sent);
        assert_eq!(received_id(&rx).as_deref(), Some("second"));
        assert_eq!(door_counts(&doors), (0, 0));

        // A closed channel dead-letters at once; Retry cannot send and keeps it listed; Discard drops it.
        let (closed_tx, closed_rx) = crossbeam::channel::bounded(1);
        closed_tx.send(DbCommand::Ping).unwrap();
        let third_at = start + Duration::from_secs(200);
        let third = submit_with(&doors, &door, &closed_tx, property_write("third"), false, petal, FullChannel::Retry, third_at);
        assert_eq!(third, GuiSend::Waiting);
        drop(closed_rx);
        let closed = pump_scene_door_at(&doors, third_at + Duration::from_secs(1));
        assert!(matches!(
            closed.as_slice(),
            [Settled::DeadLettered { reason: CONNECTION_CLOSED, correlation_id: Some(id), .. }] if id == "third"
        ));
        let listed = dead_letter_views(&doors);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, "Database connection closed");
        let third_id = listed[0].id;
        assert_eq!(retry_dead_letter_in(&doors, third_id, third_at + Duration::from_secs(2)), GuiSend::Closed);
        assert_eq!(door_counts(&doors), (0, 1), "an unsendable retry stays listed");
        assert!(discard_dead_letter_in(&doors, third_id));
        assert!(!discard_dead_letter_in(&doors, third_id));
        assert_eq!(door_counts(&doors), (0, 0));
        assert!(matches!(retry_dead_letter_in(&doors, third_id, third_at), GuiSend::Refused(_)));

        // Petal-hint leg: a follow-up write is judged against the node's own petal, not the active one.
        {
            use fe_runtime::scene_hook_gate::{
                AugmentOp, SceneBinding, SceneHook, SceneHookBackend, SceneHookCall, SceneHookGate,
                SceneHookReply, SceneHookReplySender,
            };
            struct RefusingBackend;
            impl SceneHookBackend for RefusingBackend {
                fn local_did(&self) -> &str {
                    "did:key:local"
                }
                fn bound(&self, _hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
                    (petal_id == "p-node").then(|| SceneBinding {
                        package_id: "org.example.zoning".into(),
                        binding_scope: "VERSE#v-FRACTAL#f".into(),
                        config: serde_json::json!({}),
                        property_scope: vec!["*".into()],
                        may_augment: false,
                    })
                }
                fn dispatch(&self, _call: SceneHookCall, reply: SceneHookReplySender) {
                    let _ = reply.send(SceneHookReply::Refused("outside the zone".into()));
                }
                fn stage_augment(
                    &self,
                    _petal: &str,
                    _package: &str,
                    _node_id: &str,
                    _ops: Vec<AugmentOp>,
                ) -> Result<(), String> {
                    Ok(())
                }
            }
            let (door_tx, _door_rx) = crossbeam::channel::unbounded();
            let hooked = SceneWriteDoor::new(SceneHookGate::new(std::sync::Arc::new(RefusingBackend)), door_tx);
            let (tx, rx) = crossbeam::channel::bounded(4);
            // The journey's own queue (active petal unset): the global door is shared with parallel tests.
            let refused = submit_in(&doors, &hooked, &tx, "p-node", property_write("node-petal"));
            assert!(matches!(refused, GuiSend::Refused(_)), "judged against the node's petal");
            assert!(rx.try_recv().is_err(), "a refused follow-up sends nothing");
            assert_eq!(door_counts(&doors), (0, 0), "a gate refusal is final, never dead-lettered");
            let sent = submit_in(&doors, &hooked, &tx, "p-active", property_write("active-petal"));
            assert_eq!(sent, GuiSend::Sent);
            assert_eq!(received_id(&rx).as_deref(), Some("active-petal"));
        }
    }
}
