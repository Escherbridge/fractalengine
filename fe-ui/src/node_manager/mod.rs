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
/// gimbal drag and future terrain/road-builder consumers of the shared table.
pub use dispatch::{
    resolve_operation, terrain_cell_proposal, HandleSide, HitTarget, Operation, TerrainBrush,
    TerrainProposalEdit,
};

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
        app.init_resource::<path_point_interaction::PathPointDrag>();
        app.init_resource::<path_point_interaction::PenHandleDrag>();
        app.init_resource::<path_handle_interaction::PathHandleDrag>();
        app.init_resource::<path_gimbal_drag::PathGimbalDrag>();
        app.init_resource::<selection::SelectionState>();
        app.add_systems(Startup, crate::gimbal::configure_gimbal_gizmos);
        app.add_systems(
            Update,
            (
                shortcuts::handle_tool_shortcuts,
                sidebar_sync::sync_sidebar_to_manager,
                router::resolve_pointer_frame, // arbitrate left-click ownership for this frame (first)
                gimbal_interaction::update_hovered_axis, // hover detection (before interaction)
                path_handle_interaction::sync_path_handle_markers, // keep handle markers current before their pick
                path_handle_interaction::handle_path_handle_interaction, // claims PathHandle FIRST — Q7 handle > vertex > gimbal
                path_gimbal_drag::handle_path_gimbal_drag, // FR-3: claims Gimbal FIRST for a selected vertex/segment
                gimbal_interaction::handle_gimbal_interaction, // claims Gimbal on axis pick + drag
                path_point_interaction::sync_path_point_markers, // keep markers in sync with edit buffer
                path_point_interaction::handle_path_point_interaction, // claims PathMarker / PathPlace
                path_segment_interaction::handle_path_segment_interaction, // claims PathSegment — ribbon-segment select (FR-3)
                viewport_pick::handle_viewport_click, // claims NodePick — entity pick / deselect
                viewport_pick::open_track_on_select, // clicking a track ribbon opens it for editing
                path_segment_interaction::sync_path_measurements, // live metric length readouts (FR-3)
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
        if let Some(ref mut sel) = mgr.selected {
            sel.drag = Some(AxisDrag {
                axis: crate::gimbal::GimbalAxis::X,
                start_cursor: Vec2::ZERO,
                axis_screen_dir: Vec2::X,
                start_pos: Vec3::ZERO,
                start_rot: Quat::IDENTITY,
                start_scale: Vec3::ONE,
            });
        }
        assert!(mgr.is_dragging());
    }
}
