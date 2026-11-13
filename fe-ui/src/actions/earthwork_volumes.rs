//! Bounded background measurement saves; see actions/AGENTS.md section earthwork-volume-ownership.

use super::{
    UiManager,
    terrain_proposal::{EARTHWORK_NODE_KIND, EarthworkNodeMap, KEY_CUT_VOLUME, KEY_FILL_VOLUME},
};
use crate::{
    navigation_manager::NavigationManager, plugin::LocalUserRole, terrain_map::PetalMapState,
    verse_manager::VerseManager,
};
use bevy::prelude::*;
use fe_renderer::terrain_overlay::{
    EarthworkTerrainBinding, EarthworkVolumeReport, EarthworkVolumeState,
};
use fe_runtime::{
    app::DbCommandSender,
    editor_batch::*,
    messages::{DbCommand, DbResult},
};
use std::collections::HashMap;

const TIMEOUT: f64 = 15.0;
const RETRY_READ: f64 = 5.0;

#[derive(Clone, Debug, PartialEq)]
struct Demand {
    scope: String,
    navigation: [String; 3],
    node: String,
    report: EarthworkVolumeReport,
    raw_source: [u8; 32],
    incarnation: u128,
    access: Option<u128>,
    node_source: Option<crate::verse_manager::SceneAdmission>,
    node_epoch: Option<u64>,
}

struct Read {
    id: String,
    demand: Demand,
    accepted: bool,
    at: f64,
}

struct Write {
    id: String,
    demand: Demand,
    capture: EditorBatchCapture,
    accepted: bool,
    retired: bool,
    at: f64,
    resolve: Option<(String, bool, f64)>,
}

struct Confirmed {
    demand: Demand,
    version: i64,
    node_source: Option<crate::verse_manager::SceneAdmission>,
    node_epoch: Option<u64>,
}

/// One background request owner; it never changes selection or records user Undo.
#[derive(Default, Resource)]
pub(crate) struct VolumePersistence {
    read: Option<Read>,
    write: Option<Write>,
    handled: HashMap<String, Demand>,
    confirmed: HashMap<String, Confirmed>,
    status: HashMap<String, &'static str>,
    next_read: f64,
}

#[derive(Default)]
pub(crate) struct VolumeView {
    status: HashMap<String, &'static str>,
}

impl VolumeView {
    pub(crate) fn status(&self, node: &str) -> &'static str {
        self.status.get(node).copied().unwrap_or("Current volume is unavailable or awaiting complete terrain coverage and checked readback. Stored values, if present, are the last confirmed values.")
    }
}

impl VolumePersistence {
    pub(crate) fn owns_result(&self, result: &DbResult) -> bool {
        match result {
            DbResult::EditorBatchLoaded { correlation_id, .. } => self
                .read
                .as_ref()
                .is_some_and(|read| read.accepted && read.id == *correlation_id),
            DbResult::EditorBatchCommitted { correlation_id, .. } => self
                .write
                .as_ref()
                .is_some_and(|write| write.accepted && write.id == *correlation_id),
            DbResult::EditorBatchResolved {
                correlation_id,
                resolution,
            } => self.write.as_ref().is_some_and(|write| {
                write.accepted
                    && write.id == resolution.request_id
                    && write
                        .resolve
                        .as_ref()
                        .is_some_and(|(id, accepted, _)| *accepted && id == correlation_id)
            }),
            DbResult::RequestFailed { correlation_id, .. } => {
                self.read
                    .as_ref()
                    .is_some_and(|read| read.accepted && read.id == *correlation_id)
                    || self.write.as_ref().is_some_and(|write| {
                        write.accepted
                            && (write.id == *correlation_id
                                || write.resolve.as_ref().is_some_and(|(id, accepted, _)| {
                                    *accepted && id == correlation_id
                                }))
                    })
            }
            _ => false,
        }
    }

