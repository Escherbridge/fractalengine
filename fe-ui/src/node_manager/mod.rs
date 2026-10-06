//! NodeManager — single source of truth for node selection and gimbal state.
//!
//! All selection queries (entity, node_id) go through this manager.
//! UI panels and systems read from NodeManager rather than maintaining
//! their own copies of selection state.
//!
//! State machine per selected node:
//!   None  ──click──►  Selected(Idle)  ──press axis──►  Selected(Dragging)
//!   Selected(Dragging)  ──release──►  Selected(Idle)  (+ broadcast commit)
//!   Any  ──Escape / empty click──►  None
//!
//! See `fe-ui/src/node_manager/AGENTS.md` for the submodule map.

mod billboard;
mod brush_interaction;
/// T4 right-click → `ContextTarget` classification. See AGENTS.md §context-pick.
mod context_pick;
/// Pure curve + shape math for the pen tool (phase 2). See AGENTS.md §pen-tool.
pub(crate) mod curve;
/// FR-2 object-aware left-click dispatch model (truth table). See AGENTS.md §dispatch.
mod dispatch;
mod gimbal_interaction;
mod inspector_sync;
/// FR-3 drag: gimbal-drag a selected path vertex/segment. See AGENTS.md §dispatch.
mod path_gimbal_drag;
/// FR-5 (pen_curve_tool_20260722): bezier-handle markers + drag. See AGENTS.md §pen-tool.
mod path_handle_interaction;
mod path_point_interaction;
mod path_segment_interaction;
/// FR-3 cross-authority pointer bridge: coordinates `NodeManager.selected` with
/// `PathEditorState` WITHOUT merging them. See AGENTS.md §pointer-manager.
mod pointer;
mod router;
/// Typed selection read-model (FR-1): a per-frame projection over the two
/// selection authorities. See AGENTS.md §selection-read-model.
mod selection;
mod shortcuts;
mod sidebar_sync;
mod transform_broadcast;
mod viewport_pick;

use bevy::prelude::*;

use crate::gimbal::GimbalAxis;
use crate::plugin::UiSet;

/// path_interaction_20260716 (FR-1/FR-4): precise ribbon pick geometry attached
/// to rendered track lines by the fractalengine gpx bridge. Re-exported so
/// `fe_ui::node_manager::TrackPickShape` is reachable outside this crate.
pub use path_segment_interaction::TrackPickShape;

/// FR-1 typed selection read-model, re-exported for the gimbal, the object-aware
/// left-click dispatch (FR-2), and panels (the tool inspector projects it to name
/// the live selection + gimbal affordance).
pub(crate) use selection::{project_selection, SelectionKind, SelectionState};

/// FR-2 object-aware left-click dispatch model, re-exported for the FR-3 path
/// gimbal drag, the right-click menu (`dialogs::context_menu`), and future
/// terrain/road-builder consumers of the shared table.
pub use dispatch::{resolve_operation, HandleSide, HitTarget, Operation};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Manager for node selection and active drag. Register via [`NodeManagerPlugin`].
#[derive(Resource, Default)]
pub struct NodeManager {
    pub selected: Option<NodeSelection>,
    /// Sidebar click stores the node_id here; `sync_sidebar_to_manager`
    /// resolves the ECS Entity and calls `select()`.
    pub pending_sidebar_select: Option<String>,
    /// Which axis the cursor is hovering over (for highlight feedback).
    pub hovered_axis: Option<GimbalAxis>,
    /// Per-frame mirror of `GestureParams::any_active`, written first in the
    /// chain by `shortcuts::handle_tool_shortcuts`. It is how the egui side
    /// (topbar `InputContext` stash → viewport right-click rule) learns a
    /// gesture is live without reaching the module-private gesture resources.
    ///
    /// STALENESS (finding F16): the mirror is computed BEFORE any gesture system
    /// runs, so it lags in BOTH directions — stale-FALSE on the frame a gesture
    /// STARTS (the unsafe one: that frame's right-click still opens the object
    /// menu), stale-TRUE on the frame it RELEASES (one harmless extra frame of
    /// suppression). Only a rung-0 cancel is exact. See AGENTS.md
    /// §staged-escape.
    pub gesture_active: bool,
}

