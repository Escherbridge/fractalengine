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
/// `pub(crate)`, not private: terrain_proposal_delta_viz S3's
/// `terrain_preview::publish_proposal_preview` reads `BrushGesture`'s
/// `preview_snapshot()` accessor to preview the live brush corridor before
/// commit (plan.md owns `brush_interaction.rs` "accessor only" — this
/// one-line visibility bump is the minimal supporting change that accessor
/// requires; no other behavior here changes). See AGENTS.md §terrain-proposal-editor.
pub(crate) mod brush_interaction;
/// T4 right-click → `ContextTarget` classification. See AGENTS.md §context-pick.
mod context_pick;
/// Pure curve + shape math for the pen tool (phase 2). See AGENTS.md §pen-tool.
pub(crate) mod curve;
/// FR-2 object-aware left-click dispatch model (truth table). See AGENTS.md §dispatch.
mod dispatch;
mod gimbal_interaction;
mod group_gimbal;
mod inspector_sync;
/// hexon_scale FR-9 Measure tool: Tape/Area reducer + viewport anchors. See AGENTS.md §measure-tool.
pub mod measure;
/// D-38 terrain-aware ground point; see AGENTS.md ground-snap.
pub(crate) mod ground_snap;
/// Road builder snap math; see AGENTS.md road-builder.
pub(crate) mod road_snap;
/// Road builder chain state machine; see AGENTS.md road-builder.
pub(crate) mod road_chain;
/// Road builder viewport interaction (clicks, hover); see AGENTS.md road-builder.
pub(crate) mod road_interaction;
/// Road builder ghost polyline and length readout; see AGENTS.md road-builder.
pub(crate) mod road_overlay;
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
mod selected_set;
mod selection;
pub(crate) use selected_set::{SelectionRequest, SelectionRow, SelectionSection};
pub use selected_set::{SelectionMode, SelectionTarget, MAX_SELECTED_NODES};
mod shortcuts;
mod sidebar_sync;
mod transform_broadcast;
mod transform_snap;
pub use transform_snap::{ToolSnapSettings, TransformAxisLock};
mod terrain_authoring;
pub use terrain_authoring::{TerrainAuthoringCommit, TerrainAuthoringState};
pub(crate) use terrain_authoring::shape_at;
/// The GUI scene write door (caller `Ui`); see `actions/AGENTS.md` §scene-write-door.
pub use crate::actions::node::{submit_scene_write, try_submit_scene_write, try_submit_scene_write_on, GuiSend};
mod viewport_pick;
mod stamp_selection;
pub use stamp_selection::{PromotedStampObservation, PromotedStampReader, StampSelectionOrigin};
pub(crate) use stamp_selection::{StampSelectionCatalog, StampSelectionRequests};

use bevy::prelude::*;

use crate::gimbal::GimbalAxis;
use crate::plugin::UiSet;

/// path_interaction_20260716 (FR-1/FR-4): precise ribbon pick geometry attached
/// to rendered track lines by the fractalengine gpx bridge. Re-exported so
/// `fe_ui::node_manager::TrackPickShape` is reachable outside this crate.
pub use path_segment_interaction::{TrackPickShape, TrackSourceWitness};

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
    selected: Option<NodeSelection>,
    additional: Vec<NodeSelection>,
    primary_order: usize,
    selection_petal: Option<String>,
    selection_anchor: Option<SelectionRow>,
    pub(super) pending_selection: Option<SelectionRequest>,
    cancelled_restores: Vec<CancelledRestore>,
    group_drag: Option<group_gimbal::GroupDrag>,
    selection_generation: u64,
    pending_transform_before: Option<Transform>,
    /// Sidebar click stores the node_id here; `sync_sidebar_to_manager`
    /// resolves the ECS Entity and calls `select()`.
    pub pending_sidebar_select: Option<String>,
    pub(super) pending_navigation_select: Option<NavigationSelection>,
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

pub(super) struct NavigationSelection {
    node_id: String,
    petal_id: String,
    remaining_frames: u8,
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

struct CancelledRestore {
    entity: Entity,
    node_id: String,
    transform: DragRestore,
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
const GESTURE_COUNT: usize = 7;

/// Pure aggregate for [`GestureParams::any_active`] — "a gesture is live" is
/// one OR over [`GESTURE_COUNT`] independent sources, unit-testable without a
/// Bevy `App` so a newly-added gesture that forgets the bundle trips a test
/// rather than shipping (finding F5: the entity `AxisDrag` was the sixth).
fn any_gesture_active(flags: [bool; GESTURE_COUNT]) -> bool {
    flags.iter().any(|live| *live)
}

/// Aggregate over the seven in-flight viewport gestures (road chain included) — one place to ask "is a
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
    /// Road builder: an open chain or in-flight segment; Escape drops one stage.
    road: ResMut<'w, road_chain::RoadBuilderState>,
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
            self.road.is_active(),
            // The entity gimbal's Move/Rotate/Scale `AxisDrag` (finding F5).
            self.node_mgr.is_dragging(),
        ])
    }

    /// Drop every in-flight gesture WITHOUT committing it. The entity gimbal
    /// drag additionally REVERTS its live-applied `Transform` to the press-time
    /// snapshot — the other five never write world state before their Release,
    /// so dropping the resource is their whole cancel.
    fn cancel_all(&mut self) {
        self.node_mgr.cancel_group_drag();
        for (entity, restore) in self.node_mgr.take_cancelled_restores() {
            if let Ok(mut transform) = self.transforms.get_mut(entity) {
                restore.apply(&mut transform);
            }
        }

        self.brush.cancel();
        self.path_point.active = None;
        self.pen_handle.active = None;
        self.path_handle.active = None;
        self.path_gimbal.active = None;
        // Two-stage: pending segment first, the open chain on the next press.
        self.road.cancel_stage();
        if let Some((entity, restore)) = self.node_mgr.cancel_axis_drag() {
            if let Ok(mut transform) = self.transforms.get_mut(entity) {
                restore.apply(&mut transform);
            }
        }
    }
}

impl NodeManager {
    pub fn clear_for_terminal_reset(&mut self) { *self = Self::default(); }

    /// Select after the destination petal materializes, within a bounded frame budget.
    pub fn request_select_after_navigation(&mut self, node_id: &str, petal_id: &str) {
        self.deselect();
        self.pending_sidebar_select = None;
        self.pending_navigation_select = Some(NavigationSelection {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            remaining_frames: 120,
        });
    }
    /// Queue selection through the normal entity resolver and frame its live transform.
    pub fn request_select_and_frame(
        &mut self,
        node_id: &str,
        fallback: [f32; 3],
        focus: &mut crate::plugin::CameraFocusTarget,
    ) {
        self.pending_navigation_select = None;
        self.pending_sidebar_select = Some(node_id.to_owned());
        focus.target = Some((node_id.to_owned(), fallback));
    }