    pub(crate) fn projection_receipt<'a>(
        &self,
        result: &'a DbResult,
        live: &Live<'_>,
    ) -> Option<&'a EditorBatchReceipt> {
        if !self.owns_result(result) {
            return None;
        }
        let write = self.write.as_ref()?;
        if write.retired
            || !live.matches(&write.demand)
            || live
                .manager
                .scene_admission(&write.demand.node, &write.demand.report.source.petal_id)
                != write.demand.node_source
            || live
                .manager
                .node_cache_epochs
                .get(&write.demand.node)
                .copied()
                != write.demand.node_epoch
        {
            return None;
        }
        let receipt = match result {
            DbResult::EditorBatchCommitted { receipt, .. } => receipt,
            DbResult::EditorBatchResolved { resolution, .. } => {
                let receipt = resolution.receipt.as_ref()?;
                if !capture_matches(&resolution.current, &write.demand)
                    || serde_json::to_value(&resolution.current.members).ok()
                        != serde_json::to_value(&receipt.members).ok()
                {
                    return None;
                }
                receipt
            }
            _ => return None,
        };
        (receipt_matches(receipt, write)
            && live
                .manager
                .committed_versions
                .get(&write.demand.node)
                .is_none_or(|known| {
                    receipt.members[0]
                        .history_version
                        .is_some_and(|saved| *known <= saved)
                }))
        .then_some(receipt)
    }

    pub(crate) fn update_view(&self, view: &mut VolumeView) {
        if view.status != self.status {
            view.status.clone_from(&self.status);
        }
    }

    pub(crate) fn retire_for_terminal_reset(&mut self) {
        self.read = None;
        self.handled.clear();
        self.confirmed.clear();
        self.status.clear();
        if self.write.as_ref().is_some_and(|write| !write.accepted) {
            self.write = None;
        }
        if let Some(write) = &mut self.write {
            write.retired = true;
        }
    }

    pub(crate) fn drain_terminal_result(&mut self, result: &DbResult) {
        if !self.owns_result(result) {
            return;
        }
        let Some(write) = &self.write else {
            return;
        };
        let settled = match result {
            DbResult::EditorBatchCommitted { receipt, .. } => receipt_matches(receipt, write),
            DbResult::EditorBatchResolved { resolution, .. } => match &resolution.receipt {
                Some(receipt) => receipt_matches(receipt, write),
                None => capture_matches(&resolution.current, &write.demand),
            },
            _ => false,
        };
        if settled {
            self.write = None;
        }
    }
}

pub(crate) struct Live<'a> {
    pub(crate) nav: &'a NavigationManager,
    pub(crate) map: &'a EarthworkNodeMap,
    pub(crate) terrain: &'a PetalMapState,
    pub(crate) volumes: &'a EarthworkVolumeState,
    pub(crate) binding: &'a EarthworkTerrainBinding,
    pub(crate) manager: &'a VerseManager,
    pub(crate) role: &'a LocalUserRole,
    pub(crate) raw_source: Option<[u8; 32]>,
    pub(crate) running: bool,
    pub(crate) height_generation: Option<u64>,
}