/// A currently selected node and its optional in-progress drag session.
#[derive(Debug)]
pub struct NodeSelection {
    pub entity: Entity,
    pub node_id: String,
    /// Active gimbal drag, or `None` when just selected.
    pub drag: Option<AxisDrag>,
    /// Pulses `true` for one frame when a drag is released so the broadcast
    /// system can write the final transform to the DB and peers.
    pub drag_committed: bool,
}

/// An in-progress gimbal axis drag.
#[derive(Debug)]
pub struct AxisDrag {
    pub axis: GimbalAxis,
    pub start_cursor: Vec2,
    pub axis_screen_dir: Vec2,
    pub start_pos: Vec3,
    pub start_rot: Quat,
    pub start_scale: Vec3,
}

/// Pre-drag `Transform` snapshot captured at gimbal press — the entity gimbal
/// applies its drag LIVE, so a rung-0 Escape must put these three fields back
/// or the viewport keeps a transform the DB never heard about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragRestore {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl DragRestore {
    /// Write the snapshot back over a live `Transform`.
    pub fn apply(self, transform: &mut Transform) {
        transform.translation = self.translation;
        transform.rotation = self.rotation;
        transform.scale = self.scale;
    }
}

/// How many independent in-flight gestures [`GestureParams`] aggregates.
const GESTURE_COUNT: usize = 6;

/// Pure aggregate for [`GestureParams::any_active`] — "a gesture is live" is
/// one OR over [`GESTURE_COUNT`] independent sources, unit-testable without a
/// Bevy `App` so a newly-added gesture that forgets the bundle trips a test
/// rather than shipping (finding F5: the entity `AxisDrag` was the sixth).
fn any_gesture_active(flags: [bool; GESTURE_COUNT]) -> bool {
    flags.iter().any(|live| *live)
}

/// Aggregate over the six in-flight viewport gestures — one place to ask "is a
/// gesture live?" and to drop them all. Rung 0 of the staged-Escape ladder and
/// the gesture half of the one-right-click rule read this; it is the reason no
/// tool needs its own private Escape handler. Every downstream Release handler
/// `take()`s its own state, so an externally-cleared resource is a silent no-op
/// (never a replay). See `node_manager/AGENTS.md` §staged-escape.
/// Module-private on purpose: the field types are `node_manager`-internal, and
/// the egui side reads the aggregated bit off [`NodeManager::gesture_active`]
/// instead (mirrored each frame by `handle_tool_shortcuts`).
///
/// The bundle owns `NodeManager` rather than sitting alongside it: the sixth
/// gesture (the entity gimbal's [`AxisDrag`]) lives INSIDE the selection, and
/// two `ResMut<NodeManager>` in one system is a Bevy param-aliasing panic — so
/// `handle_tool_shortcuts` reaches the manager through this bundle.
#[derive(bevy::ecs::system::SystemParam)]
struct GestureParams<'w, 's> {
    brush: ResMut<'w, brush_interaction::BrushGesture>,
    path_point: ResMut<'w, path_point_interaction::PathPointDrag>,
    pen_handle: ResMut<'w, path_point_interaction::PenHandleDrag>,
    path_handle: ResMut<'w, path_handle_interaction::PathHandleDrag>,
    path_gimbal: ResMut<'w, path_gimbal_drag::PathGimbalDrag>,
    /// Also the shortcuts system's handle on selection state (see type docs).
    node_mgr: ResMut<'w, NodeManager>,
    /// Restores the pre-drag `Transform` when a rung-0 cancel drops an
    /// entity gimbal drag.
    transforms: Query<'w, 's, &'static mut Transform>,
}

impl GestureParams<'_, '_> {
    /// `true` while ANY viewport gesture is mid-flight.
    fn any_active(&self) -> bool {
        any_gesture_active([
            self.brush.is_active(),
            self.path_point.active.is_some(),
            self.pen_handle.active.is_some(),
            self.path_handle.active.is_some(),
            self.path_gimbal.active.is_some(),
            // The entity gimbal's Move/Rotate/Scale `AxisDrag` (finding F5).
            self.node_mgr.is_dragging(),
        ])
    }

