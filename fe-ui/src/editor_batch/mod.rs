//! Checked selection editing and bounded history; see editor_batch/AGENTS.md.

mod interaction;
pub(crate) use interaction::{SelectionInteraction, TransformRoute};
mod source;
pub(crate) use source::{RenderedSources, SelectionSources};
pub(crate) mod math;
pub(crate) mod ui;
mod validation;
pub(crate) use math::affine_edits;
pub(crate) use ui::render;

use crate::{
    navigation_manager::NavigationManager, node_manager::NodeManager, verse_manager::VerseManager,
};
use fe_runtime::{
    editor_batch::*,
    messages::{DbCommand, DbResult},
};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HISTORY: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionKey {
    scope: String,
    petal_id: String,
    ids: Vec<String>,
    generation: u64,
    navigation: [String; 3],
}
impl SelectionKey {
    fn matches_navigation(&self, nav: &NavigationManager) -> bool {
        [
            nav.active_verse_id.as_deref(),
            nav.active_fractal_id.as_deref(),
            nav.active_petal_id.as_deref(),
        ] == self.navigation.each_ref().map(|value| Some(value.as_str()))
    }
    fn matches(&self, nav: &NavigationManager, nodes: &NodeManager) -> bool {
        self.matches_navigation(nav)
            && self.generation == nodes.generation()
            && self.ids.len() == nodes.selection_count()
            && nodes.selection_is_settled()
            && nodes
                .selected_petal()
                .is_none_or(|petal| petal == self.petal_id)
    }

    fn current(nav: &NavigationManager, nodes: &NodeManager) -> Option<Self> {
        let [verse, fractal, petal] = [
            nav.active_verse_id.as_deref()?,
            nav.active_fractal_id.as_deref()?,
            nav.active_petal_id.as_deref()?,
        ];
        if !nodes.selection_is_settled()
            || nodes.selection_count() == 0
            || nodes
                .selected_petal()
                .is_some_and(|selected| selected != petal)
        {
            return None;
        }
        Some(Self {
            scope: fe_database::build_scope(verse, Some(fractal), Some(petal)),
            petal_id: petal.into(),
            ids: nodes
                .members()
                .map(|member| member.node_id.clone())
                .collect(),
            generation: nodes.generation(),
            navigation: [verse.to_owned(), fractal.to_owned(), petal.to_owned()],
        })
    }
}

