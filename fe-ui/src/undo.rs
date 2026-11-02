//! Bounded editor commands; see src/AGENTS.md section editor-undo.

use crate::{navigation_manager::NavigationManager, verse_manager::VerseManager};
use bevy::prelude::*;
use fe_runtime::{
    messages::{DbCommand, DbResult, NodeMutation},
    node_history::{NodeEditBefore, NodeEditPrecondition, NodeMutationReceipt, NodeTransformValue},
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 50;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const SAVE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
enum EditValue {
    Transform(NodeTransformValue),
    PathPoints(Value),
}
impl EditValue {
    fn expected(&self) -> NodeEditBefore {
        match self {
            Self::Transform(v) => NodeEditBefore::Transform(v.clone()),
            Self::PathPoints(v) => NodeEditBefore::PathPoints(v.clone()),
        }
    }
    fn mutation(&self) -> NodeMutation {
        match self {
            Self::Transform(value) => {
                let (x, y, z) = Quat::from_array(value.rotation).to_euler(EulerRot::XYZ);
                NodeMutation::FullTransform {
                    position: value.position.map(|v| v as f32),
                    rotation: [x, y, z],
                    scale: value.scale.map(|v| v as f32),
                }
            }
            Self::PathPoints(value) => NodeMutation::PropertySet {
                key: "gpx_points".into(),
                value: value.clone(),
            },
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Transform(_) => 128,
            Self::PathPoints(v) => serde_json::to_vec(v).map_or(MAX_BYTES + 1, |v| v.len()),
        }
    }
}
#[derive(Debug, Clone)]
struct Entry {
    node_id: String,
    petal_id: String,
    scope: String,
    before: EditValue,
    after: EditValue,
    version: Option<i64>,
}
impl Entry {
    fn bytes(&self) -> usize {
        self.before.bytes() + self.after.bytes() + self.node_id.len() + self.scope.len()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Normal,
    Undo,
    Redo,
}
#[derive(Debug, Clone)]
struct PathDraft {
    node_id: String,
    index: usize,
    before: [f32; 3],
    after: [f32; 3],
    version: Option<i64>,
    rows: Vec<crate::gis::PathPointRow>,
}
#[derive(Debug)]
struct Pending {
    request_id: String,
    resolve_id: Option<String>,
    generation: u64,
    entry: Option<Entry>,
    path: Option<PathDraft>,
    origin: Origin,
    started: Instant,
    conflict: bool,
}

/// Gesture context retained before a path editor buffer can change.
#[derive(Debug, Clone)]
pub struct PointEditOrigin {
    track_id: String,
    petal_id: Option<String>,
    points: Vec<crate::gis::PathPointRow>,
}
impl PointEditOrigin {
    pub(crate) fn capture(
        state: &crate::gis::PathEditorState,
        nav: &NavigationManager,
    ) -> Option<Self> {
        Some(Self {
            track_id: state.editing_track_id.clone()?,
            petal_id: nav.active_petal_id.clone(),
            points: state.points.clone(),
        })
    }
    pub(crate) fn matches(
        &self,
        state: &crate::gis::PathEditorState,
        nav: &NavigationManager,
    ) -> bool {
        state.editing_track_id.as_deref() == Some(self.track_id.as_str())
            && self.petal_id == nav.active_petal_id
            && self.points == state.points
    }
}

/// Authoritative route projection produced only by a matching save or reconciliation.
#[derive(Debug, Clone)]
pub struct PathProjection {
    /// Track whose persisted points were confirmed.
    pub node_id: String,
    /// Stored petal owning the confirmed route.
    pub petal_id: String,
    /// Complete point rows with their original metadata.
    pub points: Value,
}
impl PathProjection {
    /// Decode the confirmed route for the editor's existing projection.
    pub fn editor_rows(&self) -> Vec<crate::gis::PathPointRow> {
        crate::gis::decode_gpx_points(&self.points)
    }
}

/// Session-only, bounded history of confirmed local transforms and point moves.
#[derive(Resource, Default)]
pub struct UndoState {
    projections: VecDeque<PathProjection>,
    scope: Option<String>,
    petal_id: Option<String>,
    generation: u64,
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    pending: Option<Pending>,
}

/// Snapshot full typed transform values before a local edit.
pub(crate) fn transform_value(value: &Transform) -> NodeTransformValue {
    NodeTransformValue {
        position: value.translation.to_array().map(f64::from),
        rotation: value.rotation.to_array(),
        scale: value.scale.to_array().map(f64::from),
    }
}

fn request_id() -> String {
    format!("ui-undo-{}", ulid::Ulid::new())
}

impl UndoState {
    fn sync(&mut self, nav: &NavigationManager) {
        let scope = nav
            .active_verse_id
            .as_deref()
            .zip(nav.active_fractal_id.as_deref())
            .zip(nav.active_petal_id.as_deref())
            .map(|((v, f), p)| fe_database::build_scope(v, Some(f), Some(p)));
        if self.scope != scope {
            self.scope = scope;
            self.petal_id = nav.active_petal_id.clone();
            self.generation = self.generation.wrapping_add(1);
            self.undo.clear();
            self.redo.clear();
            self.pending = None;
            self.projections.clear();
        }
    }
    /// Drain one confirmed route projection for the host's render bridge.
    pub fn take_path_projection(&mut self) -> Option<PathProjection> {
        self.projections.pop_front()
    }
    fn project_path(&mut self, node_id: &str, petal_id: &str, points: Value) {
        self.projections.retain(|p| p.node_id != node_id);
        if self.projections.len() >= MAX_ENTRIES {
            self.projections.pop_front();
        }
        self.projections.push_back(PathProjection {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            points,
        });
    }
    #[cfg(test)]
    pub(crate) fn history_counts(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
    /// Covered edits serialize until persistence or exact reconciliation resolves.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    /// A path awaiting a checked save; other writes to it must wait.
    pub fn pending_path(&self) -> Option<&str> {
        self.pending.as_ref().and_then(|p| {
            p.path.as_ref().map(|v| v.node_id.as_str()).or_else(|| {
                p.entry
                    .as_ref()
                    .filter(|v| matches!(v.before, EditValue::PathPoints(_)))
                    .map(|v| v.node_id.as_str())
            })
        })
    }
    /// Unsupported local operations invalidate the redo branch.
    pub fn invalidate_redo(&mut self) {
        self.redo.clear();
    }
    fn ready(&self) -> Result<(), String> {
        if self.pending.is_some() {
            Err("Saving the previous edit; wait for confirmation".into())
        } else if self.scope.is_none() {
            Err("Open a petal before editing".into())
        } else {
            Ok(())
        }
    }
    fn start(
        &mut self,
        entry: Entry,
        origin: Origin,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<String, String> {
        self.ready()?;
        let id = request_id();
        let (expected, target) = if origin == Origin::Undo {
            (&entry.after, &entry.before)
        } else {
            (&entry.before, &entry.after)
        };
        db.send(DbCommand::CommitCheckedNodeMutation {
            node_id: entry.node_id.clone(),
            mutation: target.mutation(),
            expected: NodeEditPrecondition {
                scope: entry.scope.clone(),
                history_version: entry.version,
                before: expected.expected(),
            },
            correlation_id: id.clone(),
        })
        .map_err(|_| "Database connection closed; edit was not queued")?;
        self.pending = Some(Pending {
            request_id: id.clone(),
            resolve_id: None,
            generation: self.generation,
            entry: Some(entry),
            path: None,
            origin,
            started: Instant::now(),
            conflict: false,
        });
        Ok(id)
    }
    /// Submit one ordinary transform; only its matching receipt records history.
    pub(crate) fn transform(
        &mut self,
        nav: &NavigationManager,
        node_id: &str,
        petal_id: &str,
        before: Transform,
        after: Transform,
        version: Option<i64>,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<bool, String> {
        self.sync(nav);
        self.ready()?;
        if self.petal_id.as_deref() != Some(petal_id) {
            return Err("Object moved to another petal".into());
        }
        let before = transform_value(&before);
        let after = transform_value(&after);
        if before.matches(&after) {
            return Ok(false);
        }
        self.start(
            Entry {
                node_id: node_id.into(),
                petal_id: petal_id.into(),
                scope: self.scope.clone().unwrap(),
                before: EditValue::Transform(before),
                after: EditValue::Transform(after),
                version,
            },
            Origin::Normal,
            db,
        )?;
        Ok(true)
    }
    /// Reserve one point gesture before its local row changes.
    pub fn reserve_path(
        &mut self,
        nav: &NavigationManager,
        node_id: &str,
        index: usize,
        before: [f32; 3],
        after: [f32; 3],
        version: Option<i64>,
        rows: &[crate::gis::PathPointRow],
    ) -> Result<Option<String>, String> {
        self.sync(nav);
        self.ready()?;
        if before.iter().chain(&after).any(|value| !value.is_finite()) {
            return Err("Path point coordinates must be finite".into());
        }
        if Vec3::from_array(before).distance(Vec3::from_array(after)) < 0.0001 {
            return Ok(None);
        }
        let id = request_id();
        self.pending = Some(Pending {
            request_id: id.clone(),
            resolve_id: None,
            generation: self.generation,
            entry: None,
            path: Some(PathDraft {
                node_id: node_id.into(),
                index,
                before,
                after,
                version,
                rows: rows.to_vec(),
            }),
            origin: Origin::Normal,
            started: Instant::now(),
            conflict: false,
        });
        Ok(Some(id))
    }
    /// Authorize a current-state read before preparing a reserved point mutation.
    pub fn path_read_command(&self, id: &str) -> Result<DbCommand, String> {
        let pending = self
            .pending
            .as_ref()
            .filter(|p| p.request_id == id && p.generation == self.generation && p.entry.is_none())
            .ok_or("Path reservation is no longer active")?;
        let draft = pending.path.as_ref().ok_or("Path reservation is missing")?;
        Ok(DbCommand::ResolveCheckedNodeMutation {
            node_id: draft.node_id.clone(),
            petal_id: self.petal_id.clone().ok_or("Petal closed")?,
            request_id: id.into(),
            correlation_id: id.into(),
        })
    }
    /// Whether an exact request still belongs to this active editor generation.
    pub fn owns_request(&self, id: &str) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.request_id == id && p.generation == self.generation)
    }
    /// Bind the reservation to complete persisted point JSON without rebuilding metadata.
    pub fn commit_path(
        &mut self,
        id: &str,
        before: Value,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        let pending = self
            .pending
            .as_ref()
            .filter(|p| p.request_id == id && p.generation == self.generation)
            .ok_or("Path edit belongs to an older session")?;
        let draft = pending
            .path
            .clone()
            .ok_or("Path edit reservation is missing")?;
        if crate::gis::decode_gpx_points(&before) != draft.rows {
            return Err(
                "Path point order or metadata changed since the gesture; move skipped".into(),
            );
        }
        let mut after = before.clone();
        let row = after
            .as_array_mut()
            .and_then(|rows| rows.get_mut(draft.index))
            .and_then(Value::as_array_mut)
            .filter(|row| row.len() >= 4)
            .ok_or("Path point identity changed; move skipped")?;
        if !row[..3].iter().zip(draft.before).all(|(a, b)| {
            a.as_f64()
                .is_some_and(|a| (a - f64::from(b)).abs() <= 0.0001)
        }) {
            return Err("Path point changed since the gesture; move skipped".into());
        }
        for (target, value) in row[..3].iter_mut().zip(draft.after) {
            *target = serde_json::json!(value as f64);
        }
        let entry = Entry {
            node_id: draft.node_id,
            petal_id: self.petal_id.clone().ok_or("Petal closed")?,
            scope: self.scope.clone().ok_or("Petal closed")?,
            before: EditValue::PathPoints(before),
            after: EditValue::PathPoints(after),
            version: draft.version,
        };
        db.send(DbCommand::CommitCheckedNodeMutation {
            node_id: entry.node_id.clone(),
            mutation: entry.after.mutation(),
            expected: NodeEditPrecondition {
                scope: entry.scope.clone(),
                history_version: entry.version,
                before: entry.before.expected(),
            },
            correlation_id: id.into(),
        })
        .map_err(|_| "Database connection closed; move was not queued")?;
        self.pending.as_mut().unwrap().entry = Some(entry);
        Ok(())
    }
    /// Abandon a path reservation that has not enqueued a mutation.
    pub fn cancel_path(&mut self, id: &str) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.request_id == id && p.entry.is_none())
        {
            self.pending = None;
        }
    }
    fn reconcile(&mut self, db: &crossbeam::channel::Sender<DbCommand>) -> Result<(), String> {
        let pending = self.pending.as_mut().ok_or("No edit is pending")?;
        if pending.entry.is_none() {
            return Err("Waiting for the path's stored points".into());
        }
        let entry = pending.entry.as_ref().unwrap();
        let id = request_id();
        db.send(DbCommand::ResolveCheckedNodeMutation {
            node_id: entry.node_id.clone(),
            petal_id: entry.petal_id.clone(),
            request_id: pending.request_id.clone(),
            correlation_id: id.clone(),
        })
        .map_err(|_| "Database connection closed; saved outcome remains unknown")?;
        pending.resolve_id = Some(id);
        pending.started = Instant::now();
        Ok(())
    }
    pub(crate) fn replay(
        &mut self,
        redo: bool,
        nav: &NavigationManager,
        hierarchy: &VerseManager,
        visibility: &crate::visibility::VisibilityState,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        self.sync(nav);
        if self.pending.is_some() {
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.started.elapsed() > SAVE_TIMEOUT)
            {
                self.reconcile(db)?;
            }
            return Err("Saving or reconciling the previous edit; no Undo was sent".into());
        }
        self.ready()?;
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
        if !hierarchy.all_nodes().any(|node| node.id == entry.node_id)
            || !crate::visibility::pickable(&entry.node_id, nav, visibility)
        {
            return Err("This object is unavailable; Undo skipped".into());
        }
        if hierarchy
            .committed_versions
            .get(&entry.node_id)
            .is_some_and(|v| Some(*v) != entry.version)
        {
            if redo {
                self.redo.pop();
            } else {
                self.undo.pop_back();
            }
            self.redo.clear();
            return Err("Object changed since this edit; Undo skipped".into());
        }
        self.start(entry, if redo { Origin::Redo } else { Origin::Undo }, db)?;
        Ok(())
    }
    fn confirm(&mut self, receipt: &NodeMutationReceipt) -> Result<(), String> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(());
        };
        let entry = pending
            .entry
            .as_ref()
            .ok_or("Path receipt arrived before point preparation")?;
        if pending.generation != self.generation
            || receipt.node_id != entry.node_id
            || receipt.state.node_id != entry.node_id
            || receipt.state.petal_id != entry.petal_id
        {
            return Err("Unrelated edit receipt; reconciliation required".into());
        }
        if receipt.history_version <= 0
            || entry
                .version
                .is_some_and(|old| receipt.history_version <= old)
        {
            return Err("Saved edit version did not advance; reconciliation required".into());
        }
        let target = if pending.origin == Origin::Undo {
            &entry.before
        } else {
            &entry.after
        };
        let matches = match target {
            EditValue::Transform(value) => {
                NodeTransformValue::from_row(&receipt.state).is_some_and(|v| value.matches(&v))
            }
            EditValue::PathPoints(value) => {
                receipt.state.properties.get("gpx_points") == Some(value)
            }
        };
        if !matches {
            return Err("Saved edit differs from its receipt; reconciliation required".into());
        }
        if let Some(points) = receipt
            .state
            .properties
            .get("gpx_points")
            .filter(|_| matches!(entry.before, EditValue::PathPoints(_)))
        {
            self.project_path(&receipt.node_id, &receipt.state.petal_id, points.clone());
        }
        let pending = self.pending.take().unwrap();
        let mut entry = pending.entry.unwrap();
        entry.version = Some(receipt.history_version);
        match pending.origin {
            Origin::Normal => {
                self.redo.clear();
                self.undo.push_back(entry);
            }
            Origin::Undo => {
                self.undo.pop_back();
                self.redo.push(entry);
            }
            Origin::Redo => {
                self.redo.pop();
                self.undo.push_back(entry);
            }
        }
        while self.undo.len() + self.redo.len() > MAX_ENTRIES
            || self
                .undo
                .iter()
                .chain(&self.redo)
                .map(Entry::bytes)
                .sum::<usize>()
                > MAX_BYTES
        {
            if self.undo.pop_front().is_none() {
                self.redo.remove(0);
            }
        }
        Ok(())
    }
}