    /// Drop every in-flight gesture WITHOUT committing it. The entity gimbal
    /// drag additionally REVERTS its live-applied `Transform` to the press-time
    /// snapshot — the other five never write world state before their Release,
    /// so dropping the resource is their whole cancel.
    fn cancel_all(&mut self) {
        self.brush.cancel();
        self.path_point.active = None;
        self.pen_handle.active = None;
        self.path_handle.active = None;
        self.path_gimbal.active = None;
        if let Some((entity, restore)) = self.node_mgr.cancel_axis_drag() {
            if let Ok(mut transform) = self.transforms.get_mut(entity) {
                restore.apply(&mut transform);
            }
        }
    }
}

impl NodeManager {
    pub fn is_selected(&self) -> bool {
        self.selected.is_some()
    }

    pub fn selected_entity(&self) -> Option<Entity> {
        self.selected.as_ref().map(|s| s.entity)
    }

    pub fn is_dragging(&self) -> bool {
        self.selected.as_ref().is_some_and(|s| s.drag.is_some())
    }

    /// Select a node. If the same entity is already selected the drag state
    /// is preserved; selecting a different entity resets drag state.
    pub fn select(&mut self, entity: Entity, node_id: impl Into<String>) {
        let node_id = node_id.into();
        if self.selected.as_ref().map(|s| s.entity) == Some(entity) {
            // Already selected — keep drag state intact.
            return;
        }
        self.selected = Some(NodeSelection {
            entity,
            node_id,
            drag: None,
            drag_committed: false,
        });
    }

    pub fn deselect(&mut self) {
        self.selected = None;
    }

