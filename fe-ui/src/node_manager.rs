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

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::gimbal::{GimbalAxis, GIMBAL_LEN, draw_gimbal_for_entity};
use crate::panels::Tool;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{
    InspectorFormState, SpawnedNodeMarker, ToolState, UiSet, ViewportRect,
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
}

/// A currently selected node and its optional in-progress drag session.
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
        self.selected.as_ref().map_or(false, |s| s.drag.is_some())
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
        app.init_resource::<NodeManager>();
        app.add_systems(
            Update,
            (
                handle_tool_shortcuts,
                sync_sidebar_to_manager,
                handle_gimbal_interaction, // axis pick + drag (before viewport click)
                handle_viewport_click,     // entity pick / deselect
                sync_manager_to_inspector,
                draw_gimbal_system,
                broadcast_transform,
            )
                .chain()
                .in_set(UiSet::Selection),
        );
    }
}

// ---------------------------------------------------------------------------
// System: keyboard shortcuts
// ---------------------------------------------------------------------------

fn handle_tool_shortcuts(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ToolState>,
    mut manager: ResMut<NodeManager>,
    mut egui_ctx: EguiContexts,
) {
    let egui_wants_kb = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.wants_keyboard_input())
        .unwrap_or(false);
    if egui_wants_kb {
        return;
    }

    if keyboard.just_pressed(KeyCode::KeyS) {
        tool.active_tool = Tool::Select;
    } else if keyboard.just_pressed(KeyCode::KeyG) {
        tool.active_tool = Tool::Move;
    } else if keyboard.just_pressed(KeyCode::KeyR) {
        tool.active_tool = Tool::Rotate;
    } else if keyboard.just_pressed(KeyCode::KeyX) {
        tool.active_tool = Tool::Scale;
    } else if keyboard.just_pressed(KeyCode::Escape) {
        manager.deselect();
    }
}

// ---------------------------------------------------------------------------
// System: sidebar selection → NodeManager
// ---------------------------------------------------------------------------

fn sync_sidebar_to_manager(
    nav: Res<NavigationManager>,
    mut manager: ResMut<NodeManager>,
    node_query: Query<(Entity, &SpawnedNodeMarker)>,
) {
    let Some(node_id) = manager.pending_sidebar_select.take() else {
        return;
    };

    let active_petal = nav.active_petal_id.as_deref();
    let matched = node_query.iter().find(|(_, m)| {
        m.node_id == node_id
            && active_petal
                .map(|pid| pid == m.petal_id.as_str())
                .unwrap_or(true)
    });

    if let Some((entity, _)) = matched {
        manager.select(entity, node_id);
    } else {
        manager.deselect();
    }
}

// ---------------------------------------------------------------------------
// System: gimbal axis interaction (pick → drag → commit)
// ---------------------------------------------------------------------------

const PICK_PX: f32 = 20.0;