#[derive(Debug, Clone)]
pub struct BatchOrigin {
    key: SelectionKey,
    pub capture: EditorBatchCapture,
    pub(crate) interaction: Arc<SelectionInteraction>,
}
impl BatchOrigin {
    pub(crate) fn matches(&self, nav: &NavigationManager, nodes: &NodeManager) -> bool {
        self.key.matches(nav, nodes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryDirection {
    New,
    Undo,
    Redo,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    before: EditorBatchCapture,
    after: EditorBatchCapture,
    forward: EditorBatchEdit,
    order: u64,
}

#[derive(Debug)]
struct ReadRequest {
    id: String,
    key: SelectionKey,
    replay: Option<(HistoryEntry, HistoryDirection)>,
    accepted: bool,
    started: Instant,
}

#[derive(Debug)]
struct PendingWrite {
    id: String,
    request: EditorBatchRequest,
    replay: Option<HistoryEntry>,
    direction: HistoryDirection,
    resolve_id: Option<String>,
    resolve_queued: bool,
    started: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct BatchProjection {
    pub petal_id: String,
    pub members: Vec<EditorBatchMember>,
    pub groups: Vec<fe_runtime::messages::VisibilityGroupDto>,
    pub source: ProjectionSource,
    pub draft_before: Vec<(String, serde_json::Value)>,
}

impl BatchProjection {
    pub(crate) fn editor_policy(&self, node_id: &str) -> crate::undo::PathEditorPolicy {
        crate::undo::PathEditorPolicy::PreserveChanged {
            before: self
                .draft_before
                .iter()
                .find(|(id, _)| id == node_id)
                .map(|(_, points)| points.clone()),
        }
    }

    pub(crate) fn protect_draft(&self, path: &mut crate::gis::PathEditorState) -> bool {
        if path.points_pending && path.points.is_empty() {
            return false;
        }
        let Some(member) = self
            .members
            .iter()
            .find(|member| path.editing_track_id.as_deref() == Some(member.node_id.as_str()))
        else {
            return false;
        };
        let Some(points) = member.state.properties.get("gpx_points") else {
            return false;
        };
        if self
            .editor_policy(&member.node_id)
            .preserves(&path.points, &crate::gis::decode_gpx_points(points))
        {
            crate::undo::retain_local_draft(path);
            return true;
        }
        false
    }
}

fn draft_sources(capture: &EditorBatchCapture) -> Vec<(String, serde_json::Value)> {
    capture
        .members
        .iter()
        .filter_map(|member| {
            member
                .state
                .properties
                .get("gpx_points")
                .map(|points| (member.node_id.clone(), points.clone()))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionSource {
    Commit,
    ExternalCommit,
    CurrentRead,
    UncommittedRead,
    LegacyRead,
}

#[derive(Default)]
pub(crate) struct EditorBatchState {
    key: Option<SelectionKey>,
    history_scope: Option<[String; 3]>,
    invalidate_legacy_redo: bool,
    legacy_generation: u64,
    capture: Option<Arc<EditorBatchCapture>>,
    interaction: Option<Arc<SelectionInteraction>>,
    legacy_hydration: VecDeque<String>,
    read: Option<ReadRequest>,
    pending: Option<PendingWrite>,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    path_projections: VecDeque<crate::undo::PathProjection>,
    pub error: Option<String>,
    pub draft: ui::SelectionDraft,
}

fn id() -> String {
    format!("ui-editor-batch-{}", ulid::Ulid::new())
}

fn snapshot(receipt: &EditorBatchReceipt, original: &EditorBatchCapture) -> EditorBatchCapture {
    EditorBatchCapture {
        scope: receipt.scope.clone(),
        petal_id: receipt.petal_id.clone(),
        members: receipt.members.clone(),
        groups: receipt.groups.clone(),
        petal_snapshot: original.petal_snapshot.clone(),
        world_scale: original.world_scale,
    }
}

impl EditorBatchState {
    pub(crate) fn synchronize_history(&mut self, undo: &mut crate::undo::UndoState) {
        if self.invalidate_legacy_redo {
            undo.invalidate_redo();
            self.invalidate_legacy_redo = false;
        }
        if self.legacy_generation != undo.normal_generation() {
            self.redo.clear();
            self.legacy_generation = undo.normal_generation();
        }
    }

    pub(crate) fn owns(&self, result: &DbResult) -> bool {
        match result {
            DbResult::EditorBatchLoaded { correlation_id, .. } => self
                .read
                .as_ref()
                .is_some_and(|read| read.id == *correlation_id),
            DbResult::EditorBatchCommitted { correlation_id, .. } => self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == *correlation_id),
            DbResult::EditorBatchResolved { correlation_id, .. } => self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.resolve_id.as_ref() == Some(correlation_id)),
            DbResult::RequestFailed { correlation_id, .. } => {
                self.read
                    .as_ref()
                    .is_some_and(|read| read.id == *correlation_id)
                    || self.pending.as_ref().is_some_and(|pending| {
                        pending.id == *correlation_id
                            || pending.resolve_id.as_ref() == Some(correlation_id)
                    })
            }
            _ => false,
        }
    }

    pub(crate) fn queue_projection(&mut self, projection: &BatchProjection, scene_incarnation: u128) {
        if projection.source == ProjectionSource::LegacyRead {
            return;
        }
        for member in &projection.members {
            let Some(history_version) = member.history_version.filter(|version| *version > 0)
            else {
                continue;
            };
            if let Some(points) = member.state.properties.get("gpx_points") {
                self.invalidate_path_projection(&member.node_id);
                self.path_projections
                    .push_back(crate::undo::PathProjection {
                        node_id: member.node_id.clone(),
                        petal_id: projection.petal_id.clone(),
                        points: points.clone(),
                        history_version,
                        scene_incarnation,
                        editor_policy: projection.editor_policy(&member.node_id),
                    });
            }
        }
    }

    pub(crate) fn invalidate_path_projection(&mut self, node_id: &str) {
        self.path_projections
            .retain(|projection| projection.node_id != node_id);
    }

    pub(crate) fn take_path_projection(
        &mut self,
        nav: &NavigationManager,
    ) -> Option<crate::undo::PathProjection> {
        while let Some(projection) = self.path_projections.pop_front() {
            if nav.active_petal_id.as_deref() == Some(projection.petal_id.as_str()) {
                return Some(projection);
            }
        }
        None
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some() || self.read.as_ref().is_some_and(|read| read.replay.is_some())
    }

    pub(crate) fn capture(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
    ) -> Option<&EditorBatchCapture> {
        self.key
            .as_ref()
            .filter(|key| key.matches(nav, nodes))
            .and(self.capture.as_deref())
    }
    pub(crate) fn capture_shared(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
    ) -> Option<Arc<EditorBatchCapture>> {
        self.key
            .as_ref()
            .filter(|key| key.matches(nav, nodes))
            .and(self.capture.as_ref())
            .cloned()
    }

    pub(crate) fn interaction(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
    ) -> Option<&SelectionInteraction> {
        self.key
            .as_ref()
            .filter(|key| key.matches(nav, nodes))
            .and(self.interaction.as_deref())
    }

    pub(crate) fn interaction_shared(
        &self,
        nav: &NavigationManager,
        nodes: &NodeManager,
    ) -> Option<Arc<SelectionInteraction>> {
        self.key
            .as_ref()
            .filter(|key| key.matches(nav, nodes))
            .and(self.interaction.as_ref())
            .cloned()
    }

    pub(crate) fn queue_legacy_hydration(&mut self, members: &[EditorBatchMember]) {
        for member in members {
            if self.legacy_hydration.len() < MAX_EDITOR_BATCH_TARGETS
                && !self.legacy_hydration.contains(&member.node_id)
            {
                self.legacy_hydration.push_back(member.node_id.clone());
            }
        }
    }

    pub(crate) fn invalidate_preview_capture(&mut self, node_id: &str) {
        if self
            .key
            .as_ref()
            .is_some_and(|key| key.ids.iter().any(|id| id == node_id))
            || self.capture.as_ref().is_some_and(|capture| {
                capture.members.iter().any(|member| {
                    member
                        .stamp_path
                        .as_ref()
                        .is_some_and(|path| path.state.node_id == node_id)
                })
            })
        {
            self.capture = None;
            self.interaction = None;
            self.read = None;
            self.error = Some("An external transform preview changed the selection; refresh after its save outcome is known".into());
        }
    }

    pub(crate) fn origin(&self, nodes: &NodeManager) -> Result<BatchOrigin, String> {
        let key = self.key.as_ref().ok_or("Select objects in an open Petal")?;
        if !nodes.selection_is_settled()
            || key.generation != nodes.generation()
            || key.ids
                != nodes
                    .members()
                    .map(|member| member.node_id.clone())
                    .collect::<Vec<_>>()
        {
            return Err("Selection changed; review the current selection before applying".into());
        }
        if self.is_pending() {
            return Err("Wait for the current selection edit to finish saving".into());
        }
        Ok(BatchOrigin {
            key: key.clone(),
            interaction: self
                .interaction
                .clone()
                .ok_or("Loading selected-object interaction geometry")?,
            capture: self.capture.as_deref().cloned().ok_or_else(|| {
                self.error
                    .clone()
                    .unwrap_or("Loading the selected objects' stored state".into())
            })?,
        })
    }

    pub(crate) fn maintain(
        &mut self,
        nav: &NavigationManager,
        nodes: &NodeManager,
        hierarchy: &VerseManager,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) {
        for _ in 0..8 {
            let Some(node_id) = self.legacy_hydration.front() else {
                break;
            };
            match db.try_send(DbCommand::GetNodeProperties {
                node_id: node_id.clone(),
                correlation_id: None,
            }) {
                Ok(()) => {
                    self.legacy_hydration.pop_front();
                }
                Err(crossbeam::channel::TrySendError::Full(_)) => break,
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    bevy::log::warn!(
                        "DB channel closed while reloading reconciled legacy selection"
                    );
                    self.legacy_hydration.clear();
                    break;
                }
            }
        }
        let navigation = nav
            .active_verse_id
            .as_deref()
            .zip(nav.active_fractal_id.as_deref())
            .zip(nav.active_petal_id.as_deref())
            .map(|((verse, fractal), petal)| [verse, fractal, petal]);
        if navigation
            != self
                .history_scope
                .as_ref()
                .map(|ids| ids.each_ref().map(String::as_str))
        {
            self.undo.clear();
            self.redo.clear();
            self.history_scope = navigation.map(|ids| ids.map(str::to_owned));
            if self.read.as_ref().is_some_and(|read| read.replay.is_some()) {
                self.read = None;
            }
        }
        let settled = nodes.selection_is_settled()
            && nodes.selection_count() > 0
            && navigation.is_some()
            && nodes
                .selected_petal()
                .is_none_or(|petal| nav.active_petal_id.as_deref() == Some(petal));
        let changed = self
            .key
            .as_ref()
            .map_or(settled, |key| !key.matches(nav, nodes));
        let stale = self.capture.as_ref().is_some_and(|capture| {
            capture.members.iter().any(|member| {
                hierarchy
                    .committed_versions
                    .get(&member.node_id)
                    .is_some_and(|version| member.history_version != Some(*version))
                    || member.stamp_path.as_ref().is_some_and(|path| {
                        hierarchy
                            .committed_versions
                            .get(&path.state.node_id)
                            .is_some_and(|version| path.history_version != Some(*version))
                    })
            })
        });
        if changed || stale {
            if changed {
                self.key = SelectionKey::current(nav, nodes);
                self.draft = Default::default();
            }
            self.capture = None;
            self.interaction = None;
            self.error = None;
            if self.read.as_ref().is_none_or(|read| read.replay.is_none()) {
                self.read = None;
            }
        }
        if self.pending.as_ref().is_some_and(|pending| {
            pending.started.elapsed() >= TIMEOUT && pending.resolve_id.is_none()
        }) {
            if let Err(error) = self.resolve(db) {
                self.error = Some(error);
            }
        }
        if self
            .read
            .as_ref()
            .is_some_and(|read| read.accepted && read.started.elapsed() >= TIMEOUT)
        {
            self.error = Some(
                "Selected-object read timed out. Refresh to retry; no edit was submitted.".into(),
            );
            self.read = None;
        }
        if self.capture.is_none()
            && self.read.is_none()
            && self.pending.is_none()
            && self.error.is_none()
        {
            if let Some(key) = self.key.clone() {
                if key
                    .ids
                    .iter()
                    .any(|id| crate::verse_manager::parse_stamp_marker_id(id).is_some())
                {
                    self.error =
                        Some("Promote every selected stamp before editing this selection".into());
                    return;
                }
                self.read = Some(ReadRequest {
                    id: id(),
                    key,
                    replay: None,
                    accepted: false,
                    started: Instant::now(),
                });
            }
        }
        if let Err(error) = self.flush_read(db) {
            self.error = Some(error);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.resolve_queued)
        {
            if let Err(error) = self.flush_resolution(db) {
                self.error = Some(error);
            }
        }
    }

    fn flush_read(&mut self, db: &crossbeam::channel::Sender<DbCommand>) -> Result<(), String> {
        let Some(read) = self.read.as_mut().filter(|read| !read.accepted) else {
            return Ok(());
        };
        match db.try_send(DbCommand::ReadEditorBatch {
            scope: read.key.scope.clone(),
            petal_id: read.key.petal_id.clone(),
            node_ids: read.key.ids.clone(),
            correlation_id: read.id.clone(),
        }) {
            Ok(()) => {
                read.accepted = true;
                read.started = Instant::now();
                Ok(())
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => Ok(()),
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                self.read = None;
                Err("Database connection closed; selected objects could not be read and no edit was submitted".into())
            }
        }
    }

    pub(crate) fn report_error(&mut self, error: &str) {
        self.error = Some(error.to_owned());
        self.capture = None;
        self.interaction = None;
    }

    pub(crate) fn refresh(&mut self) {
        if !self.is_pending() {
            self.error = None;
            self.capture = None;
            self.interaction = None;
            self.read = None;
        }
    }

    pub(crate) fn start(
        &mut self,
        origin: BatchOrigin,
        edit: EditorBatchEdit,
        nav: &NavigationManager,
        nodes: &NodeManager,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        if self.is_pending() {
            return Err("Wait for the current selection edit to finish saving".into());
        }
        if !origin.key.matches(nav, nodes) {
            return Err("Selection or Petal changed; the edit was not sent".into());
        }
        validation::capture(
            &origin.capture,
            &origin.key.scope,
            &origin.key.petal_id,
            &origin.key.ids,
        )?;
        if serde_json::to_vec(&origin.capture)
            .map_err(|_| "Selection is unavailable")?
            .len()
            * 2
            > MAX_EDITOR_BATCH_BYTES
        {
            return Err("Selection exceeds the bounded Undo budget; select fewer objects".into());
        }
        self.submit(
            EditorBatchRequest {
                capture: origin.capture,
                edit,
            },
            None,
            HistoryDirection::New,
            db,
        )
    }

    fn submit(
        &mut self,
        request: EditorBatchRequest,
        replay: Option<HistoryEntry>,
        direction: HistoryDirection,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        validation::edit(&request)?;
        let request_id = id();
        db.try_send(DbCommand::CommitCheckedEditorBatch {
            request: request.clone(),
            correlation_id: request_id.clone(),
        })
        .map_err(|error| match error {
            crossbeam::channel::TrySendError::Full(_) => {
                "Database queue is busy; no selection edit was queued. Apply again when ready."
            }
            crossbeam::channel::TrySendError::Disconnected(_) => {
                "Database connection closed; no selection edit was queued"
            }
        })?;
        self.pending = Some(PendingWrite {
            id: request_id,
            request,
            replay,
            direction,
            resolve_id: None,
            resolve_queued: false,
            started: Instant::now(),
        });
        self.capture = None;
        self.interaction = None;
        self.read = None;
        self.error = None;
        Ok(())
    }

    pub(crate) fn resolve(
        &mut self,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        let Some(pending) = self.pending.as_mut() else {
            return Err("There is no submitted selection save to check; an Undo read may still be preparing its edit".into());
        };
        if pending.resolve_id.is_some()
            && !pending.resolve_queued
            && pending.started.elapsed() < TIMEOUT
        {
            return Ok(());
        }
        if !pending.resolve_queued {
            pending.resolve_id = None;
        }
        pending.resolve_queued = true;
        self.flush_resolution(db)
    }

    fn flush_resolution(
        &mut self,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.resolve_queued)
        else {
            return Ok(());
        };
        let resolve_id = pending.resolve_id.get_or_insert_with(id).clone();
        match db.try_send(DbCommand::ResolveCheckedEditorBatch {
            scope: pending.request.capture.scope.clone(),
            petal_id: pending.request.capture.petal_id.clone(),
            node_ids: pending
                .request
                .capture
                .members
                .iter()
                .map(|member| member.node_id.clone())
                .collect(),
            request_id: pending.id.clone(),
            correlation_id: resolve_id,
        }) {
            Ok(()) => {
                pending.resolve_queued = false;
                pending.started = Instant::now();
                Ok(())
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => Ok(()),
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                pending.resolve_queued = false;
                pending.resolve_id = None;
                Err("Saved selection outcome is unknown; database connection closed. No write was retried.".into())
            }
        }
    }

    pub(crate) fn newest(&self, redo: bool) -> Option<u64> {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
        .map(|entry| entry.order)
    }
    pub(crate) fn invalidate_redo(&mut self) {
        self.redo.clear();
    }

    pub(crate) fn replay(
        &mut self,
        redo: bool,
        nav: &NavigationManager,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        if self.is_pending() {
            return Err(
                "Wait for the pending selection edit or reconciliation; no Undo was sent".into(),
            );
        }
        let entry = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
        .cloned()
        .ok_or(if redo {
            "Nothing to redo"
        } else {
            "Nothing to undo"
        })?;
        let petal = nav
            .active_petal_id
            .as_deref()
            .ok_or("Open the edit's Petal before Undo")?;
        let scope = fe_database::build_scope(
            nav.active_verse_id
                .as_deref()
                .ok_or("Open the edit's Verse")?,
            nav.active_fractal_id.as_deref(),
            Some(petal),
        );
        if scope != entry.after.scope || petal != entry.after.petal_id {
            return Err("This Undo belongs to another Petal".into());
        }
        let ids = entry
            .after
            .members
            .iter()
            .map(|member| member.node_id.clone())
            .collect::<Vec<_>>();
        let request_id = id();
        self.read = Some(ReadRequest {
            id: request_id,
            key: SelectionKey {
                scope,
                petal_id: petal.into(),
                ids,
                generation: 0,
                navigation: [
                    nav.active_verse_id.clone().ok_or("Verse closed")?,
                    nav.active_fractal_id.clone().ok_or("Fractal closed")?,
                    petal.to_owned(),
                ],
            },
            accepted: false,
            replay: Some((
                entry,
                if redo {
                    HistoryDirection::Redo
                } else {
                    HistoryDirection::Undo
                },
            )),
            started: Instant::now(),
        });
        self.flush_read(db)
    }

    pub(crate) fn route(
        &mut self,
        result: &DbResult,
        nav: &NavigationManager,
        preflight: impl Fn(&BatchProjection) -> Result<(), String>,
        admit_inverse: impl Fn(&EditorBatchCapture) -> Result<(), String>,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<Option<BatchProjection>, String> {
        match result {
            DbResult::EditorBatchLoaded {
                capture,
                correlation_id,
            } if self
                .read
                .as_ref()
                .is_some_and(|read| read.id == *correlation_id) =>
            {
                let read = self.read.take().unwrap();
                if !read.key.matches_navigation(nav) {
                    return Err("Petal changed; the selected-object read or Undo was canceled before any write".into());
                }
                validation::capture(capture, &read.key.scope, &read.key.petal_id, &read.key.ids)?;
                if let Some((entry, direction)) = read.replay {
                    let expected = if direction == HistoryDirection::Undo {
                        &entry.after
                    } else {
                        &entry.before
                    };
                    if !validation::same_state(capture, expected) {
                        return Err("One or more selected objects or group memberships changed; the complete Undo was refused".into());
                    }
                    let edit = validation::replay_edit(&entry, capture, direction)?;
                    admit_inverse(capture)?;
                    self.submit(
                        EditorBatchRequest {
                            capture: capture.clone(),
                            edit,
                        },
                        Some(entry),
                        direction,
                        db,
                    )?;
                } else if self.key.as_ref() == Some(&read.key) {
                    self.interaction = Some(Arc::new(SelectionInteraction::capture(capture)));
                    self.capture = Some(Arc::new(capture.clone()));
                    self.error = None;
                }
                Ok(None)
            }
            DbResult::EditorBatchCommitted {
                receipt,
                correlation_id,
            } if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == *correlation_id) =>
            {
                self.finish(receipt, None, &preflight)
            }
            DbResult::EditorBatchResolved {
                resolution,
                correlation_id,
            } if self.pending.as_ref().is_some_and(|pending| {
                pending.id == resolution.request_id
                    && pending.resolve_id.as_ref() == Some(correlation_id)
            }) =>
            {
                let pending = self.pending.as_ref().unwrap();
                let ids = pending
                    .request
                    .capture
                    .members
                    .iter()
                    .map(|member| member.node_id.clone())
                    .collect::<Vec<_>>();
                validation::capture(
                    &resolution.current,
                    &pending.request.capture.scope,
                    &pending.request.capture.petal_id,
                    &ids,
                )?;
                if let Some(receipt) = &resolution.receipt {
                    self.finish(receipt, Some(&resolution.current), &preflight)
                } else {
                    let unchanged =
                        validation::same_state(&resolution.current, &pending.request.capture);
                    if unchanged {
                        self.pending = None;
                        self.capture = None;
                        self.interaction = None;
                        self.error = Some("Selection edit was not committed; the captured stored state is unchanged.".into());
                        return Err(self.error.clone().unwrap());
                    }
                    let source = if resolution
                        .current
                        .members
                        .iter()
                        .any(|member| member.history_version.is_none())
                    {
                        ProjectionSource::LegacyRead
                    } else {
                        ProjectionSource::UncommittedRead
                    };
                    let projection = BatchProjection {
                        petal_id: resolution.current.petal_id.clone(),
                        members: resolution.current.members.clone(),
                        groups: resolution.current.groups.clone(),
                        draft_before: draft_sources(&pending.request.capture),
                        source,
                    };
                    preflight(&projection)?;
                    self.pending = None;
                    self.capture = None;
                    self.interaction = None;
                    self.error = Some(
                        "Selection edit was not committed. Current stored objects were restored."
                            .into(),
                    );
                    Ok(Some(projection))
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == *correlation_id) =>
            {
                self.error = Some(error.clone());
                self.resolve(db)?;
                Err(error.clone())
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if self
                .read
                .as_ref()
                .is_some_and(|read| read.id == *correlation_id) =>
            {
                self.read = None;
                self.error = Some(error.clone());
                Err(error.clone())
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.resolve_id.as_ref() == Some(correlation_id)) =>
            {
                if let Some(pending) = self.pending.as_mut() {
                    pending.resolve_id = None;
                    pending.resolve_queued = false;
                }
                self.error = Some(format!(
                    "Saved selection outcome is unknown: {error}. No write was retried."
                ));
                Err(self.error.clone().unwrap())
            }
            _ => Ok(None),
        }
    }

    fn finish(
        &mut self,
        receipt: &EditorBatchReceipt,
        current: Option<&EditorBatchCapture>,
        preflight: &impl Fn(&BatchProjection) -> Result<(), String>,
    ) -> Result<Option<BatchProjection>, String> {
        let pending = self
            .pending
            .as_ref()
            .ok_or("No selection edit owns this receipt")?;
        validation::receipt(receipt, &pending.id, &pending.request)?;
        let after = snapshot(receipt, &pending.request.capture);
        let projected = current.unwrap_or(&after);
        let projection = BatchProjection {
            petal_id: projected.petal_id.clone(),
            members: projected.members.clone(),
            groups: projected.groups.clone(),
            draft_before: draft_sources(&pending.request.capture),
            source: if current.is_some() {
                ProjectionSource::CurrentRead
            } else {
                ProjectionSource::Commit
            },
        };
        preflight(&projection)?;
        let pending = self.pending.take().unwrap();
        let mut entry = pending.replay.unwrap_or_else(|| HistoryEntry {
            before: pending.request.capture.clone(),
            after: after.clone(),
            forward: pending.request.edit.clone(),
            order: 0,
        });
        entry.order = crate::undo::next_history_order();
        let scope_matches = self.history_scope.as_ref().is_some_and(|ids| {
            ids[2] == receipt.petal_id
                && fe_database::build_scope(&ids[0], Some(&ids[1]), Some(&ids[2])) == receipt.scope
        });
        if scope_matches {
            match pending.direction {
                HistoryDirection::New => {
                    self.redo.clear();
                    self.invalidate_legacy_redo = true;
                    self.undo.push_back(entry);
                }
                HistoryDirection::Undo => {
                    self.undo.pop_back();
                    entry.before = after;
                    self.redo.push(entry);
                }
                HistoryDirection::Redo => {
                    self.redo.pop();
                    entry.after = after;
                    self.undo.push_back(entry);
                }
            }
            while self.undo.len() + self.redo.len() > MAX_HISTORY
                || self
                    .undo
                    .iter()
                    .chain(&self.redo)
                    .map(|entry| {
                        serde_json::to_vec(&entry.before)
                            .map_or(MAX_EDITOR_BATCH_BYTES, |value| value.len())
                            + serde_json::to_vec(&entry.after)
                                .map_or(MAX_EDITOR_BATCH_BYTES, |value| value.len())
                    })
                    .sum::<usize>()
                    > MAX_EDITOR_BATCH_BYTES
            {
                if self.undo.pop_front().is_none() {
                    self.redo.remove(0);
                }
            }
        }
        self.error = None;
        self.capture = None;
        self.interaction = None;
        Ok(Some(projection))
    }
}

pub(crate) fn replay(
    batches: &mut EditorBatchState,
    undo: &mut crate::undo::UndoState,
    redo: bool,
    nav: &NavigationManager,
    hierarchy: &VerseManager,
    visibility: &crate::visibility::VisibilityState,
    db: &crossbeam::channel::Sender<DbCommand>,
) -> Result<(), String> {
    if batches.is_pending() || undo.is_pending() {
        return Err("Wait for the pending edit or reconciliation; no Undo was sent".into());
    }
    batches.synchronize_history(undo);
    if batches.newest(redo) > undo.newest(redo) {
        batches.replay(redo, nav, db)
    } else {
        undo.replay(redo, nav, hierarchy, visibility, db)
    }
}
