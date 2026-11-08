//! The DB thread's command intake: FIFO, except that a peer's Serve admission
//! is answered ahead of the queued backlog; see src/AGENTS.md §command-lane.

use crossbeam::channel::{Receiver, RecvError};
use fe_runtime::messages::DbCommand;
use fe_runtime::scene_asset::SceneAssetTransferPurpose;
use std::collections::VecDeque;

/// Commands drained ahead of the one being handled, so a Serve admission queued
/// behind a replication burst is found (bounded: senders keep their backpressure).
const LOOKAHEAD: usize = 128;

/// FIFO intake with one priority class (QA-0143).
#[derive(Default)]
pub(crate) struct CommandLane {
    queued: VecDeque<DbCommand>,
    /// The last command jumped the queue, so the next one is FIFO (no starvation).
    jumped: bool,
}

impl CommandLane {
    /// The next command to handle, blocking while none is queued.
    pub(crate) fn next(&mut self, rx: &Receiver<DbCommand>) -> Result<DbCommand, RecvError> {
        while self.queued.len() < LOOKAHEAD {
            match rx.try_recv() {
                Ok(command) => self.queued.push_back(command),
                Err(_) => break,
            }
        }
        if !std::mem::take(&mut self.jumped) {
            if let Some(index) = self.queued.iter().position(is_serve_admission) {
                self.jumped = index > 0;
                if let Some(command) = self.queued.remove(index) {
                    return Ok(command);
                }
            }
        }
        match self.queued.pop_front() {
            Some(command) => Ok(command),
            None => rx.recv(),
        }
    }

    /// Commands drained from the channel but not yet handled.
    pub(crate) fn len(&self) -> usize {
        self.queued.len()
    }
}

/// A remote peer waits on this read; it must not wait out the replication backlog.
fn is_serve_admission(command: &DbCommand) -> bool {
    matches!(
        command,
        DbCommand::InspectSceneAssetTransfer { request, .. }
            if matches!(request.purpose, SceneAssetTransferPurpose::Serve { .. })
    )
}
