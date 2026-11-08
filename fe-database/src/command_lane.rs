//! The DB thread's command intake: FIFO, except that a peer's Serve admission
//! is answered ahead of queued reads and a peer's newer node snapshot replaces
//! its queued older one; see src/AGENTS.md §command-lane.

use crate::handlers::scene_publication::wire::live_snapshot_of;
use crossbeam::channel::{Receiver, RecvError};
use fe_runtime::messages::DbCommand;
use fe_runtime::scene_asset::SceneAssetTransferPurpose;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Commands drained ahead of the one being handled, so a Serve admission queued
/// behind a read burst is found (bounded: senders keep their backpressure).
const LOOKAHEAD: usize = 128;

/// FIFO intake with one priority class (QA-0143) behind a security barrier (QA-0154).
#[derive(Default)]
pub(crate) struct CommandLane {
    queued: VecDeque<DbCommand>,
    /// The last command jumped the queue, so the next one is FIFO (no starvation).
    jumped: bool,
    /// Inbound node versions absorbed by [`CommandLane::supersede`], for the slow-command log.
    superseded: usize,
}

impl CommandLane {
    /// The next command to handle, blocking while none is queued.
    pub(crate) fn next(&mut self, rx: &Receiver<DbCommand>) -> Result<DbCommand, RecvError> {
        while self.queued.len() < LOOKAHEAD {
            match rx.try_recv() {
                Ok(command) => {
                    if let Some(command) = self.supersede(command) {
                        self.queued.push_back(command);
                    }
                }
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

    /// QA-0164: a peer's newer complete snapshot of a node replaces its queued older one in
    /// place (the older version's slot, so it still precedes dependents queued after it);
    /// `None` when absorbed. See src/AGENTS.md §command-lane "Superseded inbound versions".
    fn supersede(&mut self, command: DbCommand) -> Option<DbCommand> {
        let DbCommand::ApplyReplicatedRow { verse_id, table, record_id, row_bytes, author_did } = &command else {
            return Some(command);
        };
        if table != "node" {
            return Some(command);
        }
        let Some((clock, petal)) = live_snapshot_of(row_bytes, author_did) else {
            return Some(command);
        };
        for queued in self.queued.iter_mut().rev() {
            let DbCommand::ApplyReplicatedRow {
                verse_id: queued_verse,
                table: queued_table,
                record_id: queued_record,
                row_bytes: queued_bytes,
                author_did: queued_author,
            } = queued
            else {
                break;
            };
            if queued_table.as_str() == "node" && *queued_record == *record_id && *queued_verse == *verse_id {
                let older = (*queued_author == *author_did)
                    .then(|| live_snapshot_of(queued_bytes, queued_author))
                    .flatten();
                if older.is_some_and(|(older_clock, older_petal)| older_clock < clock && older_petal == petal) {
                    *queued_bytes = row_bytes.clone();
                    self.superseded += 1;
                    return None;
                }
                break;
            }
            if !matches!(queued_table.as_str(), "node" | "iot_reading" | "device_payload") {
                break;
            }
        }
        Some(command)
    }

    /// Inbound node versions absorbed by a newer queued one since the last call.
    pub(crate) fn take_superseded(&mut self) -> usize {
        std::mem::take(&mut self.superseded)
    }

    /// Commands drained from the channel but not yet handled.
    pub(crate) fn len(&self) -> usize {
        self.queued.len()
    }

    /// No command is waiting, here or in the channel.
    pub(crate) fn is_idle(&self, rx: &Receiver<DbCommand>) -> bool {
        self.queued.is_empty() && rx.is_empty()
    }
}

/// Longest a committed transform waits for its scene publication (QA-0161).
const PUBLICATION_DEFER_MAX: Duration = Duration::from_millis(250);

/// Scene publications of committed transforms, run when the lane idles (QA-0161).
#[derive(Default)]
pub(crate) struct DeferredPublications {
    nodes: Vec<String>,
    oldest: Option<Instant>,
}

impl DeferredPublications {
    /// Queue `node_id`'s publication; one publication per node covers every write before it.
    pub(crate) fn defer(&mut self, node_id: String) {
        self.oldest.get_or_insert_with(Instant::now);
        if !self.nodes.contains(&node_id) {
            self.nodes.push(node_id);
        }
    }

    /// Publish now: the lane idles or the oldest waited its bound.
    pub(crate) fn due(&self, lane_idle: bool) -> bool {
        !self.nodes.is_empty()
            && (lane_idle
                || self
                    .oldest
                    .is_some_and(|oldest| oldest.elapsed() >= PUBLICATION_DEFER_MAX))
    }

    /// Publish before `next` runs (`None`: the channel closed, so publish before exit).
    pub(crate) fn due_before(&self, next: Option<&DbCommand>) -> bool {
        !self.nodes.is_empty() && next.is_none_or(|command| !runs_before_publication(command))
    }

    /// The queued node ids in first-write order, emptying the queue.
    pub(crate) fn take(&mut self) -> Vec<String> {
        self.oldest = None;
        std::mem::take(&mut self.nodes)
    }
}

/// Commands that may run while transform publications wait: the transform/ingest hot path
/// and reads that never consult a scene clock. Every other command, and every variant
/// added later, publishes first, so replication, role and asset commands see them.
fn runs_before_publication(command: &DbCommand) -> bool {
    matches!(
        command,
        DbCommand::UpdateNodeTransform { .. }
            | DbCommand::InsertIotReadings { .. }
            | DbCommand::InsertDevicePayloads { .. }
            | DbCommand::GetNodeTransform { .. }
            | DbCommand::GetNodeProperties { .. }
            | DbCommand::LoadNodesByPetal { .. }
            | DbCommand::LoadHierarchy
            | DbCommand::ReadHierarchy
    )
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