impl Live<'_> {
    fn admitted(&self, report: &EarthworkVolumeReport) -> Option<(&str, [u8; 32])> {
        let petal = &report.source.petal_id;
        let node = self.map.node_for(petal, &report.region_id)?;
        let raw_source = self.binding.raw_fingerprint?;
        if !self.running
            || !self.role.can_edit()
            || self.manager.scene_access_blocked()
            || self.nav.active_petal_id.as_deref() != Some(petal.as_str())
            || self.terrain.petal_id.as_deref() != Some(petal.as_str())
            || !self.terrain.loaded
            || self.terrain.pending_set.is_some()
            || self.binding.petal_id.as_deref() != Some(petal.as_str())
            || !self.binding.input_scale_valid
            || self.binding.config_fingerprint != report.source.assignment_fingerprint
            || report.source.assignment_fingerprint.is_none()
            || report.source.raw_source_fingerprint != Some(raw_source)
            || self.raw_source != Some(raw_source)
            || self.height_generation != Some(report.source.height_generation)
            || self.volumes.source.as_ref() != Some(&report.source)
            || self.volumes.reports.get(&report.region_id) != Some(report)
            || self.manager.petal_id_of(node) != Some(petal.as_str())
            || !EarthworkTerrainBinding::valid_scale(report.source.world_scale, None)
            || report.expected_cells == 0
            || report.expected_cells > 4_000_000
            || report.sampled_cells != report.expected_cells
            || report.pair.is_none_or(|pair| {
                !pair.cut_m3.is_finite()
                    || !pair.fill_m3.is_finite()
                    || pair.cut_m3 < 0.0
                    || pair.fill_m3 < 0.0
            })
        {
            return None;
        }
        Some((node, raw_source))
    }

    fn demand(&self, report: &EarthworkVolumeReport) -> Option<Demand> {
        let (node, raw_source) = self.admitted(report)?;
        Some(Demand {
            scope: self.nav.active_scope()?,
            navigation: [
                self.nav.active_verse_id.clone()?,
                self.nav.active_fractal_id.clone()?,
                self.nav.active_petal_id.clone()?,
            ],
            node: node.into(),
            report: report.clone(),
            raw_source,
            incarnation: self.manager.scene_incarnation(),
            access: self.manager.scene_access_generation(),
            node_source: self.manager.scene_admission(node, &report.source.petal_id),
            node_epoch: self.manager.node_cache_epochs.get(node).copied(),
        })
    }

    fn same_measurement(&self, demand: &Demand) -> bool {
        self.volumes.reports.get(&demand.report.region_id) == Some(&demand.report)
            && self
                .map
                .node_for(&demand.report.source.petal_id, &demand.report.region_id)
                == Some(demand.node.as_str())
            && self.raw_source == Some(demand.raw_source)
    }

    fn matches(&self, demand: &Demand) -> bool {
        self.admitted(&demand.report) == Some((demand.node.as_str(), demand.raw_source))
            && [
                self.nav.active_verse_id.as_deref(),
                self.nav.active_fractal_id.as_deref(),
                self.nav.active_petal_id.as_deref(),
            ] == demand
                .navigation
                .each_ref()
                .map(|value| Some(value.as_str()))
            && self.manager.scene_incarnation() == demand.incarnation
            && self.manager.scene_access_generation() == demand.access
    }
}

fn capture_matches(capture: &EditorBatchCapture, demand: &Demand) -> bool {
    let Some(member) = capture.members.first() else {
        return false;
    };
    crate::editor_batch::validate_capture(
        capture,
        &demand.scope,
        &demand.report.source.petal_id,
        std::slice::from_ref(&demand.node),
    )
    .is_ok()
        && capture.target == EditorBatchTarget::Nodes
        && capture.deleted_group.is_none()
        && capture.scope == demand.scope
        && capture.petal_id == demand.report.source.petal_id
        && capture.terrain_source_fingerprint == Some(demand.raw_source)
        && !capture.petal_snapshot.is_empty()
        && capture.members.len() == 1
        && member.node_id == demand.node
        && member.state.node_id == demand.node
        && member.state.petal_id == capture.petal_id
        && member.state.tombstone.is_none()
        && member.history_version.is_none_or(|version| version > 0)
        && member.stamp_path.is_none()
        && member
            .state
            .properties
            .get("node_kind")
            .and_then(|v| v.as_str())
            == Some(EARTHWORK_NODE_KIND)
        && member
            .state
            .properties
            .get("region_id")
            .and_then(|v| v.as_str())
            == Some(demand.report.region_id.as_str())
        && !member.state.properties.contains_key("gpx_points")
        && !member
            .state
            .properties
            .contains_key(fe_sdk::path_asset::PATH_ASSET_PROPERTY_KEY)
}