pub(crate) fn observe_results(
    mut results: MessageReader<DbResult>,
    nav: Res<NavigationManager>,
    mut undo: ResMut<UndoState>,
    db: Res<fe_runtime::app::DbCommandSender>,
    mut ui: ResMut<crate::actions::UiManager>,
    clock: Res<crate::notice::NoticeClock>,
    mut manager: ResMut<VerseManager>,
    mut nodes: Query<(&mut Transform, &crate::plugin::SpawnedNodeMarker)>,
    mut path_cache: Option<ResMut<crate::verse_manager::PathAssetCache>>,
) {
    undo.sync(&nav);
    let now = clock.elapsed_secs_f64();
    for result in results.read() {
        match result {
            DbResult::NodeMutationCommitted {
                receipt,
                correlation_id: Some(id),
            } if undo.pending.as_ref().is_some_and(|p| p.request_id == *id) => {
                match undo.confirm(receipt) {
                    Ok(()) => ui.show_toast("Edit saved", now),
                    Err(error) => {
                        ui.show_error(error, now);
                        if let Err(error) = undo.reconcile(&db.0) {
                            ui.show_error(error, now);
                        }
                    }
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if undo
                .pending
                .as_ref()
                .is_some_and(|p| p.request_id == *correlation_id) =>
            {
                undo.pending.as_mut().unwrap().conflict =
                    error.contains("changed") || error.contains("moved");
                ui.show_advisory(error.clone(), now);
                if let Err(error) = undo.reconcile(&db.0) {
                    ui.show_error(error, now);
                }
            }
            DbResult::RequestFailed {
                correlation_id,
                error,
            } if undo
                .pending
                .as_ref()
                .is_some_and(|p| p.resolve_id.as_ref() == Some(correlation_id)) =>
            {
                ui.show_error(
                    format!("Saved outcome is unknown: {error}. No retry was sent."),
                    now,
                )
            }
            DbResult::CheckedNodeMutationResolved {
                node_id,
                request_id,
                receipt,
                current,
                correlation_id,
            } if undo.pending.as_ref().is_some_and(|p| {
                p.request_id == *request_id
                    && p.resolve_id.as_ref() == Some(correlation_id)
                    && p.entry.as_ref().is_some_and(|e| {
                        e.node_id == *node_id
                            && current.node_id == *node_id
                            && e.petal_id == current.petal_id
                    })
                    && p.generation == undo.generation
            }) =>
            {
                if undo
                    .pending
                    .as_ref()
                    .and_then(|p| p.entry.as_ref())
                    .is_some_and(|e| matches!(e.before, EditValue::PathPoints(_)))
                {
                    undo.project_path(
                        node_id,
                        &current.petal_id,
                        current
                            .properties
                            .get("gpx_points")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!([])),
                    );
                }
                if let Some(cache) = path_cache.as_deref_mut() {
                    cache.note_properties(
                        node_id,
                        Some(&current.petal_id),
                        &Value::Object(current.properties.clone()),
                    );
                }
                if let Some(value) = NodeTransformValue::from_row(current) {
                    let transform = Transform {
                        translation: Vec3::from_array(value.position.map(|v| v as f32)),
                        rotation: Quat::from_array(value.rotation),
                        scale: Vec3::from_array(value.scale.map(|v| v as f32)),
                    };
                    manager.update_node_transform(node_id, transform);
                    for (mut target, marker) in &mut nodes {
                        if marker.node_id == *node_id && marker.petal_id == current.petal_id {
                            *target = transform;
                        }
                    }
                }
                if let Some(receipt) = receipt {
                    if let Err(error) = undo.confirm(receipt) {
                        ui.show_error(error, now);
                    }
                } else {
                    let pending = undo.pending.take().unwrap();
                    if pending.conflict && pending.origin != Origin::Normal {
                        if pending.origin == Origin::Undo {
                            undo.undo.pop_back();
                        } else {
                            undo.redo.pop();
                        }
                        undo.redo.clear();
                    }
                    ui.show_advisory("Edit was not committed; current stored state restored", now);
                }
            }
            DbResult::DatabaseReset { .. } => {
                undo.scope = None;
                undo.generation = undo.generation.wrapping_add(1);
                undo.projections.clear();
                undo.undo.clear();
                undo.redo.clear();
                undo.pending = None;
            }
            DbResult::NodeMutationCommitted { receipt, .. }
                if undo.petal_id.as_deref() == Some(receipt.state.petal_id.as_str()) =>
            {
                undo.redo.clear()
            }
            DbResult::NodePropertySet { node_id, .. }
            | DbResult::NodePropertyDeleted { node_id, .. }
                if manager.node_index.get(node_id).is_some_and(|&(v, f, p)| {
                    undo.petal_id.as_deref()
                        == Some(manager.verses[v].fractals[f].petals[p].id.as_str())
                }) =>
            {
                undo.redo.clear()
            }
            _ => {}
        }
    }
    if undo
        .pending
        .as_ref()
        .is_some_and(|p| p.started.elapsed() > SAVE_TIMEOUT && p.entry.is_none())
    {
        undo.pending = None;
        ui.show_error("Path read timed out; no point move was sent", now);
    }
    if undo.pending.as_ref().is_some_and(|p| {
        p.started.elapsed() > SAVE_TIMEOUT && p.resolve_id.is_none() && p.entry.is_some()
    }) {
        ui.show_advisory(
            "Save confirmation is delayed; checking its exact stored outcome",
            now,
        );
        if let Err(error) = undo.reconcile(&db.0) {
            ui.show_error(error, now);
        }
    }
}