    /// Drop an in-flight entity gimbal drag WITHOUT committing it, yielding the
    /// dragged entity and the pre-drag [`DragRestore`] snapshot the caller must
    /// write back (this type owns no `Transform` access). `None` when no drag
    /// was live. `drag_committed` stays `false`, so `broadcast_transform` never
    /// sees a canceled drag and the selection itself survives — cancel undoes
    /// the transform, it does not deselect. Rung 0 of the staged-Escape ladder
    /// (finding F5).
    pub fn cancel_axis_drag(&mut self) -> Option<(Entity, DragRestore)> {
        let selection = self.selected.as_mut()?;
        let drag = selection.drag.take()?;
        selection.drag_committed = false;
        Some((
            selection.entity,
            DragRestore {
                translation: drag.start_pos,
                rotation: drag.start_rot,
                scale: drag.start_scale,
            },
        ))
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct NodeManagerPlugin;

impl Plugin for NodeManagerPlugin {
    fn build(&self, app: &mut App) {
        app.init_gizmo_group::<crate::gimbal::GimbalGizmoGroup>();
        app.init_resource::<NodeManager>();
        app.init_resource::<router::ClickArbiter>();
        app.init_resource::<brush_interaction::BrushGesture>();
        app.init_resource::<path_point_interaction::PathPointDrag>();
        app.init_resource::<path_point_interaction::PenHandleDrag>();
        app.init_resource::<path_handle_interaction::PathHandleDrag>();
        app.init_resource::<path_gimbal_drag::PathGimbalDrag>();
        app.init_resource::<selection::SelectionState>();
        app.add_systems(Startup, crate::gimbal::configure_gimbal_gizmos);
        app.add_systems(
            Update,
            (
                // MUST stay first: it owns the staged-Escape ladder, so a rung-0
                // gesture cancel lands before any gesture system runs this frame.
                shortcuts::handle_tool_shortcuts,
                sidebar_sync::sync_sidebar_to_manager,
                router::resolve_pointer_frame, // arbitrate left-click ownership for this frame (first)
                // This registration order is NORMATIVE for `ClickPriority` (D14-A):
                // Brush claims 2nd, ahead of every other consumer, while active.
                brush_interaction::handle_brush_interaction, // Brush owns viewport gestures while active
                gimbal_interaction::update_hovered_axis,     // hover detection (before interaction)
                path_handle_interaction::sync_path_handle_markers, // keep handle markers current before their pick
                path_handle_interaction::handle_path_handle_interaction, // claims PathHandle FIRST — Q7 handle > vertex > gimbal
                path_gimbal_drag::handle_path_gimbal_drag, // FR-3: claims Gimbal FIRST for a selected vertex/segment
                gimbal_interaction::handle_gimbal_interaction, // claims Gimbal on axis pick + drag
                path_point_interaction::sync_path_point_markers, // keep markers in sync with edit buffer
                path_point_interaction::handle_path_point_interaction, // claims PathMarker / PathPlace
                path_segment_interaction::handle_path_segment_interaction, // claims PathSegment — ribbon-segment select (FR-3)
                viewport_pick::handle_viewport_click, // claims NodePick — entity pick / deselect
                pointer::open_track_on_select, // clicking a track ribbon opens it for editing (re-homed FR-3)
                inspector_sync::sync_manager_to_inspector,
                selection::update_selection_state, // FR-1: project the read-model before the gimbal draws
                path_handle_interaction::draw_handle_stems, // anchor→handle stems (draws only, steals no clicks)
                gimbal_interaction::draw_gimbal_system,
                transform_broadcast::broadcast_transform,
                transform_broadcast::apply_inbound_transforms,
            )
                .chain()
                .in_set(UiSet::Selection),
        );
        app.add_systems(
            Update,
            (
                context_pick::classify_context_menu
                    .after(viewport_pick::handle_viewport_click)
                    .before(pointer::open_track_on_select),
                path_segment_interaction::sync_path_measurements
                    .after(path_segment_interaction::handle_path_segment_interaction)
                    .before(inspector_sync::sync_manager_to_inspector),
            )
                .in_set(UiSet::Selection),
        );
        // Billboard facing is orientation-only and order-independent of the
        // selection chain above, so it runs as a standalone per-frame system
        // (data_icons_20260713).
        app.add_systems(Update, billboard::billboard_face_camera);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u32) -> Entity {
        Entity::from_bits(n as u64)
    }

    #[test]
    fn select_sets_selected_entity() {
        let mut mgr = NodeManager::default();
        assert!(!mgr.is_selected());
        mgr.select(entity(1), "node-1");
        assert!(mgr.is_selected());
        assert_eq!(mgr.selected_entity(), Some(entity(1)));
    }

    #[test]
    fn select_same_entity_preserves_drag_state() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        if let Some(ref mut sel) = mgr.selected {
            sel.drag_committed = true;
        }
        mgr.select(entity(1), "node-1");
        assert!(
            mgr.selected
                .as_ref()
                .map(|s| s.drag_committed)
                .unwrap_or(false),
            "drag_committed should be preserved when re-selecting same entity"
        );
    }

    #[test]
    fn select_new_entity_resets_drag_state() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        if let Some(ref mut sel) = mgr.selected {
            sel.drag_committed = true;
        }
        mgr.select(entity(2), "node-2");
        assert_eq!(mgr.selected_entity(), Some(entity(2)));
        assert!(
            !mgr.selected
                .as_ref()
                .map(|s| s.drag_committed)
                .unwrap_or(true),
            "drag_committed should be false after selecting a new entity"
        );
        assert!(
            mgr.selected
                .as_ref()
                .and_then(|s| s.drag.as_ref())
                .is_none(),
            "drag should be None after selecting a new entity"
        );
    }