fn current_pair(capture: &EditorBatchCapture, demand: &Demand) -> bool {
    let Some(pair) = demand.report.pair else {
        return false;
    };
    capture.members.first().is_some_and(|member| {
        member
            .state
            .properties
            .get(KEY_CUT_VOLUME)
            .and_then(|v| v.as_f64())
            == Some(pair.cut_m3)
            && member
                .state
                .properties
                .get(KEY_FILL_VOLUME)
                .and_then(|v| v.as_f64())
                == Some(pair.fill_m3)
    })
}

fn request(write: &Write) -> EditorBatchRequest {
    let pair = write.demand.report.pair.expect("complete captured demand");
    EditorBatchRequest {
        capture: write.capture.clone(),
        edit: EditorBatchEdit::Targets(vec![EditorTargetEdit {
            node_id: write.demand.node.clone(),
            mutation: EditorTargetMutation::Properties(vec![
                EditorPropertyEdit::Set {
                    key: KEY_CUT_VOLUME.into(),
                    value: serde_json::json!(pair.cut_m3),
                },
                EditorPropertyEdit::Set {
                    key: KEY_FILL_VOLUME.into(),
                    value: serde_json::json!(pair.fill_m3),
                },
            ]),
        }]),
    }
}

fn receipt_matches(receipt: &EditorBatchReceipt, write: &Write) -> bool {
    if receipt.request_id != write.id
        || receipt.target != EditorBatchTarget::Nodes
        || receipt.scope != write.capture.scope
        || receipt.petal_id != write.capture.petal_id
        || receipt.members.len() != 1
        || receipt.created_group_id.is_some()
        || serde_json::to_value(&receipt.groups).ok()
            != serde_json::to_value(&write.capture.groups).ok()
    {
        return false;
    }
    let original = &write.capture.members[0];
    let saved = &receipt.members[0];
    let mut expected = original.state.clone();
    let pair = write.demand.report.pair.expect("complete demand");
    expected
        .properties
        .insert(KEY_CUT_VOLUME.into(), serde_json::json!(pair.cut_m3));
    expected
        .properties
        .insert(KEY_FILL_VOLUME.into(), serde_json::json!(pair.fill_m3));
    saved.node_id == original.node_id
        && saved.stamp_path.is_none()
        && saved.history_version.is_some_and(|version| {
            version > 0
                && original
                    .history_version
                    .is_none_or(|before| version > before)
        })
        && serde_json::to_value(&saved.state).ok() == serde_json::to_value(expected).ok()
        && serde_json::to_vec(receipt).is_ok_and(|bytes| bytes.len() <= MAX_EDITOR_BATCH_BYTES)
}

impl VolumePersistence {
    fn finish(
        &mut self,
        receipt: &EditorBatchReceipt,
        current: Option<&EditorBatchCapture>,
        projected: bool,
        live: &Live<'_>,
    ) {
        let Some(write) = self.write.as_ref() else {
            return;
        };
        if !receipt_matches(receipt, write) {
            self.status.insert(
                write.demand.node.clone(),
                "Volume save outcome is unknown: unexpected receipt. No write was retried.",
            );
            return;
        }
        let version = receipt.members[0]
            .history_version
            .expect("validated committed version");
        let node_source = live
            .manager
            .scene_admission(&write.demand.node, &receipt.petal_id);
        let node_epoch = live
            .manager
            .node_cache_epochs
            .get(&write.demand.node)
            .copied();
        let still_current = projected
            && !write.retired
            && live.matches(&write.demand)
            && current.is_none_or(|current| {
                capture_matches(current, &write.demand)
                    && serde_json::to_value(&current.members).ok()
                        == serde_json::to_value(&receipt.members).ok()
            });
        let write = self.write.take().expect("owned write");
        if still_current {
            self.confirmed.insert(
                write.demand.node.clone(),
                Confirmed {
                    demand: write.demand.clone(),
                    version,
                    node_source,
                    node_epoch,
                },
            );
            self.status.insert(
                write.demand.node.clone(),
                "Complete current terrain volume saved as one atomic cut/fill pair.",
            );
        } else {
            self.status.insert(write.demand.node.clone(), "Earlier volume save completed; its terrain context changed. Current volume remains unavailable until recalculated.");
        }
        self.handled
            .insert(write.demand.node.clone(), write.demand.clone());
    }