    pub(crate) fn mark_transform_committed(
        &mut self,
        entity: Entity,
        before: Transform,
    ) -> Result<(), String> {
        if self.selection_count() != 1 || self.selected_entity() != Some(entity) {
            return Err("Selection changed before the transform could be committed".into());
        }
        self.selected.as_mut().unwrap().drag_committed = true;
        self.pending_transform_before = Some(before);
        Ok(())
    }

    pub(crate) fn invalidate_external_preview(&mut self, node_id: &str) {
        self.invalidate_transform_preview(node_id);
    }

    pub fn is_selected(&self) -> bool {
        self.selected.is_some()
    }

    pub fn selected_entity(&self) -> Option<Entity> {
        self.selected.as_ref().map(|s| s.entity)
    }

    pub fn is_dragging(&self) -> bool {
        self.group_drag.is_some() || self.selected.as_ref().is_some_and(|s| s.drag.is_some())
    }

    /// Select a node. If the same entity is already selected the drag state
    /// is preserved; selecting a different entity resets drag state.
    pub fn select(&mut self, entity: Entity, node_id: impl Into<String>) {
        let target = SelectionTarget {
            entity,
            node_id: node_id.into(),
        };
        let petal = self.selection_petal.clone();
        let _ = self.reduce_selection(petal.as_deref(), vec![target], SelectionMode::Replace);
    }

