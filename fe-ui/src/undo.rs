//! Bounded editor commands; see src/AGENTS.md section editor-undo.

use crate::{navigation_manager::NavigationManager, verse_manager::VerseManager};
use bevy::prelude::*;
use fe_runtime::{
    messages::{DbCommand, DbResult, NodeMutation},
    node_history::{
        NodeEditBefore, NodeEditPrecondition, NodeMutationReceipt, NodeRowSnapshot,
        NodeTransformValue,
    },
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(crate) fn next_history_order() -> u64 {
    static ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    ORDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

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
    fn matches_state(&self, state: &NodeRowSnapshot) -> bool {
        match self {
            Self::Transform(value) => {
                NodeTransformValue::from_row(state).is_some_and(|current| value.matches(&current))
            }
            Self::PathPoints(value) => state.properties.get("gpx_points") == Some(value),
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
    order: u64,
    node_id: String,
    petal_id: String,
    scope: String,
    before: EditValue,
    after: EditValue,
    before_version: Option<i64>,
    after_version: Option<i64>,
}
impl Entry {
    fn expected_version(&self, origin: Origin) -> Option<i64> {
        if origin == Origin::Undo {
            self.after_version
        } else {
            self.before_version
        }
    }
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
enum PointChange {
    Move([f32; 3]),
    Remove,
    Metadata {
        corner: Option<crate::gis::CornerKind>,
        handles: Option<crate::gis::AnchorHandles>,
    },
}

#[derive(Debug, Clone)]
struct PathDraft {
    node_id: String,
    index: usize,
    before: [f32; 3],
    change: PointChange,
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

fn navigation_key(nav: &NavigationManager) -> Option<[&str; 3]> {
    Some([
        nav.active_verse_id.as_deref()?,
        nav.active_fractal_id.as_deref()?,
        nav.active_petal_id.as_deref()?,
    ])
}

fn point_context_key(
    track: Option<&str>,
    navigation: Option<[&str; 3]>,
    points: &[crate::gis::PathPointRow],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    navigation.hash(&mut hash);
    track.hash(&mut hash);
    for row in points {
        row.position.map(f32::to_bits).hash(&mut hash);
        row.time_seconds.map(f64::to_bits).hash(&mut hash);
        row.handle_in.map(|v| v.map(f32::to_bits)).hash(&mut hash);
        row.handle_out.map(|v| v.map(f32::to_bits)).hash(&mut hash);
        row.corner.to_code().to_bits().hash(&mut hash);
        row.smoothness.to_bits().hash(&mut hash);
    }
    hash.finish()
}

/// Gesture context retained before a path editor buffer can change.
#[derive(Debug, Clone)]
pub struct PointEditOrigin {
    track_id: String,
    navigation: Option<[String; 3]>,
    points: Vec<crate::gis::PathPointRow>,
}
impl PointEditOrigin {
    /// Binds an inline confirmation to every currently rendered route field.
    pub(crate) fn confirmation_key(
        state: &crate::gis::PathEditorState,
        nav: &NavigationManager,
    ) -> u64 {
        point_context_key(
            state.editing_track_id.as_deref(),
            navigation_key(nav),
            &state.points,
        )
    }

    pub(crate) fn capture(
        state: &crate::gis::PathEditorState,
        nav: &NavigationManager,
    ) -> Option<Self> {
        Some(Self {
            track_id: state.editing_track_id.clone()?,
            navigation: navigation_key(nav).map(|ids| ids.map(str::to_owned)),
            points: state.points.clone(),
        })
    }
    pub(crate) fn matches(
        &self,
        state: &crate::gis::PathEditorState,
        nav: &NavigationManager,
    ) -> bool {
        state.editing_track_id.as_deref() == Some(self.track_id.as_str())
            && self
                .navigation
                .as_ref()
                .map(|ids| ids.each_ref().map(String::as_str))
                == navigation_key(nav)
            && self.points == state.points
    }
}

const DRAFT_RETAINED: &str =
    "Stored path state changed; local unsaved draft retained. Refresh explicitly to replace it.";

pub(crate) fn retain_local_draft(state: &mut crate::gis::PathEditorState) {
    state.points_pending = false;
    state.last_error = Some(DRAFT_RETAINED.into());
}

#[derive(Debug, Clone)]
pub enum PathEditorPolicy {
    Replace,
    PreserveChanged { before: Option<Value> },
}

impl PathEditorPolicy {
    pub fn preserves(
        &self,
        current: &[crate::gis::PathPointRow],
        incoming: &[crate::gis::PathPointRow],
    ) -> bool {
        let Self::PreserveChanged { before } = self else {
            return false;
        };
        if same_editor_rows(current, incoming) {
            return false;
        }
        before
            .as_ref()
            .is_none_or(|before| !same_editor_rows(current, &crate::gis::decode_gpx_points(before)))
    }
}

fn same_editor_rows(left: &[crate::gis::PathPointRow], right: &[crate::gis::PathPointRow]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.position == right.position
                && left.time_seconds.unwrap_or(0.0) == right.time_seconds.unwrap_or(0.0)
                && left.handle_in == right.handle_in
                && left.handle_out == right.handle_out
                && left.corner == right.corner
                && left.smoothness == right.smoothness
        })
}

/// Authoritative route projection produced only by a matching save or reconciliation.
#[derive(Debug, Clone)]
pub struct PathProjection {
    /// Track whose persisted points were confirmed.
    pub node_id: String,
    /// Stored petal owning the confirmed route.
    pub petal_id: String,
    /// Positive journal version of the confirmed points.
    pub history_version: i64,
    pub scene_incarnation: u128,
    /// Complete point rows with their original metadata.
    pub points: Value,
    pub editor_policy: PathEditorPolicy,
}
impl PathProjection {
    /// Reject stale receipts before the host changes route or editor caches.
    pub fn is_current(&self, manager: &VerseManager) -> bool {
        self.history_version > 0
            && self.scene_incarnation == manager.scene_incarnation()
            && manager.petal_id_of(&self.node_id) == Some(self.petal_id.as_str())
            && manager.local_scene_admission(
                &self.node_id,
                &self.petal_id,
                self.history_version,
            ).is_some()
            && manager.current_scene_points_match(&self.node_id, &self.petal_id, &self.points)
    }
    /// Preserve a changed local draft while authoritative geometry/history advance.
    pub fn apply_editor(&self, state: &mut crate::gis::PathEditorState) -> bool {
        if state.editing_track_id.as_deref() != Some(self.node_id.as_str()) {
            return false;
        }
        let rows = self.editor_rows();
        let retained = !(state.points_pending && state.points.is_empty())
            && self.editor_policy.preserves(&state.points, &rows);
        state.points_pending = false;
        if retained {
            retain_local_draft(state);
            return true;
        }
        if state.last_error.as_deref() == Some(DRAFT_RETAINED) {
            state.last_error = None;
        }
        if rows.len() != state.points.len() {
            state.selected_point = None;
            state.selected_segment = None;
        }
        state.points = rows;
        false
    }

    /// Decode the confirmed route for the editor's existing projection.
    pub fn editor_rows(&self) -> Vec<crate::gis::PathPointRow> {
        crate::gis::decode_gpx_points(&self.points)
    }
}

/// Session-only, bounded history of confirmed local transforms and point moves.
#[derive(Resource, Default)]
pub struct UndoState {
    projections: VecDeque<PathProjection>,
    hydration_nodes: VecDeque<String>,
    hydration_hierarchy: bool,
    navigation: Option<[String; 3]>,
    scope: Option<String>,
    petal_id: Option<String>,
    generation: u64,
    normal_generation: u64,
    owned_restores: VecDeque<crate::editor_batch::history_chain::OwnedRestore>,
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
    pub(crate) fn take_owned_restores(
        &mut self,
    ) -> VecDeque<crate::editor_batch::history_chain::OwnedRestore> {
        std::mem::take(&mut self.owned_restores)
    }

    pub(crate) fn rebase_history(
        &mut self,
        link: &crate::editor_batch::history_chain::OwnedRestore,
    ) {
        let update = |entry: &mut Entry| {
            let (version, expected) = if link.redo {
                (&mut entry.before_version, &entry.before)
            } else {
                (&mut entry.after_version, &entry.after)
            };
            if let Some(saved) = link.ordinary_version(
                &entry.scope,
                &entry.petal_id,
                &entry.node_id,
                *version,
                &expected.expected(),
            ) {
                *version = Some(saved);
            }
        };
        if link.redo {
            self.redo.iter_mut().for_each(update);
        } else {
            self.undo.iter_mut().for_each(update);
        }
    }

    pub(crate) fn normal_generation(&self) -> u64 {
        self.normal_generation
    }
    pub(crate) fn newest(&self, redo: bool) -> Option<u64> {
        if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
        .map(|entry| entry.order)
    }

    fn sync(&mut self, nav: &NavigationManager) {
        let current = navigation_key(nav);
        if self
            .navigation
            .as_ref()
            .map(|ids| ids.each_ref().map(String::as_str))
            == current
        {
            return;
        }
        self.navigation = current.map(|ids| ids.map(str::to_owned));
        self.scope = current.map(|[verse, fractal, petal]| {
            fe_database::build_scope(verse, Some(fractal), Some(petal))
        });
        self.petal_id = nav.active_petal_id.clone();
        self.generation = self.generation.wrapping_add(1);
        self.owned_restores.clear();
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.projections.clear();
        self.hydration_nodes.clear();
        self.hydration_hierarchy = false;
    }
    /// Recheck the actual navigation resource at a host bridge boundary.
    pub fn synchronize_navigation(&mut self, nav: &NavigationManager) {
        self.sync(nav);
    }
    /// Discard queued geometry superseded by another authoritative source.
    pub(crate) fn invalidate_path_projection(&mut self, node_id: &str) {
        self.projections
            .retain(|projection| projection.node_id != node_id);
    }

    /// Drain one confirmed route projection for the host's render bridge.
    pub fn take_path_projection(&mut self, nav: &NavigationManager) -> Option<PathProjection> {
        self.sync(nav);
        self.projections.pop_front()
    }
    fn pending_editor_policy(&self) -> PathEditorPolicy {
        let before = self
            .pending
            .as_ref()
            .and_then(|pending| {
                pending.entry.as_ref().map(|entry| {
                    if pending.origin == Origin::Undo {
                        &entry.after
                    } else {
                        &entry.before
                    }
                })
            })
            .and_then(|value| match value {
                EditValue::PathPoints(points) => Some(points.clone()),
                _ => None,
            });
        PathEditorPolicy::PreserveChanged { before }
    }

    fn project_path(
        &mut self,
        node_id: &str,
        petal_id: &str,
        history_version: i64,
        scene_incarnation: u128,
        points: Value,
        editor_policy: PathEditorPolicy,
    ) {
        if history_version <= 0
            || self.projections.iter().any(|projection| {
                projection.node_id == node_id && projection.history_version > history_version
            })
        {
            return;
        }
        self.invalidate_path_projection(node_id);
        if self.projections.len() >= MAX_ENTRIES {
            self.projections.pop_front();
        }
        self.projections.push_back(PathProjection {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            history_version,
            scene_incarnation,
            points,
            editor_policy,
        });
    }
    fn request_hydration(&mut self, node_id: &str) {
        self.hydration_hierarchy = true;
        if !self.hydration_nodes.iter().any(|id| id == node_id) {
            if self.hydration_nodes.len() >= MAX_ENTRIES {
                self.hydration_nodes.pop_front();
            }
            self.hydration_nodes.push_back(node_id.into());
        }
    }
    fn flush_hydration(&mut self, db: &crossbeam::channel::Sender<DbCommand>) {
        if self.hydration_hierarchy {
            match db.try_send(DbCommand::LoadHierarchy) {
                Ok(()) => self.hydration_hierarchy = false,
                Err(crossbeam::channel::TrySendError::Full(_)) => return,
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    bevy::log::warn!("Database connection closed while hydrating the current path");
                    self.hydration_hierarchy = false;
                    self.hydration_nodes.clear();
                    return;
                }
            }
        }
        while let Some(node_id) = self.hydration_nodes.front() {
            match db.try_send(DbCommand::GetNodeProperties {
                node_id: node_id.clone(),
                correlation_id: None,
            }) {
                Ok(()) => {
                    self.hydration_nodes.pop_front();
                }
                Err(crossbeam::channel::TrySendError::Full(_)) => return,
                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                    bevy::log::warn!(
                        "Database connection closed while hydrating current path properties"
                    );
                    self.hydration_nodes.clear();
                    return;
                }
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn history_counts(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
    #[cfg(test)]
    pub(crate) fn queued_path_projection(&self) -> Option<&PathProjection> {
        self.projections.front()
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
        door: &fe_runtime::scene_write::SceneWriteDoor,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<String, String> {
        self.ready()?;
        let id = request_id();
        let (expected, target) = if origin == Origin::Undo {
            (&entry.after, &entry.before)
        } else {
            (&entry.before, &entry.after)
        };
        // Scene write door: a refusal leaves the stack and pending state untouched.
        match crate::actions::node::submit_scene_write_retrying(
            door,
            db,
            Some(&entry.petal_id),
            DbCommand::CommitCheckedNodeMutation {
                node_id: entry.node_id.clone(),
                mutation: target.mutation(),
                expected: NodeEditPrecondition {
                    scope: entry.scope.clone(),
                    history_version: entry.expected_version(origin),
                    before: expected.expected(),
                },
                correlation_id: id.clone(),
            },
        ) {
            crate::actions::node::GuiSend::Sent | crate::actions::node::GuiSend::Waiting => {}
            crate::actions::node::GuiSend::Refused(message) => return Err(message),
            crate::actions::node::GuiSend::Busy => {
                return Err("Database queue is busy; edit was not queued. Try again.".into())
            }
            crate::actions::node::GuiSend::Closed => {
                return Err("Database connection closed; edit was not queued".into())
            }
        }
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
        door: &fe_runtime::scene_write::SceneWriteDoor,
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
                order: 0,
                node_id: node_id.into(),
                petal_id: petal_id.into(),
                scope: self.scope.clone().unwrap(),
                before: EditValue::Transform(before),
                after: EditValue::Transform(after),
                before_version: version,
                after_version: None,
            },
            Origin::Normal,
            door,
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
                change: PointChange::Move(after),
                version,
                rows: rows.to_vec(),
            }),
            origin: Origin::Normal,
            started: Instant::now(),
            conflict: false,
        });
        Ok(Some(id))
    }
    /// Reserve removal against the complete captured route and current history version.
    pub(crate) fn reserve_path_remove(
        &mut self,
        nav: &NavigationManager,
        node_id: &str,
        index: usize,
        version: Option<i64>,
        rows: &[crate::gis::PathPointRow],
    ) -> Result<String, String> {
        self.sync(nav);
        self.ready()?;
        let before = rows.get(index).ok_or("Path point is unavailable")?.position;
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
                change: PointChange::Remove,
                version,
                rows: rows.to_vec(),
            }),
            origin: Origin::Normal,
            started: Instant::now(),
            conflict: false,
        });
        Ok(id)
    }
    /// Reserve one anchor metadata edit against its complete original route.
    pub(crate) fn reserve_path_metadata(
        &mut self,
        nav: &NavigationManager,
        node_id: &str,
        index: usize,
        corner: Option<crate::gis::CornerKind>,
        handles: Option<crate::gis::AnchorHandles>,
        version: Option<i64>,
        rows: &[crate::gis::PathPointRow],
    ) -> Result<Option<String>, String> {
        self.sync(nav);
        self.ready()?;
        let row = rows.get(index).ok_or("Path point is unavailable")?;
        if corner.is_none_or(|kind| kind == row.corner)
            && handles.is_none_or(|value| value == (row.handle_in, row.handle_out, row.smoothness))
        {
            return Ok(None);
        }
        if let Some((hin, hout, smoothness)) = handles {
            if !smoothness.is_finite()
                || !(0.0..=1.0).contains(&smoothness)
                || hin
                    .iter()
                    .chain(hout.iter())
                    .flatten()
                    .any(|v| !v.is_finite())
            {
                return Err("Anchor metadata must be finite and smoothness must be 0..=1".into());
            }
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
                before: row.position,
                change: PointChange::Metadata { corner, handles },
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
        nav: &NavigationManager,
        id: &str,
        current: &NodeRowSnapshot,
        db: &crossbeam::channel::Sender<DbCommand>,
    ) -> Result<(), String> {
        self.sync(nav);
        let pending = self
            .pending
            .as_ref()
            .filter(|p| p.request_id == id && p.generation == self.generation && p.entry.is_none())
            .ok_or("Path edit belongs to an older session")?;
        let draft = pending
            .path
            .clone()
            .ok_or("Path edit reservation is missing")?;
        if current.node_id != draft.node_id
            || self.petal_id.as_deref() != Some(current.petal_id.as_str())
        {
            return Err("Path read belongs to another target or petal".into());
        }
        let before = current
            .properties
            .get("gpx_points")
            .cloned()
            .ok_or("Path points are unavailable")?;
        let mut stored_rows = crate::gis::decode_gpx_points(&before);
        let mut expected_rows = draft.rows.clone();
        for row in stored_rows.iter_mut().chain(&mut expected_rows) {
            row.time_seconds = Some(row.time_seconds.unwrap_or(0.0));
        }
        if stored_rows != expected_rows {
            return Err(
                "Path point order or metadata changed since the gesture; edit skipped".into(),
            );
        }
        let mut after = before.clone();
        let row = after
            .as_array_mut()
            .and_then(|rows| rows.get_mut(draft.index))
            .and_then(Value::as_array_mut)
            .filter(|row| row.len() >= 4)
            .ok_or("Path point identity changed; edit skipped")?;
        if !row[..3].iter().zip(draft.before).all(|(a, b)| {
            a.as_f64()
                .is_some_and(|a| (a - f64::from(b)).abs() <= 0.0001)
        }) {
            return Err("Path point changed since the gesture; edit skipped".into());
        }
        match draft.change {
            PointChange::Move(position) => {
                for (target, value) in row[..3].iter_mut().zip(position) {
                    *target = serde_json::json!(value as f64);
                }
            }
            PointChange::Remove => {
                after
                    .as_array_mut()
                    .ok_or("Path points are unavailable")?
                    .remove(draft.index);
            }
            PointChange::Metadata { corner, handles } => {
                if row.len() == 4 {
                    row.resize(12, serde_json::json!(0.0));
                }
                if row.len() < 12 {
                    return Err("Anchor metadata row shape is unsupported".into());
                }
                if let Some(kind) = corner {
                    row[10] = serde_json::json!(kind.to_code());
                }
                if let Some((hin, hout, smoothness)) = handles {
                    for (start, handle) in [(4, hin), (7, hout)] {
                        for (target, value) in row[start..start + 3]
                            .iter_mut()
                            .zip(handle.unwrap_or([0.0; 3]))
                        {
                            *target = serde_json::json!(value as f64);
                        }
                    }
                    row[11] = serde_json::json!(smoothness as f64);
                }
            }
        }
        let entry = Entry {
            order: 0,
            node_id: draft.node_id,
            petal_id: self.petal_id.clone().ok_or("Petal closed")?,
            scope: self.scope.clone().ok_or("Petal closed")?,
            before: EditValue::PathPoints(before),
            after: EditValue::PathPoints(after),
            before_version: draft.version,
            after_version: None,
        };
        // Refused here keeps the path reservation; the caller cancels it via `cancel_path`.
        match crate::actions::node::submit_scene_write_retrying(
            &fe_runtime::scene_write::SceneWriteDoor::installed(),
            db,
            Some(&entry.petal_id),
            DbCommand::CommitCheckedNodeMutation {
                node_id: entry.node_id.clone(),
                mutation: entry.after.mutation(),
                expected: NodeEditPrecondition {
                    scope: entry.scope.clone(),
                    history_version: entry.before_version,
                    before: entry.before.expected(),
                },
                correlation_id: id.into(),
            },
        ) {
            crate::actions::node::GuiSend::Sent | crate::actions::node::GuiSend::Waiting => {}
            crate::actions::node::GuiSend::Refused(message) => return Err(message),
            crate::actions::node::GuiSend::Busy => {
                return Err("Database queue is busy; point edit was not queued. Try again.".into())
            }
            crate::actions::node::GuiSend::Closed => {
                return Err("Database connection closed; point edit was not queued".into())
            }
        }
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
                .is_some_and(|p| {
                    p.started.elapsed() > SAVE_TIMEOUT
                        && !crate::actions::node::scene_write_in_door(&p.request_id)
                })
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
            .is_some_and(|v| {
                Some(*v) != entry.expected_version(if redo { Origin::Redo } else { Origin::Undo })
            })
        {
            if redo {
                self.redo.pop();
            } else {
                self.undo.pop_back();
            }
            self.redo.clear();
            return Err("Object changed since this edit; Undo skipped".into());
        }
        self.start(
            entry,
            if redo { Origin::Redo } else { Origin::Undo },
            &fe_runtime::scene_write::SceneWriteDoor::installed(),
            db,
        )?;
        Ok(())
    }
    fn confirm_history(
        &mut self,
        receipt: &NodeMutationReceipt,
        current: &NodeRowSnapshot,
    ) -> Result<(), String> {
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
                .expected_version(pending.origin)
                .is_some_and(|old| receipt.history_version <= old)
        {
            return Err("Saved edit version did not advance; reconciliation required".into());
        }
        let target = if pending.origin == Origin::Undo {
            &entry.before
        } else {
            &entry.after
        };
        if !target.matches_state(&receipt.state) {
            return Err("Saved edit differs from its receipt; reconciliation required".into());
        }
        let current_is_receipt = serde_json::to_value(current)
            .ok()
            .zip(serde_json::to_value(&receipt.state).ok())
            .is_some_and(|(a, b)| a == b);
        let pending = self.pending.take().unwrap();
        let mut entry = pending.entry.unwrap();
        entry.order = next_history_order();
        let restore = if current_is_receipt && pending.origin != Origin::Normal {
            let (version, restored) = if pending.origin == Origin::Undo {
                (entry.before_version, &entry.before)
            } else {
                (entry.after_version, &entry.after)
            };
            crate::editor_batch::history_chain::OwnedRestore::ordinary(
                entry.scope.clone(),
                entry.petal_id.clone(),
                entry.node_id.clone(),
                version,
                restored.expected(),
                fe_runtime::editor_batch::EditorBatchMember {
                    node_id: entry.node_id.clone(),
                    history_version: Some(receipt.history_version),
                    state: receipt.state.clone(),
                    stamp_path: None,
                },
                pending.origin == Origin::Redo,
            )
        } else {
            None
        };
        if pending.origin == Origin::Undo {
            entry.before_version = Some(receipt.history_version);
        } else {
            entry.after_version = Some(receipt.history_version);
        }
        match pending.origin {
            Origin::Normal => {
                self.normal_generation = self.normal_generation.wrapping_add(1);
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
        if let Some(link) = restore {
            self.rebase_history(&link);
            crate::editor_batch::history_chain::queue(&mut self.owned_restores, link);
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
    mut nav: ResMut<NavigationManager>,
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
                let path = undo
                    .pending
                    .as_ref()
                    .and_then(|p| p.entry.as_ref())
                    .is_some_and(|entry| matches!(entry.before, EditValue::PathPoints(_)));
                let editor_policy = undo.pending_editor_policy();
                match undo.confirm_history(receipt, &receipt.state) {
                    Ok(()) => {
                        if path {
                            if let Some(points) = receipt.state.properties.get("gpx_points") {
                                undo.project_path(
                                    &receipt.node_id,
                                    &receipt.state.petal_id,
                                    receipt.history_version,
                                    manager.scene_incarnation(),
                                    points.clone(),
                                    editor_policy,
                                );
                            }
                        }
                        ui.show_toast("Edit saved", now);
                    }
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
                let path = undo
                    .pending
                    .as_ref()
                    .and_then(|pending| pending.entry.as_ref())
                    .is_some_and(|entry| matches!(entry.before, EditValue::PathPoints(_)));
                let confirmed_path_version = receipt
                    .as_ref()
                    .filter(|_| path)
                    .filter(|receipt| {
                        receipt.node_id == *node_id
                            && receipt.history_version > 0
                            && serde_json::to_value(&receipt.state)
                                .ok()
                                .zip(serde_json::to_value(current).ok())
                                .is_some_and(|(saved, current)| saved == current)
                    })
                    .map(|receipt| receipt.history_version);
                if path && confirmed_path_version.is_none() {
                    undo.request_hydration(node_id);
                    nav.suppress_initial_auto_navigation();
                    ui.show_advisory(
                        "Path reconciliation has no current version; refreshing authoritative state. The edit is unsaved or its current outcome is unknown.", now,
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
                    let editor_policy = undo.pending_editor_policy();
                    if let Err(error) = undo.confirm_history(receipt, current) {
                        ui.show_error(error, now);
                    } else if let Some(version) = confirmed_path_version {
                        undo.project_path(
                            node_id,
                            &current.petal_id,
                            version,
                            manager.scene_incarnation(),
                            current
                                .properties
                                .get("gpx_points")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([])),
                            editor_policy,
                        );
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
                    ui.show_advisory(
                        if path {
                            "Edit was not committed; refreshing the current stored path"
                        } else {
                            "Edit was not committed; current stored state restored"
                        },
                        now,
                    );
                }
            }
            DbResult::DatabaseReset { .. } => {
                undo.scope = None;
                undo.navigation = None;
                undo.generation = undo.generation.wrapping_add(1);
                undo.projections.clear();
                undo.hydration_nodes.clear();
                undo.hydration_hierarchy = false;
                undo.owned_restores.clear();
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
        ui.show_error("Path read timed out; no point edit was sent", now);
    }
    if undo.pending.as_ref().is_some_and(|p| {
        p.started.elapsed() > SAVE_TIMEOUT
            && p.resolve_id.is_none()
            && p.entry.is_some()
            && !crate::actions::node::scene_write_in_door(&p.request_id)
    }) {
        ui.show_advisory(
            "Save confirmation is delayed; checking its exact stored outcome",
            now,
        );
        if let Err(error) = undo.reconcile(&db.0) {
            ui.show_error(error, now);
        }
    }
    undo.flush_hydration(&db.0);
}