    pub(crate) fn route(&mut self, result: &DbResult, live: &Live<'_>, now: f64, projected: bool) {
        match result {
            DbResult::EditorBatchLoaded {
                capture,
                correlation_id,
            } if self
                .read
                .as_ref()
                .is_some_and(|read| read.accepted && read.id == *correlation_id) =>
            {
                let read = self.read.take().expect("owned read");
                if !live.matches(&read.demand) {
                    return;
                }
                if !capture_matches(capture, &read.demand) {
                    self.handled
                        .insert(read.demand.node.clone(), read.demand.clone());
                    self.status.insert(read.demand.node, "Volume not saved: stored node or terrain source changed. No automatic field rebase.");
                    return;
                }
                if current_pair(capture, &read.demand) {
                    self.handled
                        .insert(read.demand.node.clone(), read.demand.clone());
                    self.status.insert(read.demand.node, "Complete terrain volume already matches the current stored pair; no save needed.");
                    return;
                }
                self.write = Some(Write {
                    id: format!("earthwork-volume-save-{}", ulid::Ulid::new()),
                    demand: read.demand,
                    capture: capture.clone(),
                    accepted: false,
                    retired: false,
                    at: now,
                    resolve: None,
                });
            }
            DbResult::EditorBatchCommitted {
                receipt,
                correlation_id,
            } if self
                .write
                .as_ref()
                .is_some_and(|write| write.accepted && write.id == *correlation_id) =>
            {
                self.finish(receipt, None, projected, live);
            }
            DbResult::EditorBatchResolved {
                resolution,
                correlation_id,
            } if self.write.as_ref().is_some_and(|write| {
                write.id == resolution.request_id
                    && write
                        .resolve
                        .as_ref()
                        .is_some_and(|(id, accepted, _)| *accepted && id == correlation_id)
            }) =>
            {
                if let Some(receipt) = &resolution.receipt {
                    self.finish(receipt, Some(&resolution.current), projected, live);
                } else {
                    let write = self.write.as_ref().expect("owned outcome");
                    if resolution.current.target != EditorBatchTarget::Nodes
                        || resolution.current.scope != write.capture.scope
                        || resolution.current.petal_id != write.capture.petal_id
                        || resolution.current.members.len() != 1
                        || resolution.current.members[0].node_id != write.demand.node
                    {
                        return;
                    }
                    let write = self.write.take().expect("owned outcome");
                    self.handled
                        .insert(write.demand.node.clone(), write.demand.clone());
                    self.status.insert(write.demand.node.clone(), "Volume save was not committed; no write was replayed. Reopen properties to read the current stored state.");
                }
            }
            DbResult::RequestFailed { correlation_id, .. } => {
                if self
                    .read
                    .as_ref()
                    .is_some_and(|read| read.id == *correlation_id)
                {
                    let read = self.read.take().expect("owned read");
                    self.status.insert(
                        read.demand.node,
                        "Volume capture unavailable; no measurement write was sent.",
                    );
                    self.next_read = now + RETRY_READ;
                } else if let Some(write) = self.write.as_mut() {
                    if write.id == *correlation_id {
                        write.at = now - TIMEOUT;
                        self.status.insert(write.demand.node.clone(), "Volume save outcome is unknown; checking the original request without replay.");
                    } else if write
                        .resolve
                        .as_ref()
                        .is_some_and(|(id, _, _)| id == correlation_id)
                    {
                        write.resolve = None;
                        write.at = now;
                        self.status.insert(write.demand.node.clone(), "Volume outcome check unavailable. Original accepted save remains owned; no replay.");
                    }
                }
            }
            _ => {}
        }
    }