fn handle_gimbal_interaction(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<
        (&Camera, &GlobalTransform),
        With<fe_renderer::camera::OrbitCameraController>,
    >,
    mut manager: ResMut<NodeManager>,
    tool: Res<ToolState>,
    mut transform_query: Query<(&mut Transform, &GlobalTransform)>,
    mut egui_ctx: EguiContexts,
    viewport_rect: Res<ViewportRect>,
) {
    if tool.active_tool == Tool::Select {
        return;
    }
    let Some(sel) = manager.selected.as_mut() else {
        return;
    };
    let entity = sel.entity;

    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);

    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, cam_tx)) = cameras.single() else { return };

    // Release → commit
    if mouse_button.just_released(MouseButton::Left) && sel.drag.is_some() {
        sel.drag = None;
        sel.drag_committed = true;
        return;
    }

    // Apply active drag
    if mouse_button.pressed(MouseButton::Left) {
        if let Some(ref drag) = sel.drag {
            let Some(cursor_pos) = cursor else { return };
            let movement = (cursor_pos - drag.start_cursor).dot(drag.axis_screen_dir);
            let Ok((mut transform, g_tx)) = transform_query.get_mut(entity) else {
                return;
            };
            let scale_factor = (g_tx.translation() - cam_tx.translation()).length().max(0.5) * 0.002;
            let axis_dir = axis_vec(drag.axis);
            match tool.active_tool {
                Tool::Move => {
                    transform.translation = drag.start_pos + axis_dir * movement * scale_factor;
                }
                Tool::Scale => {
                    let f = 1.0 + movement * 0.005;
                    let b = drag.start_scale;
                    transform.scale = Vec3::new(
                        if drag.axis == GimbalAxis::X { b.x * f } else { b.x },
                        if drag.axis == GimbalAxis::Y { b.y * f } else { b.y },
                        if drag.axis == GimbalAxis::Z { b.z * f } else { b.z },
                    );
                }
                Tool::Rotate => {
                    transform.rotation =
                        drag.start_rot * Quat::from_axis_angle(axis_dir, movement * 0.01);
                }
                Tool::Select => {}
            }
            return;
        }
    }

    // Pick axis on press
    let in_viewport = cursor
        .map(|c| viewport_rect.0.contains(bevy_egui::egui::pos2(c.x, c.y)))
        .unwrap_or(false);

    if mouse_button.just_pressed(MouseButton::Left) && !egui_using && in_viewport {
        let Some(cursor_pos) = cursor else { return };
        let Ok((t, g_tx)) = transform_query.get(entity) else { return };
        let center_3d = g_tx.translation();
        let Ok(center_screen) = camera.world_to_viewport(cam_tx, center_3d) else { return };

        for axis in [GimbalAxis::X, GimbalAxis::Y, GimbalAxis::Z] {
            let tip_3d = center_3d + axis_vec(axis) * GIMBAL_LEN;
            let Ok(tip_screen) = camera.world_to_viewport(cam_tx, tip_3d) else { continue };
            let screen_dir = (tip_screen - center_screen).normalize_or_zero();
            let hit = (cursor_pos - tip_screen).length() < PICK_PX
                || segment_dist_2d(cursor_pos, center_screen, tip_screen) < PICK_PX * 0.6;
            if hit {
                sel.drag = Some(AxisDrag {
                    axis,
                    start_cursor: cursor_pos,
                    axis_screen_dir: screen_dir,
                    start_pos: t.translation,
                    start_rot: t.rotation,
                    start_scale: t.scale,
                });
                sel.drag_committed = false;
                break;
            }
        }
    }
}

