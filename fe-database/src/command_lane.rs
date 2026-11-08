//! The DB thread's command intake: FIFO, except that a peer's Serve admission
//! is answered ahead of queued reads; see src/AGENTS.md §command-lane.

use crossbeam::channel::{Receiver, RecvError};
use fe_runtime::messages::DbCommand;
use fe_runtime::scene_asset::SceneAssetTransferPurpose;
use std::collections::VecDeque;

/// Commands drained ahead of the one being handled, so a Serve admission queued
/// behind a read burst is found (bounded: senders keep their backpressure).
const LOOKAHEAD: usize = 128;

/// FIFO intake with one priority class (QA-0143) behind a security barrier (QA-0154).
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
            // QA-0154: a Serve admission never overtakes a command that could change what
            // it judges (a revoke, a role/ACL or replicated-row apply, an asset write ...).
            let index = self
                .queued
                .iter()
                .take_while(|command| is_transparent(command))
                .position(is_serve_admission);
            if let Some(index) = index {
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

/// A remote peer waits on this read; it must not wait out a queued read burst.
fn is_serve_admission(command: &DbCommand) -> bool {
    matches!(
        command,
        DbCommand::InspectSceneAssetTransfer { request, .. }
            if matches!(request.purpose, SceneAssetTransferPurpose::Serve { .. })
    )
}

/// Commands a Serve admission may be answered ahead of: reads, and writes that cannot
/// change a role, grant, membership, asset row, classification or node→asset reference
/// (transforms, readings, device payloads). Every other command, and every variant added
/// later, is a barrier — the allowlist fails closed (QA-0154).
fn is_transparent(command: &DbCommand) -> bool {
    matches!(
        command,
        DbCommand::Ping
            | DbCommand::InspectSceneAssetTransfer { .. }
            | DbCommand::ReadSceneAssetPublication { .. }
            | DbCommand::ReadSceneAssetAdoption { .. }
            | DbCommand::ReadReplicatedNodeCurrent { .. }
            | DbCommand::ReadEditorBatch { .. }
            | DbCommand::LoadHierarchy
            | DbCommand::ReadHierarchy
            | DbCommand::ReadNodeUrl { .. }
            | DbCommand::ReadNativePetalView { .. }
            | DbCommand::GetEntityDetails { .. }
            | DbCommand::LoadNodesByPetal { .. }
            | DbCommand::GetNodeTransform { .. }
            | DbCommand::GetNodeProperties { .. }
            | DbCommand::CountNodeDescendants { .. }
            | DbCommand::GetPetalVisibilityGroups { .. }
            | DbCommand::ReadNodeHistory { .. }
            | DbCommand::ReadNodeMeasurements { .. }
            | DbCommand::ReadSceneMaterials { .. }
            | DbCommand::RawQuery { .. }
            | DbCommand::ExecuteTsPartial { .. }
            | DbCommand::GetPetalTerrain { .. }
            | DbCommand::GetPetalRenderDistance { .. }
            | DbCommand::UpdateNodeTransform { .. }
            | DbCommand::InsertIotReadings { .. }
            | DbCommand::InsertDevicePayloads { .. }
    )
}