    fn pump(&mut self, live: &Live<'_>, db: &crossbeam::channel::Sender<DbCommand>, now: f64) {
        self.handled
            .retain(|_, demand| live.same_measurement(demand));
        let statuses = &mut self.status;
        self.confirmed.retain(|node, saved| {
            let current = live.matches(&saved.demand)
                && live
                    .manager
                    .committed_versions
                    .get(node)
                    .is_none_or(|known| *known <= saved.version)
                && (live
                    .manager
                    .local_scene_admission(
                        node,
                        &saved.demand.report.source.petal_id,
                        saved.version,
                    )
                    .is_some()
                    || (live
                        .manager
                        .scene_admission(node, &saved.demand.report.source.petal_id)
                        == saved.node_source
                        && live.manager.node_cache_epochs.get(node).copied() == saved.node_epoch));
            if !current {
                statuses.remove(node);
            }
            current
        });
        for (node, demand) in &self.handled {
            if !self.confirmed.contains_key(node)
                && (live
                    .manager
                    .scene_admission(node, &demand.report.source.petal_id)
                    != demand.node_source
                    || live.manager.node_cache_epochs.get(node).copied() != demand.node_epoch)
            {
                self.status.insert(node.clone(), "Stored object changed after measurement readback; current volume confirmation is unavailable. No automatic field rebase.");
            }
        }
        self.status.retain(|node, _| {
            live.manager.petal_id_of(node) == live.nav.active_petal_id.as_deref()
                && live.volumes.reports.values().any(|report| {
                    live.map
                        .node_for(&report.source.petal_id, &report.region_id)
                        == Some(node.as_str())
                })
        });
        if let Some(write) = self.write.as_mut() {
            write.retired |= !live.matches(&write.demand);
            if write.retired && write.accepted {
                self.status.insert(write.demand.node.clone(), "Earlier volume save outcome remains pending; current terrain context is unavailable. No write replay.");
            }
            if !write.accepted && write.retired {
                self.write = None;
                return;
            }
            if !write.accepted {
                if now < write.at {
                    return;
                }
                // Scene write door: Waiting/Sent are owned by the door queue (its full-channel retry replaces the local backoff).
                match crate::actions::node::try_submit_scene_write_on(
                    db,
                    DbCommand::CommitCheckedEditorBatch {
                        request: request(write),
                        correlation_id: write.id.clone(),
                    },
                ) {
                    crate::actions::node::GuiSend::Sent
                    | crate::actions::node::GuiSend::Waiting => {
                        write.accepted = true;
                        write.at = now;
                        self.status.insert(write.demand.node.clone(), "Complete volume pair save pending; channel acceptance is not confirmation.");
                    }
                    crate::actions::node::GuiSend::Refused(_) => {
                        // Never sent: the door already queued the refusal advisory; drop the write and retry reads later.
                        self.status.insert(
                            write.demand.node.clone(),
                            "Volume was not saved: a scene hook refused the write.",
                        );
                        self.write = None;
                        self.next_read = now + RETRY_READ;
                    }
                    crate::actions::node::GuiSend::Closed => {
                        self.status.insert(
                            write.demand.node.clone(),
                            "Volume was not saved: DB channel disconnected before acceptance.",
                        );
                        self.write = None;
                        self.next_read = now + RETRY_READ;
                    }
                }
                return;
            }
            if !live.running {
                return;
            }
            if now - write.at >= TIMEOUT {
                if write
                    .resolve
                    .as_ref()
                    .is_some_and(|(_, accepted, at)| *accepted && now - at >= TIMEOUT)
                {
                    write.resolve = None;
                }
                let resolve = write.resolve.get_or_insert_with(|| {
                    (
                        format!("earthwork-volume-resolve-{}", ulid::Ulid::new()),
                        false,
                        now,
                    )
                });
                if !resolve.1 {
                    match db.try_send(DbCommand::ResolveCheckedEditorBatch {
                        target: EditorBatchTarget::Nodes,
                        scope: write.capture.scope.clone(),
                        petal_id: write.capture.petal_id.clone(),
                        node_ids: vec![write.demand.node.clone()],
                        request_id: write.id.clone(),
                        correlation_id: resolve.0.clone(),
                    }) {
                        Ok(()) => {
                            resolve.1 = true;
                            resolve.2 = now;
                        }
                        Err(crossbeam::channel::TrySendError::Full(_)) => {}
                        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                            write.at = now;
                            bevy::log::warn!(
                                "DB channel disconnected: accepted volume save remains unknown"
                            );
                        }
                    }
                }
            }
            return;
        }
        if self.read.as_ref().is_some_and(|read| {
            !live.matches(&read.demand) || read.accepted && now - read.at >= TIMEOUT
        }) {
            self.read = None;
        }
        if self.read.is_none() && now >= self.next_read {
            for report in live.volumes.reports.values().take(MAX_EDITOR_BATCH_TARGETS) {
                let Some(node) = live
                    .map
                    .node_for(&report.source.petal_id, &report.region_id)
                else {
                    continue;
                };
                if live.admitted(report).is_none() {
                    self.status.insert(node.into(), "Volume unavailable: missing, partial, nonfinite or stale terrain coverage/source. Stored values remain unchanged.");
                    continue;
                }
                if self
                    .handled
                    .get(node)
                    .is_some_and(|handled| live.same_measurement(handled))
                {
                    continue;
                }
                let Some(demand) = live.demand(report) else {
                    continue;
                };
                self.read = Some(Read {
                    id: format!("earthwork-volume-read-{}", ulid::Ulid::new()),
                    demand,
                    accepted: false,
                    at: now,
                });
                break;
            }
        }
        if let Some(read) = self.read.as_mut() {
            if !read.accepted {
                if now < read.at {
                    return;
                }
                match db.try_send(DbCommand::ReadEditorBatch {
                    target: EditorBatchTarget::Nodes,
                    scope: read.demand.scope.clone(),
                    petal_id: read.demand.report.source.petal_id.clone(),
                    node_ids: vec![read.demand.node.clone()],
                    correlation_id: read.id.clone(),
                }) {
                    Ok(()) => {
                        read.accepted = true;
                        read.at = now;
                        self.status.insert(read.demand.node.clone(), "Complete terrain measurement ready; checking current stored source before saving.");
                    }
                    Err(crossbeam::channel::TrySendError::Full(_)) => {
                        read.at = now + 0.5;
                    }
                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                        self.status.insert(read.demand.node.clone(), "Volume capture unavailable: DB channel disconnected; no write accepted.");
                        self.read = None;
                        self.next_read = now + RETRY_READ;
                    }
                }
            }
        }
    }
}