    #[test]
    fn deselect_clears_selection() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        assert!(mgr.is_selected());
        mgr.deselect();
        assert!(!mgr.is_selected());
        assert!(mgr.selected_entity().is_none());
    }

    #[test]
    fn gesture_active_defaults_false_and_survives_selection_changes() {
        // It is a per-frame mirror owned by `handle_tool_shortcuts`, NOT part
        // of the selection state machine — select/deselect must not touch it.
        let mut mgr = NodeManager::default();
        assert!(!mgr.gesture_active);
        mgr.gesture_active = true;
        mgr.select(entity(1), "node-1");
        assert!(mgr.gesture_active);
        mgr.deselect();
        assert!(mgr.gesture_active);
    }

    #[test]
    fn is_dragging_returns_false_when_no_drag() {
        let mut mgr = NodeManager::default();
        assert!(!mgr.is_dragging());
        mgr.select(entity(1), "node-1");
        assert!(!mgr.is_dragging());
    }

    #[test]
    fn is_dragging_returns_true_when_drag_active() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        begin_axis_drag(&mut mgr);
        assert!(mgr.is_dragging());
    }

    // --- F5: the entity gimbal drag is the SIXTH in-flight gesture ---

    /// Start an entity gimbal drag whose press-time snapshot is a recognizable
    /// non-identity transform, so a restore is distinguishable from a no-op.
    fn begin_axis_drag(mgr: &mut NodeManager) {
        if let Some(ref mut sel) = mgr.selected {
            sel.drag = Some(AxisDrag {
                axis: crate::gimbal::GimbalAxis::X,
                start_cursor: Vec2::ZERO,
                axis_screen_dir: Vec2::X,
                start_pos: Vec3::new(1.0, 2.0, 3.0),
                start_rot: Quat::from_rotation_y(0.5),
                start_scale: Vec3::new(2.0, 2.0, 2.0),
            });
        }
    }

    #[test]
    fn any_gesture_active_is_armed_by_each_of_the_six_sources() {
        // The aggregate is one OR over six independent gestures — a bundle
        // that forgets one (F5's entity AxisDrag) fails this arity check.
        assert!(!any_gesture_active([false; GESTURE_COUNT]));
        for i in 0..GESTURE_COUNT {
            let mut flags = [false; GESTURE_COUNT];
            flags[i] = true;
            assert!(any_gesture_active(flags), "source {i} arms the aggregate");
        }
        assert!(any_gesture_active([true; GESTURE_COUNT]));
    }

    #[test]
    fn axis_drag_counts_as_a_live_gesture_for_the_aggregate() {
        // `any_active` feeds `is_dragging()` in as its sixth flag, so an
        // entity transform drag pins Escape at rung 0 and suppresses the
        // right-click object menu like every other gesture.
        let mut mgr = NodeManager::default();
        assert!(!any_gesture_active([
            false,
            false,
            false,
            false,
            false,
            mgr.is_dragging()
        ]));
        mgr.select(entity(1), "node-1");
        begin_axis_drag(&mut mgr);
        assert!(any_gesture_active([
            false,
            false,
            false,
            false,
            false,
            mgr.is_dragging()
        ]));
    }

    #[test]
    fn cancel_axis_drag_returns_the_pre_drag_snapshot_and_keeps_the_selection() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        begin_axis_drag(&mut mgr);
        let (dragged, restore) = mgr.cancel_axis_drag().expect("a drag was live");
        assert_eq!(dragged, entity(1));
        assert_eq!(restore.translation, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(restore.rotation, Quat::from_rotation_y(0.5));
        assert_eq!(restore.scale, Vec3::splat(2.0));
        assert!(!mgr.is_dragging(), "the drag is dropped");
        // Cancel undoes the transform, it does not deselect.
        assert!(mgr.is_selected(), "the selection survives");
        assert!(
            !mgr.selected.as_ref().map(|s| s.drag_committed).unwrap(),
            "a canceled drag must never look committed to broadcast_transform"
        );
    }

    #[test]
    fn cancel_axis_drag_is_a_no_op_without_a_live_drag() {
        let mut mgr = NodeManager::default();
        assert!(mgr.cancel_axis_drag().is_none(), "nothing selected");
        mgr.select(entity(1), "node-1");
        assert!(
            mgr.cancel_axis_drag().is_none(),
            "selected but not dragging"
        );
        assert!(mgr.is_selected());
    }

    #[test]
    fn drag_restore_writes_all_three_transform_fields_back() {
        let mut transform = Transform::from_xyz(9.0, 9.0, 9.0)
            .with_rotation(Quat::from_rotation_z(1.0))
            .with_scale(Vec3::splat(7.0));
        DragRestore {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
        .apply(&mut transform);
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(transform.rotation, Quat::IDENTITY);
        assert_eq!(transform.scale, Vec3::ONE);
    }
}