    /// Bind an authoritative promoted address to the same selected live stamp entity.
    pub(crate) fn bind_promoted_stamp(&mut self, entity: Entity, node_id: &str) -> bool {
        let duplicate = self
            .members()
            .any(|member| member.node_id == node_id && member.entity != entity);
        if duplicate {
            self.retain_members(|member| member.node_id != node_id || member.entity == entity);
        }
        let Some(selected) = self
            .selected
            .iter_mut()
            .chain(self.additional.iter_mut())
            .find(|member| member.entity == entity)
        else {
            return false;
        };
        if selected.node_id != node_id {
            selected.node_id = node_id.into();
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
        true
    }

    pub fn deselect(&mut self) {
        self.pending_navigation_select = None;
        self.pending_selection = None;
        self.replace_members(Vec::new(), None, None);
        self.selection_anchor = None;
    }

    /// A newer persisted snapshot wins over an unfinished local axis drag.
    pub(crate) fn accept_committed_transform(&mut self, node_id: &str) -> bool {
        self.invalidate_transform_preview(node_id)
    }

    fn invalidate_transform_preview(&mut self, node_id: &str) -> bool {
        let group_interrupted = self
            .group_drag
            .as_ref()
            .is_some_and(|drag| drag.contains_node(node_id));
        if group_interrupted {
            self.cancel_group_drag();
        }
        self.cancelled_restores
            .retain(|restore| restore.node_id != node_id);
        if let Some(selection) = self
            .selected
            .as_mut()
            .filter(|selection| selection.node_id == node_id)
        {
            self.pending_transform_before = None;
            let interrupted = selection.drag.is_some() || selection.drag_committed;
            selection.drag = None;
            selection.drag_committed = false;
            return interrupted || group_interrupted;
        }
        group_interrupted
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
        self.pending_transform_before=None;
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
        app.init_resource::<crate::undo::UndoState>();
        app.init_resource::<router::ClickArbiter>();
        app.init_resource::<brush_interaction::BrushGesture>();
        app.init_resource::<measure::MeasureState>();
        app.init_resource::<road_chain::RoadBuilderState>();
        app.init_resource::<road_interaction::RoadHoverState>();
        // HS-P6-CALLOUT: pending-stash + node bookkeeping for persisted
        // dimension callouts (see `actions::dimension_callout`).
        app.init_resource::<crate::actions::dimension_callout::DimensionCalloutMap>();
        app.init_resource::<path_point_interaction::PathPointDrag>();
        app.init_resource::<path_point_interaction::PenHandleDrag>();
        app.init_resource::<path_handle_interaction::PathHandleDrag>();
        app.init_resource::<path_gimbal_drag::PathGimbalDrag>();
        app.init_resource::<selection::SelectionState>();
        stamp_selection::register(app);
        // Before the action handlers, so a node write is authorized against this frame's active petal.
        app.add_systems(
            Update,
            crate::actions::node::drain_gui_scene_writes.before(crate::actions::process_ui_actions),
        );
        app.add_systems(Startup, crate::gimbal::configure_gimbal_gizmos);
        app.add_systems(
            Update,
            (
                // MUST stay first: it owns the staged-Escape ladder, so a rung-0
                // gesture cancel lands before any gesture system runs this frame.
                (
                    selected_set::reconcile_selection,
                    shortcuts::handle_tool_shortcuts,
                )
                    .chain(),
                sidebar_sync::sync_sidebar_to_manager,
                router::resolve_pointer_frame.in_set(fe_renderer::scene_pointer::ScenePointerContextSet),
                // This registration order is NORMATIVE for `ClickPriority` (D14-A):
                // Brush claims 2nd, ahead of every other consumer, while active;
                // Measure claims right after it and Road claims right after Measure
                // (nested only to stay within Bevy's 20-element tuple limit — the
                // inner `.chain()` keeps the order).
                (
                    brush_interaction::handle_brush_interaction, // Brush owns viewport gestures while active
                    measure::handle_measure_interaction, // Measure places anchors while active
                    road_interaction::handle_road_interaction, // Road places chain points while active
                )
                    .chain(),
                gimbal_interaction::update_hovered_axis, // hover detection (before interaction)
                path_handle_interaction::sync_path_handle_markers, // keep handle markers current before their pick
                path_handle_interaction::handle_path_handle_interaction, // claims PathHandle FIRST — Q7 handle > vertex > gimbal
                path_gimbal_drag::handle_path_gimbal_drag, // FR-3: claims Gimbal FIRST for a selected vertex/segment
                (
                    group_gimbal::interact,
                    gimbal_interaction::handle_gimbal_interaction,
                )
                    .chain(),
                path_point_interaction::sync_path_point_markers, // keep markers in sync with edit buffer
                path_point_interaction::handle_path_point_interaction, // claims PathMarker / PathPlace
                path_segment_interaction::handle_path_segment_interaction, // claims PathSegment — ribbon-segment select (FR-3)
                viewport_pick::handle_viewport_click, // claims NodePick — entity pick / deselect
                pointer::open_track_on_select, // clicking a track ribbon opens it for editing (re-homed FR-3)
                inspector_sync::sync_manager_to_inspector,
                selection::update_selection_state, // FR-1: project the read-model before the gimbal draws
                path_handle_interaction::draw_handle_stems, // anchor→handle stems (draws only, steals no clicks)
                (
                    gimbal_interaction::draw_gimbal_system,
                    gimbal_interaction::draw_selection_outline_system, // QA-0006: visible feedback even in Select tool
                    terrain_authoring::draw_terrain_draft,
                )
                    .chain(),
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
                measure::draw_measure_overlay.after(measure::handle_measure_interaction),
                // The road readout is egui, registered in EguiPrimaryContextPass by plugin.rs.
                road_overlay::draw_road_overlay.after(road_interaction::handle_road_interaction),
            )
                .in_set(UiSet::Selection),
        );
        // Billboard facing is orientation-only and order-independent of the
        // selection chain above, so it runs as a standalone per-frame system
        // (data_icons_20260713).
        app.add_systems(Update, billboard::billboard_face_camera);
        // HS-P6-CALLOUT: dimension callouts draw unconditionally (not
        // mode-gated, outside the chain) — a saved callout stays visible
        // while any tool is active, not just while Measure is.
        app.add_systems(
            Update,
            crate::actions::dimension_callout::draw_dimension_callouts,
        );
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
        let target = |id: u32| SelectionTarget {
            entity: entity(id),
            node_id: format!("node-{id}"),
        };
        mgr.reduce_selection(Some("petal"), vec![target(1)], SelectionMode::Replace)
            .unwrap();
        let generation = mgr.generation();
        mgr.reduce_selection(Some("petal"), vec![target(2)], SelectionMode::Toggle)
            .unwrap();
        mgr.reduce_selection(Some("petal"), vec![target(3)], SelectionMode::Toggle)
            .unwrap();
        assert_eq!(
            mgr.members()
                .map(|member| member.node_id.as_str())
                .collect::<Vec<_>>(),
            ["node-1", "node-2", "node-3"]
        );
        assert_eq!(mgr.primary().unwrap().node_id, "node-3");
        assert!(mgr.generation() > generation);
        mgr.reduce_selection(Some("petal"), vec![target(2)], SelectionMode::Toggle)
            .unwrap();
        assert_eq!(
            mgr.members()
                .map(|member| member.node_id.as_str())
                .collect::<Vec<_>>(),
            ["node-1", "node-3"]
        );
        mgr.reduce_selection(Some("petal"), vec![target(3)], SelectionMode::Toggle)
            .unwrap();
        assert_eq!(mgr.primary().unwrap().node_id, "node-1");
        mgr.selection_anchor = Some(SelectionRow::all("petal", "node-3"));
        let request = SelectionRequest {
            petal_id: "petal".into(),
            node_ids: vec!["node-1".into()],
            mode: SelectionMode::Range,
            clicked_row: None,
            visible_order: vec![
                SelectionRow::all("petal", "node-1"),
                SelectionRow::all("petal", "node-2"),
                SelectionRow::all("petal", "node-2"),
                SelectionRow::all("petal", "node-3"),
            ],
        };
        assert_eq!(mgr.requested_ids(&request), ["node-1", "node-2", "node-3"]);
        mgr.reduce_requested_selection(&request, vec![target(1), target(2), target(3)])
            .unwrap();
        assert_eq!(
            mgr.primary().unwrap().node_id,
            "node-1",
            "reverse ranges retain the clicked primary"
        );
        assert_eq!(mgr.selection_count(), 3);
        let row = |group: &str, node: &str| SelectionRow {
            petal_id: "petal".into(), section: SelectionSection::Group(group.into()), node_id: node.into()
        };
        let aliases = vec![row("a", "node-1"), row("a", "node-2"), row("b", "node-1"), row("b", "node-3")];
        mgr.request_row_selection(aliases[2].clone(), SelectionMode::Replace, aliases.clone());
        let clicked = mgr.pending_selection.take().unwrap();
        mgr.reduce_requested_selection(&clicked, vec![target(1)]).unwrap();
        mgr.request_row_selection(aliases[3].clone(), SelectionMode::Range, aliases.clone());
        let range = mgr.pending_selection.take().unwrap();
        assert_eq!(mgr.requested_ids(&range), ["node-1", "node-3"], "range starts at the clicked second alias, not the first semantic match");
        mgr.request_row_selection(aliases[0].clone(), SelectionMode::Range, aliases);
        let reverse = mgr.pending_selection.take().unwrap();
        assert_eq!(mgr.requested_ids(&reverse), ["node-1", "node-2"], "deduplicate semantic IDs only after extracting the visual range");
        mgr.reduce_requested_selection(&request, vec![target(1), target(2), target(3)]).unwrap();
        let generation = mgr.generation();
        assert!(mgr
            .reduce_selection(
                Some("petal"),
                vec![target(1), target(1)],
                SelectionMode::Replace
            )
            .is_err());
        assert!(mgr
            .reduce_selection(
                Some("petal"),
                (1..=257).map(target).collect(),
                SelectionMode::Replace
            )
            .is_err());
        assert_eq!(
            mgr.generation(),
            generation,
            "a refused set cannot partially change selection"
        );
        for mode in [SelectionMode::Extend, SelectionMode::Toggle] {
            assert!(mgr
                .reduce_selection(
                    Some("petal"),
                    vec![SelectionTarget {
                        entity: entity(1),
                        node_id: "alias-for-existing-entity".into()
                    }],
                    mode
                )
                .is_err());
            assert_eq!(
                mgr.generation(),
                generation,
                "post-merge entity aliases refuse the complete operation"
            );
            assert_eq!(mgr.selection_count(), 3);
        }
        mgr.reduce_selection(Some("other-petal"), vec![target(4)], SelectionMode::Toggle)
            .unwrap();
        assert_eq!(
            mgr.selection_count(),
            1,
            "a Petal change never carries old members"
        );
        assert_eq!(mgr.selected_petal(), Some("other-petal"));
        mgr.selected.as_mut().unwrap().drag = Some(AxisDrag {
            axis: crate::gimbal::GimbalAxis::X,
            start_cursor: Vec2::ZERO,
            start_pos: Vec3::ZERO,
            start_rot: Quat::IDENTITY,
            start_scale: Vec3::ONE,
            axis_screen_dir: Vec2::X,
        });
        mgr.deselect();
        assert!(mgr.members().next().is_none());
        mgr.accept_committed_transform("node-4");
        assert!(
            mgr.take_cancelled_restores().is_empty(),
            "a receipt for a deselected node removes its old queued restore by semantic ID"
        );
        mgr.select(entity(4), "node-4");
        mgr.selected.as_mut().unwrap().drag = Some(AxisDrag {
            axis: GimbalAxis::X,
            start_cursor: Vec2::ZERO,
            axis_screen_dir: Vec2::X,
            start_pos: Vec3::ZERO,
            start_rot: Quat::IDENTITY,
            start_scale: Vec3::ONE,
        });
        mgr.deselect();
        mgr.invalidate_external_preview("node-4");
        assert!(mgr.take_cancelled_restores().is_empty(), "optimistic incoming transforms also fence obsolete restores without becoming committed history");
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
    fn any_gesture_active_is_armed_by_each_of_the_seven_sources() {
        // The aggregate is one OR over seven independent gestures (the road
        // chain is the seventh) — a bundle that forgets one fails this arity check.
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
        // `any_active` feeds `is_dragging()` in as its last flag, so an
        // entity transform drag pins Escape at rung 0 and suppresses the
        // right-click object menu like every other gesture.
        let mut mgr = NodeManager::default();
        assert!(!any_gesture_active([
            false,
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
            false,
            mgr.is_dragging()
        ]));
    }

    /// Load `node-1`'s stored capture: a single-object gimbal commits only on the
    /// ordinary-single transform route, which needs the selection's capture.
    fn seed_single_capture(app: &mut App) {
        app.world_mut()
            .resource_scope(|world, mut ui: Mut<crate::actions::UiManager>| {
                let (batch_tx, batch_rx) = crossbeam::channel::unbounded();
                let nav = world.resource::<crate::navigation_manager::NavigationManager>();
                ui.editor_batch.maintain(
                    nav,
                    world.resource::<NodeManager>(),
                    world.resource::<crate::verse_manager::VerseManager>(),
                    &batch_tx,
                );
                let Ok(fe_runtime::messages::DbCommand::ReadEditorBatch { correlation_id, .. }) = batch_rx.try_recv()
                else {
                    panic!("the selection requests its stored capture")
                };
                let state: fe_runtime::node_history::NodeRowSnapshot =
                    serde_json::from_value(serde_json::json!({
                        "node_id": "node-1", "petal_id": "p", "position": [1.0, 2.0, 3.0],
                        "elevation": 2.0, "rotation": [0.0, 0.0, 0.0], "scale": [2.0, 2.0, 2.0],
                        "interactive": true, "created_at": "test", "edit_seq": 0, "properties": {}
                    }))
                    .unwrap();
                let capture = fe_runtime::editor_batch::EditorBatchCapture {
                    target: fe_runtime::editor_batch::EditorBatchTarget::Nodes,
                    deleted_group: None,
                    scope: "VERSE#v-FRACTAL#f-PETAL#p".into(),
                    petal_id: "p".into(),
                    members: vec![fe_runtime::editor_batch::EditorBatchMember {
                        node_id: "node-1".into(),
                        history_version: Some(1),
                        stamp_path: None,
                        state,
                    }],
                    groups: vec![],
                    petal_snapshot: "fixture".into(),
                    world_scale: 1.0,
                    terrain_source_fingerprint: None,
                };
                ui.editor_batch
                    .route(
                        &fe_runtime::messages::DbResult::EditorBatchLoaded {
                            capture,
                            correlation_id,
                        },
                        nav,
                        |_| Ok(()),
                        |_| Ok(()),
                        &batch_tx,
                    )
                    .expect("the stored capture loads");
            });
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

        use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry, VerseManager};
        use fe_runtime::messages::{DbCommand, DbResult, NodeMutation};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<DbResult>();
        let nav = crate::navigation_manager::NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("f".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        };
        app.insert_resource(nav)
            .init_resource::<crate::undo::UndoState>()
            .init_resource::<NodeManager>()
            .init_resource::<router::ClickArbiter>()
            .init_resource::<crate::actions::UiManager>()
            .init_resource::<crate::visibility::VisibilityState>();
        app.insert_resource(crate::plugin::ToolState {
            active_tool: crate::panels::toolbar::Tool::Move,
            ..Default::default()
        });
        app.insert_resource(VerseManager::from_verses(vec![VerseEntry {
            id: "v".into(),
            fractals: vec![FractalEntry {
                id: "f".into(),
                petals: vec![PetalEntry {
                    id: "p".into(),
                    nodes: vec![NodeEntry {
                        id: "node-1".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }]));
        let (tx, rx) = crossbeam::channel::unbounded();
        app.insert_resource(fe_runtime::app::DbCommandSender(tx));
        let after = Transform::from_xyz(4.0, 5.0, 6.0)
            .with_rotation(Quat::from_rotation_z(0.7))
            .with_scale(Vec3::new(3.0, 4.0, 5.0));
        let target = app
            .world_mut()
            .spawn((
                after,
                GlobalTransform::default(),
                crate::plugin::SpawnedNodeMarker {
                    node_id: "node-1".into(),
                    petal_id: "p".into(),
                },
            ))
            .id();
        app.world_mut().spawn(bevy::window::Window::default());
        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::default(),
            fe_renderer::camera::OrbitCameraController::default(),
        ));
        {
            let mut manager = app.world_mut().resource_mut::<NodeManager>();
            manager.select(target, "node-1");
            begin_axis_drag(&mut manager);
        }
        seed_single_capture(&mut app);
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        mouse.clear();
        mouse.release(MouseButton::Left);
        app.insert_resource(mouse);
        app.add_systems(
            Update,
            (
                crate::undo::observe_results,
                gimbal_interaction::handle_gimbal_interaction,
                transform_broadcast::broadcast_transform,
            )
                .chain(),
        );
        // Refusing gate: the one scene door refuses, so no command is sent, the entity returns
        // to its pre-drag pose and the player sees "Refused by <extension>".
        {
            use fe_runtime::scene_hook_gate::{
                AugmentOp, SceneBinding, SceneHook, SceneHookBackend, SceneHookCall,
                SceneHookGate, SceneHookReply, SceneHookReplySender,
            };
            struct RefusingBackend;
            impl SceneHookBackend for RefusingBackend {
                fn local_did(&self) -> &str {
                    "did:key:local"
                }
                fn bound(&self, _hook: SceneHook, petal_id: &str) -> Option<SceneBinding> {
                    (petal_id == "p").then(|| SceneBinding {
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
            let (door_tx, door_rx) = crossbeam::channel::unbounded();
            app.insert_resource(fe_runtime::scene_write::SceneWriteDoor::new(
                SceneHookGate::new(std::sync::Arc::new(RefusingBackend)),
                door_tx,
            ));
            app.update();
            assert!(rx.try_recv().is_err(), "a refused gimbal commit sends no command");
            assert!(door_rx.try_recv().is_err(), "the door never sends a refused write");
            let restored = *app.world().get::<Transform>(target).unwrap();
            assert_eq!(restored.translation, Vec3::new(1.0, 2.0, 3.0), "entity back at before");
            assert_eq!(restored.scale, Vec3::splat(2.0));
            let advisory = app
                .world()
                .resource::<crate::actions::UiManager>()
                .active_notice(0.0)
                .map(|(notice, _)| notice.text.clone())
                .unwrap_or_default();
            assert!(
                advisory.starts_with("Refused by org.example.zoning"),
                "advisory shown, got {advisory:?}"
            );
            // Lift the refusal and replay the same gesture for the committed journey below.
            app.world_mut()
                .remove_resource::<fe_runtime::scene_write::SceneWriteDoor>();
            *app.world_mut().get_mut::<Transform>(target).unwrap() = after;
            begin_axis_drag(&mut app.world_mut().resource_mut::<NodeManager>());
            let mut replay = ButtonInput::<MouseButton>::default();
            replay.press(MouseButton::Left);
            replay.clear();
            replay.release(MouseButton::Left);
            app.insert_resource(replay);
        }
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (0, 0)
        );
        let command = rx.try_recv().expect("release sends one checked operation");
        let DbCommand::CommitCheckedNodeMutation {
            expected,
            mutation,
            correlation_id,
            ..
        } = command
        else {
            panic!("release must use checked commit, never legacy UpdateNodeTransform")
        };
        let fe_runtime::node_history::NodeEditBefore::Transform(before) = expected.before else {
            panic!("full starting transform required")
        };
        assert_eq!(before.position, [1.0, 2.0, 3.0]);
        assert_eq!(before.scale, [2.0; 3]);
        assert!(before.matches(&crate::undo::transform_value(
            &Transform::from_xyz(1.0, 2.0, 3.0)
                .with_rotation(Quat::from_rotation_y(0.5))
                .with_scale(Vec3::splat(2.0))
        )));
        let NodeMutation::FullTransform {
            position,
            rotation,
            scale,
        } = mutation
        else {
            panic!("full transform required")
        };
        assert_eq!(position, [4.0, 5.0, 6.0]);
        assert_eq!(scale, [3.0, 4.0, 5.0]);
        assert!(rx.try_recv().is_err(), "one release must not double-commit");
        let mut state:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(serde_json::json!({"node_id":"node-1","petal_id":"p","position":position,"elevation":position[1],"rotation":rotation,"scale":scale,"interactive":false,"created_at":"test","edit_seq":1,"properties":{"nested":{"gain":2}}})).unwrap();
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 1,
                    state: state.clone(),
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: Some(correlation_id),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (1, 0)
        );
        app.world_mut().resource_mut::<NodeManager>().deselect();
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.replay(
                    false,
                    world.resource(),
                    world.resource(),
                    world.resource(),
                    &world.resource::<fe_runtime::app::DbCommandSender>().0,
                )
                .unwrap()
            });
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (1, 0),
            "Undo waits for its own receipt"
        );
        let DbCommand::CommitCheckedNodeMutation {
            mutation:
                NodeMutation::FullTransform {
                    position,
                    rotation,
                    scale,
                },
            expected,
            correlation_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("checked inverse expected")
        };
        assert_eq!(expected.history_version, Some(1));
        assert_eq!(position, [1.0, 2.0, 3.0]);
        assert_eq!(scale, [2.0; 3]);
        state.position = position.map(f64::from);
        state.elevation = state.position[1];
        state.rotation = rotation.map(f64::from).to_vec();
        state.scale = scale.map(f64::from);
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 2,
                    state: state.clone(),
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: Some(correlation_id),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (0, 1)
        );
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.replay(
                    true,
                    world.resource(),
                    world.resource(),
                    world.resource(),
                    &world.resource::<fe_runtime::app::DbCommandSender>().0,
                )
                .unwrap()
            });
        let DbCommand::CommitCheckedNodeMutation {
            mutation:
                NodeMutation::FullTransform {
                    position,
                    rotation,
                    scale,
                },
            expected,
            correlation_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("checked redo expected")
        };
        assert_eq!(expected.history_version, Some(2));
        assert_eq!(position, [4.0, 5.0, 6.0]);
        assert_eq!(scale, [3.0, 4.0, 5.0]);
        state.position = position.map(f64::from);
        state.elevation = state.position[1];
        state.rotation = rotation.map(f64::from).to_vec();
        state.scale = scale.map(f64::from);
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 3,
                    state,
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: Some(correlation_id),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (1, 0)
        );
        let proof_nav = crate::navigation_manager::NavigationManager {
            active_verse_id: Some("v".into()),
            active_fractal_id: Some("f".into()),
            active_petal_id: Some("p".into()),
            ..Default::default()
        };
        let points = serde_json::json!([
            [1.0, 0.0, 2.0, 17.25, -0.5, 0.0, 0.0, 0.5, 0.0, 0.0, 2.0, 0.6],
            [8.0, 2.0, 9.0, 29.5]
        ]);
        for reordered in [false,true] {
            let mut state=crate::gis::PathEditorState::default();state.start_editing("node-1".into());
            state.points=crate::gis::decode_gpx_points(&points);
            let original=state.points.clone();
            let origin=crate::undo::PointEditOrigin::capture(&state,&proof_nav).unwrap();
            let mut corner_undo=crate::undo::UndoState::default();let mut corner_ops=crate::path_ops::PendingPathOps::default();
            let id=crate::actions::path::queue_checked_corner(&mut corner_undo,&proof_nav,app.world().resource::<VerseManager>(),&mut corner_ops,&state,"node-1".into(),0,crate::gis::CornerKind::Corner,&origin).unwrap().unwrap();
            assert_eq!(state.points,original,"Corner dispatch cannot mutate authoritative editor rows");
            assert!(matches!(&corner_ops.0[..],[crate::path_ops::PathOp::CheckedAnchorMetadata {request_id,..}] if request_id==&id));
            assert!(matches!(corner_undo.path_read_command(&id).unwrap(),DbCommand::ResolveCheckedNodeMutation {..}));
            let mut stored=points.clone();if reordered {stored.as_array_mut().unwrap().swap(0,1);}
            let current:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(serde_json::json!({"node_id":"node-1","petal_id":"p","position":[0.0,0.0,0.0],"elevation":0.0,"rotation":[0.0,0.0,0.0],"scale":[1.0,1.0,1.0],"interactive":false,"created_at":"test","edit_seq":3,"properties":{"gpx_points":stored}})).unwrap();
            let (tx,corner_rx)=crossbeam::channel::unbounded();
            let result=corner_undo.commit_path(&proof_nav,&id,&current,&tx);
            if reordered {assert!(result.is_err() && corner_rx.try_recv().is_err(),"reorder after dispatch before readback cannot write to a reused index");}
            else {
                result.unwrap();let DbCommand::CommitCheckedNodeMutation {mutation:NodeMutation::PropertySet {value,..},expected,..}=corner_rx.try_recv().unwrap() else {panic!("checked corner commit required")};
                let mut wanted=points.clone();wanted[0][10]=serde_json::json!(0.0);
                assert_eq!(value,wanted,"only the intended corner field changes");
                assert!(matches!(expected.before,fe_runtime::node_history::NodeEditBefore::PathPoints(ref before) if before==&points));
            }
            assert!(corner_undo.take_path_projection(&proof_nav).is_none(),"no corner projection before authoritative receipt");
        }
        let removal_rows = crate::gis::decode_gpx_points(&points);
        for stale in [false, true] {
            let mut removal = crate::undo::UndoState::default();
            let request = removal.reserve_path_remove(&proof_nav, "node-1", 0, Some(3), &removal_rows).unwrap();
            let mut current: fe_runtime::node_history::NodeRowSnapshot = serde_json::from_value(serde_json::json!({
                "node_id":"node-1", "petal_id":"p", "position":[0.0,0.0,0.0], "elevation":0.0,
                "rotation":[0.0,0.0,0.0], "scale":[1.0,1.0,1.0], "interactive":false,
                "created_at":"test", "edit_seq":3, "properties":{"gpx_points":points}
            })).unwrap();
            if stale { current.properties.get_mut("gpx_points").unwrap()[1][0] = serde_json::json!(999); }
            let (tx, rx_remove) = crossbeam::channel::unbounded();
            let result = removal.commit_path(&proof_nav, &request, &current, &tx);
            if stale {
                assert!(result.is_err() && rx_remove.try_recv().is_err(), "reindexed route cannot remove another point");
            } else {
                result.unwrap();
                let DbCommand::CommitCheckedNodeMutation { mutation: NodeMutation::PropertySet { value, .. }, expected, .. } = rx_remove.try_recv().unwrap() else { panic!("checked removal required") };
                assert_eq!(value, serde_json::json!([points[1].clone()]));
                assert!(matches!(expected.before, fe_runtime::node_history::NodeEditBefore::PathPoints(ref before) if before == &points));
                assert_eq!(expected.history_version, Some(3));
                assert!(removal.take_path_projection(&proof_nav).is_none(), "no optimistic removal");
            }
        }
        let rows = crate::gis::decode_gpx_points(&points);
        let id = app
            .world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.reserve_path(
                    world.resource(),
                    "node-1",
                    0,
                    [1.0, 0.0, 2.0],
                    [4.0, 3.0, 6.0],
                    Some(3),
                    &rows,
                )
                .unwrap()
                .unwrap()
            });
        let point_current:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(serde_json::json!({"node_id":"node-1","petal_id":"p","position":[4.0,5.0,6.0],"elevation":5.0,"rotation":[0.0,0.0,0.7],"scale":[3.0,4.0,5.0],"interactive":false,"created_at":"test","edit_seq":3,"properties":{"nested":{"gain":2},"gpx_points":points}})).unwrap();
        let read = app
            .world()
            .resource::<crate::undo::UndoState>()
            .path_read_command(&id)
            .unwrap();
        assert!(matches!(read,DbCommand::ResolveCheckedNodeMutation {request_id,..} if request_id==id));
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.commit_path(
                    world.resource(),
                    &id,
                    &point_current,
                    &world.resource::<fe_runtime::app::DbCommandSender>().0,
                )
                .unwrap()
            });
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (1, 0)
        );
        assert!(
            app.world_mut()
                .resource_mut::<crate::undo::UndoState>()
                .take_path_projection(&proof_nav)
                .is_none(),
            "queued move must not project as confirmed"
        );
        let DbCommand::CommitCheckedNodeMutation {
            mutation: NodeMutation::PropertySet { value, .. },
            expected,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("point move uses checked property commit")
        };
        assert!(
            matches!(expected.before,fe_runtime::node_history::NodeEditBefore::PathPoints(ref before) if before==&points)
        );
        assert_eq!(
            value[0].as_array().unwrap()[3..],
            points[0].as_array().unwrap()[3..]
        );
        assert_eq!(value[1], points[1]);
        let mut path_state:fe_runtime::node_history::NodeRowSnapshot=serde_json::from_value(serde_json::json!({"node_id":"node-1","petal_id":"p","position":[4.0,5.0,6.0],"elevation":5.0,"rotation":[0.0,0.0,0.7],"scale":[3.0,4.0,5.0],"interactive":false,"created_at":"test","edit_seq":3,"properties":{"nested":{"gain":2},"gpx_points":value}})).unwrap();
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 4,
                    state: path_state.clone(),
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: Some(id),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (2, 0)
        );
        let projection = app
            .world_mut()
            .resource_mut::<crate::undo::UndoState>()
            .take_path_projection(&proof_nav)
            .unwrap();
        assert_eq!(projection.points, path_state.properties["gpx_points"]);
        assert_eq!(projection.editor_rows()[0].time_seconds, Some(17.25));
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.replay(
                    false,
                    world.resource(),
                    world.resource(),
                    world.resource(),
                    &world.resource::<fe_runtime::app::DbCommandSender>().0,
                )
                .unwrap()
            });
        let DbCommand::CommitCheckedNodeMutation {
            mutation: NodeMutation::PropertySet { value, .. },
            expected,
            correlation_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("point inverse uses checked property commit")
        };
        assert_eq!(expected.history_version, Some(4));
        assert_eq!(value, points);
        path_state.properties.insert("gpx_points".into(), value);
        app.world_mut()
            .write_message(DbResult::NodeMutationCommitted {
                receipt: fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 5,
                    state: path_state,
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                },
                correlation_id: Some(correlation_id),
            });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (1, 1)
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<crate::undo::UndoState>()
                .take_path_projection(&proof_nav)
                .unwrap()
                .points,
            points
        );
        // These boundary receipts exercise the real release/observer flow, not DB persistence.
        app.insert_resource(crate::undo::UndoState::default());
        app.init_resource::<crate::verse_manager::PathAssetCache>();
        let baseline = Transform::from_xyz(4.0, 5.0, 6.0)
            .with_rotation(Quat::from_rotation_z(0.7))
            .with_scale(Vec3::new(3.0, 4.0, 5.0));
        let edit_a = Transform::from_xyz(7.0, 8.0, 9.0)
            .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.2, 0.4, 0.6))
            .with_scale(Vec3::new(2.0, 3.0, 4.0));
        let edit_b = Transform::from_xyz(10.0, 11.0, 12.0)
            .with_rotation(Quat::from_rotation_y(0.9))
            .with_scale(Vec3::new(4.0, 5.0, 6.0));
        let mut chain_state = point_current.clone();
        for (before, after, version) in [(baseline, edit_a, 6), (edit_a, edit_b, 7)] {
            app.world_mut()
                .resource_mut::<VerseManager>()
                .committed_versions
                .insert("node-1".into(), version - 1);
            *app.world_mut().get_mut::<Transform>(target).unwrap() = after;
            {
                let mut manager = app.world_mut().resource_mut::<NodeManager>();
                manager.select(target, "node-1");
                manager.selected.as_mut().unwrap().drag = Some(AxisDrag {
                    axis: crate::gimbal::GimbalAxis::X,
                    start_cursor: Vec2::ZERO,
                    axis_screen_dir: Vec2::X,
                    start_pos: before.translation,
                    start_rot: before.rotation,
                    start_scale: before.scale,
                });
            }
            seed_single_capture(&mut app);
            {
                let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
                mouse.press(MouseButton::Left);
                mouse.clear();
                mouse.release(MouseButton::Left);
            }
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            let DbCommand::CommitCheckedNodeMutation {
                mutation:
                    NodeMutation::FullTransform {
                        position,
                        rotation,
                        scale,
                    },
                expected,
                correlation_id,
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("each release must be one checked command")
            };
            assert_eq!(expected.history_version, Some(version - 1));
            chain_state.position = position.map(f64::from);
            chain_state.elevation = chain_state.position[1];
            chain_state.rotation = rotation.map(f64::from).to_vec();
            chain_state.scale = scale.map(f64::from);
            app.world_mut()
                .write_message(DbResult::NodeMutationCommitted {
                    receipt: fe_runtime::node_history::NodeMutationReceipt {
                        node_id: "node-1".into(),
                        history_version: version,
                        state: chain_state.clone(),
                        persist: fe_runtime::node_history::PersistStatus::Committed,
                    },
                    correlation_id: Some(correlation_id),
                });
            app.update();
            app.world_mut()
                .resource_mut::<VerseManager>()
                .committed_versions
                .insert("node-1".into(), version);
        }
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (2, 0)
        );
        for (redo, version, wanted, counts) in [
            (false, 8, edit_a, (1, 1)),
            (false, 9, baseline, (0, 2)),
            (true, 10, edit_a, (1, 1)),
            (true, 11, edit_b, (2, 0)),
        ] {
            app.world_mut()
                .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                    undo.replay(
                        redo,
                        world.resource(),
                        world.resource(),
                        world.resource(),
                        &world.resource::<fe_runtime::app::DbCommandSender>().0,
                    )
                    .unwrap()
                });
            let DbCommand::CommitCheckedNodeMutation {
                mutation:
                    NodeMutation::FullTransform {
                        position,
                        rotation,
                        scale,
                    },
                expected,
                correlation_id,
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("checked replay required")
            };
            assert_eq!(
                expected.history_version,
                Some(version - 1),
                "the next same-node command must adopt only the verified cursor"
            );
            chain_state.position = position.map(f64::from);
            chain_state.elevation = chain_state.position[1];
            chain_state.rotation = rotation.map(f64::from).to_vec();
            chain_state.scale = scale.map(f64::from);
            assert!(
                fe_runtime::node_history::NodeTransformValue::from_row(&chain_state)
                    .unwrap()
                    .matches(&crate::undo::transform_value(&wanted))
            );
            app.world_mut()
                .write_message(DbResult::NodeMutationCommitted {
                    receipt: fe_runtime::node_history::NodeMutationReceipt {
                        node_id: "node-1".into(),
                        history_version: version,
                        state: chain_state.clone(),
                        persist: fe_runtime::node_history::PersistStatus::Committed,
                    },
                    correlation_id: Some(correlation_id),
                });
            app.update();
            app.world_mut()
                .resource_mut::<VerseManager>()
                .committed_versions
                .insert("node-1".into(), version);
            assert_eq!(
                app.world()
                    .resource::<crate::undo::UndoState>()
                    .history_counts(),
                counts
            );
        }
        let rows = crate::gis::decode_gpx_points(&points);
        let request = app
            .world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.reserve_path(
                    world.resource(),
                    "node-1",
                    0,
                    [1.0, 0.0, 2.0],
                    [4.0, 3.0, 6.0],
                    Some(11),
                    &rows,
                )
                .unwrap()
                .unwrap()
            });
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                undo.commit_path(
                    world.resource(),
                    &request,
                    &chain_state,
                    &world.resource::<fe_runtime::app::DbCommandSender>().0,
                )
                .unwrap()
            });
        let DbCommand::CommitCheckedNodeMutation {
            mutation: NodeMutation::PropertySet { value, .. },
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("checked point edit required")
        };
        let mut saved = chain_state.clone();
        saved.properties.insert("gpx_points".into(), value);
        app.world_mut().write_message(DbResult::RequestFailed {
            correlation_id: request.clone(),
            error: "confirmation unavailable".into(),
        });
        app.update();
        let DbCommand::ResolveCheckedNodeMutation {
            request_id,
            correlation_id: resolve_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("only exact receipt recovery may be queued")
        };
        assert_eq!(request_id, request);
        assert!(rx.try_recv().is_err());
        let mut current = saved.clone();
        let mut newer_points = current.properties["gpx_points"].clone();
        newer_points[0][0] = serde_json::json!(100.0);
        newer_points[0][2] = serde_json::json!(200.0);
        current
            .properties
            .insert("gpx_points".into(), newer_points.clone());
        current.properties.insert(
            "path_asset".into(),
            serde_json::json!({"asset_path":"blob://tree.glb","spacing_value":2.0}),
        );
        {
            let mut ui = app.world_mut().resource_mut::<crate::actions::UiManager>();
            while let Some((notice, _)) = ui.active_notice(0.0) { let id = notice.id; ui.dismiss_notice(id); }
        }
        app.world_mut()
            .write_message(DbResult::CheckedNodeMutationResolved {
                node_id: "node-1".into(),
                request_id: request,
                receipt: Some(fe_runtime::node_history::NodeMutationReceipt {
                    node_id: "node-1".into(),
                    history_version: 12,
                    state: saved,
                    persist: fe_runtime::node_history::PersistStatus::Committed,
                }),
                current: current.clone(),
                correlation_id: resolve_id,
            });
        app.update();
        assert!(app.world().resource::<crate::undo::UndoState>().queued_path_projection().is_none(), "unversioned current reads cannot enter the committed projection queue");
        assert_eq!(
            app.world()
                .resource::<crate::verse_manager::PathAssetCache>()
                .cached_point_positions("node-1")
                .unwrap()[0],
            [100.0, 3.0, 200.0]
        );
        let hydration = rx.try_iter().collect::<Vec<_>>();
        assert!(hydration.iter().any(|command| matches!(command, DbCommand::LoadHierarchy)));
        assert!(hydration.iter().any(|command| matches!(command, DbCommand::GetNodeProperties { node_id, .. } if node_id == "node-1")));
        assert!(app.world().resource::<crate::actions::UiManager>().active_notice(0.0).is_some_and(|(notice, _)| notice.text.contains("no current version")));
        let rows = crate::gis::decode_gpx_points(&newer_points);
        let old_read =
            app.world_mut()
                .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                    undo.reserve_path(
                        world.resource(),
                        "node-1",
                        0,
                        [100.0, 3.0, 200.0],
                        [101.0, 3.0, 200.0],
                        None,
                        &rows,
                    )
                    .unwrap()
                    .unwrap()
                });
        app.world_mut()
            .resource_mut::<crate::navigation_manager::NavigationManager>()
            .navigate_to_petal("other-petal");
        // No observer update occurs between navigation and this bridge-use boundary.
        app.world_mut()
            .resource_scope(|world, mut undo: Mut<crate::undo::UndoState>| {
                assert!(undo
                    .commit_path(
                        world.resource(),
                        &old_read,
                        &current,
                        &world.resource::<fe_runtime::app::DbCommandSender>().0
                    )
                    .is_err());
                assert!(undo.take_path_projection(world.resource()).is_none());
            });
        assert!(
            rx.try_recv().is_err(),
            "a late read must not enqueue a covered write after navigation"
        );
        app.world_mut()
            .resource_mut::<crate::navigation_manager::NavigationManager>()
            .navigate_to_petal("other-petal");
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            (0, 0)
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

        for (value, step, expected) in [(-1.5, 1.0, -2.0), (-0.5, 1.0, -1.0), (0.5, 1.0, 1.0), (2.24, 0.5, 2.0)] {
            assert_eq!(transform_snap::snap_scalar(value, step), expected);
        }
        assert_eq!(transform_snap::snap_scalar(2.25, f32::NAN), 2.25);
        let snap = ToolSnapSettings { grid_enabled: true, angle_enabled: true, scale_enabled: true, ..Default::default() };
        assert_eq!(snap.move_position(Vec3::new(1.0, 2.0, 3.0), GimbalAxis::X, 1.2, 2.0), Vec3::new(2.0, 2.0, 3.0));
        assert!((snap.rotation_angle(20.0_f32.to_radians()).to_degrees() - 15.0).abs() < 0.001);
        assert_eq!(snap.scale_value(Vec3::splat(2.0), GimbalAxis::X, 1.12), Vec3::new(2.2, 2.0, 2.0));
        assert!((ToolSnapSettings { uniform_scale:true, ..snap }.scale_value(Vec3::new(2.0,3.0,4.0), GimbalAxis::X, 1.12)-Vec3::new(2.2,3.3,4.4)).length()<0.00001);
        assert_eq!(ToolSnapSettings { axis:TransformAxisLock::Y, ..snap }.move_position(Vec3::ONE,GimbalAxis::X,5.0,1.0),Vec3::ONE);

        for enabled in [false, true] {
            let settings = ToolSnapSettings {
                grid_enabled: enabled,
                scale_enabled: enabled,
                ..Default::default()
            };
            let uniform = ToolSnapSettings { uniform_scale: true, ..settings };
            for start in [
                Vec3::new(f32::MAX, 2.0, 3.0),
                Vec3::new(-f32::MAX, 2.0, 3.0),
            ] {
                assert_eq!(
                    settings.move_position(start, GimbalAxis::X, start.x, 1.0),
                    start
                );
                assert_eq!(settings.scale_value(start, GimbalAxis::X, 2.0), start);
                assert_eq!(uniform.scale_value(start, GimbalAxis::X, 2.0), start);
            }
            let start = Vec3::new(1.0, f32::MAX, 3.0);
            assert_eq!(uniform.scale_value(start, GimbalAxis::X, 2.0), start);
        }

        let mut app = App::new();
        app.init_resource::<NodeManager>()
            .init_resource::<crate::undo::UndoState>()
            .init_resource::<router::ClickArbiter>()
            .insert_resource(crate::plugin::ToolState { active_tool:crate::panels::toolbar::Tool::Move, snap, ..Default::default() })
            .insert_resource(crate::terrain_map::PetalMapState { loaded:true,petal_id:Some("snap-petal".into()),world_scale:2.0,terrain_json:Some(serde_json::json!({"enabled":true})),..Default::default() })
            .insert_resource(crate::navigation_manager::NavigationManager { active_petal_id:Some("snap-petal".into()),..Default::default() });
        let mut window=Window::default();
        window.set_cursor_position(Some(Vec2::new(10.0,0.0)));
        app.world_mut().spawn(window);
        app.world_mut().spawn((Camera::default(),GlobalTransform::default(),fe_renderer::camera::OrbitCameraController::default()));
        let node=app.world_mut().spawn((Transform::from_xyz(1.0,2.0,3.0),GlobalTransform::from_translation(Vec3::new(0.0,0.0,100.0)))).id();
        {
            let mut manager=app.world_mut().resource_mut::<NodeManager>();
            manager.select(node,"snap-node");
            begin_axis_drag(&mut manager);
        }
        let mut buttons=ButtonInput::<MouseButton>::default();buttons.press(MouseButton::Left);buttons.clear();
        app.insert_resource(buttons).add_systems(Update,gimbal_interaction::handle_gimbal_interaction);
        app.update();
        assert_eq!(app.world().get::<Transform>(node).unwrap().translation,Vec3::new(4.0,2.0,3.0));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
        app.update();
        let manager=app.world().resource::<NodeManager>();
        assert!(manager.selected.as_ref().unwrap().drag_committed);
        assert_eq!(manager.pending_transform_before.as_ref().unwrap().translation,Vec3::new(1.0,2.0,3.0));
    }
}