pub(crate) fn persist(
    mut owner: ResMut<VolumePersistence>,
    mut ui: ResMut<UiManager>,
    map: Res<EarthworkNodeMap>,
    terrain: Res<PetalMapState>,
    volumes: Res<EarthworkVolumeState>,
    binding: Res<EarthworkTerrainBinding>,
    manager: Res<VerseManager>,
    nav: Res<NavigationManager>,
    role: Res<LocalUserRole>,
    db: Res<DbCommandSender>,
    time: Res<Time<Real>>,
    terminal: Option<Res<fe_runtime::maintenance::TerminalMode>>,
    height: Option<Res<fe_renderer::terrain_height::TerrainHeightField>>,
) {
    if terminal.as_ref().is_some_and(|mode| mode.is_terminal()) {
        owner.retire_for_terminal_reset();
        ui.earthwork_volumes = VolumeView::default();
        return;
    }
    let live = Live {
        nav: &nav,
        map: &map,
        terrain: &terrain,
        volumes: &volumes,
        binding: &binding,
        manager: &manager,
        role: &role,
        raw_source: terrain.confirmed_terrain_fingerprint(),
        running: terminal.is_none_or(|mode| !mode.is_terminal()),
        height_generation: height.as_ref().map(|field| field.generation()),
    };
    let now = time.elapsed_secs_f64();
    owner.pump(&live, &db.0, now);
    owner.update_view(&mut ui.earthwork_volumes);
}

#[cfg(test)]
#[path = "earthwork_volumes_journey.rs"]
pub(crate) mod tests;