fn axis_vec(axis: GimbalAxis) -> Vec3 {
    match axis {
        GimbalAxis::X => Vec3::X,
        GimbalAxis::Y => Vec3::Y,
        GimbalAxis::Z => Vec3::Z,
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
            mgr.selected.as_ref().map(|s| s.drag_committed).unwrap_or(false),
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
            !mgr.selected.as_ref().map(|s| s.drag_committed).unwrap_or(true),
            "drag_committed should be false after selecting a new entity"
        );
        assert!(
            mgr.selected.as_ref().and_then(|s| s.drag.as_ref()).is_none(),
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

fn segment_dist_2d(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.dot(ab).max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

// ---------------------------------------------------------------------------
// System: viewport click → select / deselect
// ---------------------------------------------------------------------------

fn handle_viewport_click(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<
        (&Camera, &GlobalTransform),
        With<fe_renderer::camera::OrbitCameraController>,
    >,
    node_query: Query<(Entity, &GlobalTransform, &SpawnedNodeMarker)>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    viewport_rect: Res<ViewportRect>,
    mut egui_ctx: EguiContexts,
) {
    // If a drag was just started this frame (by handle_gimbal_interaction), skip.
    if manager.is_dragging() {
        return;
    }
    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    // Guard against egui-owned pointer (panel resize, button hold, etc.)
    let egui_using = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.is_using_pointer())
        .unwrap_or(false);
    if egui_using {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };

    if !viewport_rect.0.contains(bevy_egui::egui::pos2(cursor.x, cursor.y)) {
        return;
    }

    let Ok((camera, cam_tx)) = cameras.single() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_tx, cursor) else { return };

    let active_petal = nav.active_petal_id.as_deref();
    const PICK_RADIUS: f32 = 1.5;
    let mut best: Option<(Entity, f32, String)> = None;

    for (entity, g_tx, marker) in node_query.iter() {
        if active_petal
            .map(|pid| pid != marker.petal_id.as_str())
            .unwrap_or(false)
        {
            continue;
        }
        let pos = g_tx.translation();
        let along = (pos - ray.origin).dot(*ray.direction);
        if along < 0.0 {
            continue;
        }
        let closest = ray.origin + *ray.direction * along;
        if (pos - closest).length() < PICK_RADIUS {
            if best.is_none() || along < best.as_ref().unwrap().1 {
                best = Some((entity, along, marker.node_id.clone()));
            }
        }
    }

    if let Some((entity, _, node_id)) = best {
        manager.select(entity, node_id.clone());
    } else {
        manager.deselect();
    }
}

// ---------------------------------------------------------------------------
// System: NodeManager → InspectorFormState (display sync)
// ---------------------------------------------------------------------------

fn sync_manager_to_inspector(
    manager: Res<NodeManager>,
    mut inspector: ResMut<InspectorFormState>,
    verse_mgr: Res<crate::verse_manager::VerseManager>,
    // Changed<Transform> avoids 9 format!() allocations per frame while dragging.
    changed_query: Query<&Transform, Changed<Transform>>,
    // Plain query used on initial selection so the inspector populates even when
    // the transform hasn't changed this frame (e.g. freshly selected static node).
    all_query: Query<&Transform>,
    mut last_selected: Local<Option<Entity>>,
) {
    // Early return when nothing is selected.
    let Some(entity) = manager.selected_entity() else {
        *last_selected = None;
        return;
    };

    // On initial selection the transform hasn't Changed<> yet — read it
    // unconditionally so the inspector populates immediately.
    let just_selected = *last_selected != Some(entity);
    *last_selected = Some(entity);

    // Sync per-node URL from VerseManager on selection change so the
    // inspector shows the correct URL for the newly-selected node.
    if just_selected {
        if let Some(ref sel) = manager.selected {
            let url = verse_mgr
                .all_nodes()
                .find(|n| n.id == sel.node_id)
                .and_then(|n| n.webpage_url.clone())
                .unwrap_or_default();
            inspector.external_url = url;
        }
    }

    let t = if just_selected {
        let Ok(t) = all_query.get(entity) else { return };
        t
    } else {
        let Ok(t) = changed_query.get(entity) else { return };
        t
    };
    let (rx, ry, rz) = t.rotation.to_euler(EulerRot::XYZ);
    inspector.pos = [
        format!("{:.2}", t.translation.x),
        format!("{:.2}", t.translation.y),
        format!("{:.2}", t.translation.z),
    ];
    inspector.rot = [
        format!("{:.1}", rx.to_degrees()),
        format!("{:.1}", ry.to_degrees()),
        format!("{:.1}", rz.to_degrees()),
    ];
    inspector.scale = [
        format!("{:.2}", t.scale.x),
        format!("{:.2}", t.scale.y),
        format!("{:.2}", t.scale.z),
    ];
}

// ---------------------------------------------------------------------------
// System: gimbal drawing (delegates to gimbal.rs pure-visual functions)
// ---------------------------------------------------------------------------

fn draw_gimbal_system(
    manager: Res<NodeManager>,
    tool: Res<ToolState>,
    g_transform_query: Query<&GlobalTransform>,
    gizmos: Gizmos,
) {
    let Some(sel) = &manager.selected else { return };
    let dragging = sel.drag.as_ref().map(|d| d.axis);
    draw_gimbal_for_entity(sel.entity, tool.active_tool, dragging, &g_transform_query, gizmos);
}

// ---------------------------------------------------------------------------
// System: broadcast committed transform to DB + P2P sync
// ---------------------------------------------------------------------------

fn broadcast_transform(
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    transform_query: Query<(&Transform, &SpawnedNodeMarker)>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
) {
    let Some(sel) = manager.selected.as_mut() else { return };
    if !sel.drag_committed {
        return;
    }
    sel.drag_committed = false;

    let Ok((transform, marker)) = transform_query.get(sel.entity) else { return };
    let pos = transform.translation;
    let (rx, ry, rz) = transform.rotation.to_euler(EulerRot::XYZ);
    let sc = transform.scale;

    // Keep in-memory NodeEntry in sync so respawn_on_petal_change uses the
    // updated position instead of the stale initial one.
    verse_mgr.update_node_position(&marker.node_id, [pos.x, pos.y, pos.z]);

    if db_sender
        .0
        .send(fe_runtime::messages::DbCommand::UpdateNodeTransform {
            node_id: marker.node_id.clone(),
            position: [pos.x, pos.y, pos.z],
            rotation: [rx, ry, rz],
            scale: [sc.x, sc.y, sc.z],
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — DB thread may have crashed");
    }

    if let Some(sync) = sync_sender {
        if let Some(ref verse_id) = nav.active_verse_id {
            if sync
                .0
                .send(fe_sync::SyncCommand::UpdateNodeTransform {
                    verse_id: verse_id.clone(),
                    node_id: marker.node_id.clone(),
                    position: [pos.x, pos.y, pos.z],
                    rotation: [rx, ry, rz],
                    scale: [sc.x, sc.y, sc.z],
                })
                .is_err()
            {
                bevy::log::warn!("sync_sender channel closed — sync thread may have crashed");
            }
        }
    }
}
