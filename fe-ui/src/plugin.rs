use crate::{atlas::DashboardState, panels, panels::Tool, role_chip};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

/// Marker attached to every `SceneRoot` spawned from a DB node so that
/// the UI can despawn/refresh scene entities when the active petal changes.
#[derive(Component, Debug, Clone)]
pub struct SpawnedNodeMarker {
    pub node_id: String,
    pub petal_id: String,
}

/// Sidebar visibility and search state.
#[derive(Resource)]
pub struct SidebarState {
    pub open: bool,
    pub tag_filter_buf: String,
    pub selected_node_id: Option<String>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            open: true,
            tag_filter_buf: String::new(),
            selected_node_id: None,
        }
    }
}

/// Currently active editor tool.
#[derive(Resource, Default)]
pub struct ToolState {
    pub active_tool: Tool,
}

/// Inspector panel state: selection, transform buffers, URL fields.
#[derive(Resource)]
pub struct InspectorState {
    pub selected_entity: Option<Entity>,
    pub is_admin: bool,
    pub external_url: String,
    pub config_url: String,
    pub pos: [String; 3],
    pub rot: [String; 3],
    pub scale: [String; 3],
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            selected_entity: None,
            is_admin: false,
            external_url: String::new(),
            config_url: String::new(),
            pos: ["0.00".into(), "0.00".into(), "0.00".into()],
            rot: ["0.00".into(), "0.00".into(), "0.00".into()],
            scale: ["1.00".into(), "1.00".into(), "1.00".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// Create dialog state
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct CreateDialogState {
    pub open: bool,
    pub kind: CreateKind,
    pub parent_id: String,
    pub name_buf: String,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum CreateKind {
    #[default]
    Verse,
    Fractal,
    Petal,
    Node,
}

// ---------------------------------------------------------------------------
// Context menu state (viewport right-click)
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct ContextMenuState {
    pub open: bool,
    pub screen_pos: [f32; 2],
    pub world_pos: [f32; 3],
}

// ---------------------------------------------------------------------------
// Camera focus target (set by sidebar click, consumed by camera system)
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct CameraFocusTarget {
    pub target: Option<[f32; 3]>,
}

// ---------------------------------------------------------------------------
// GLTF import dialog state
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct GltfImportState {
    pub open: bool,
    pub file_path_buf: String,
    pub name_buf: String,
    pub position: [f32; 3],
}

// ---------------------------------------------------------------------------
// Viewport cursor world position (camera ray → Y=0 plane intersection)
// ---------------------------------------------------------------------------

/// Tracks the current cursor's world-space position projected onto Y=0.
/// Updated every frame by `update_viewport_cursor_world`.
/// Used by the context menu to place imported GLB models at the correct spot.
#[derive(Resource, Default)]
pub struct ViewportCursorWorld {
    pub pos: Option<[f32; 3]>,
}

/// The egui screen-space rect of the 3-D viewport (CentralPanel).
/// Updated every frame by `gardener_ui_system` and read by the gimbal pick
/// system to reject clicks that land inside sidebar / inspector panels.
#[derive(Resource)]
pub struct ViewportRect(pub bevy_egui::egui::Rect);

impl Default for ViewportRect {
    fn default() -> Self {
        Self(bevy_egui::egui::Rect::EVERYTHING)
    }
}

// ---------------------------------------------------------------------------
// Phase F: Invite/Join dialog state
// ---------------------------------------------------------------------------

/// State for the "Generate Invite" dialog.
#[derive(Resource, Default)]
pub struct InviteDialogState {
    pub open: bool,
    pub invite_string: String,
    /// Whether to include write capability (namespace secret) in the invite.
    pub include_write_cap: bool,
    /// Expiry in hours (default 24).
    pub expiry_hours: u32,
}

/// State for the "Join Verse" dialog.
#[derive(Resource, Default)]
pub struct JoinDialogState {
    pub open: bool,
    pub invite_buf: String,
}

/// Phase F: whether the peer debug panel is visible.
#[derive(Resource, Default)]
pub struct PeerDebugPanelState {
    pub open: bool,
}

/// State for the alt-click node options dialog.
#[derive(Resource, Default)]
pub struct NodeOptionsState {
    pub open: bool,
    pub node_id: String,
    pub node_name_buf: String,
    pub webpage_url_buf: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_manager_default_has_no_active_verse() {
        let state = crate::navigation_manager::NavigationManager::default();
        assert!(state.active_verse_id.is_none());
        assert!(state.active_fractal_id.is_none());
        assert!(state.active_petal_id.is_none());
    }

    #[test]
    fn spawned_node_marker_carries_both_ids() {
        let marker = SpawnedNodeMarker {
            node_id: "node-abc".to_string(),
            petal_id: "petal-xyz".to_string(),
        };
        let cloned = marker.clone();
        assert_eq!(cloned.node_id, "node-abc");
        assert_eq!(cloned.petal_id, "petal-xyz");
        // Debug derive is exercised so the format succeeds.
        let _ = format!("{marker:?}");
    }

    #[test]
    fn viewport_cursor_world_default_is_none() {
        let cursor = ViewportCursorWorld::default();
        assert!(cursor.pos.is_none());
    }

    #[test]
    fn context_menu_captures_world_pos() {
        // Simulate the context menu capturing a non-zero world position.
        let cursor_world = ViewportCursorWorld {
            pos: Some([5.0, 0.0, -3.0]),
        };
        let mut ctx_menu = ContextMenuState::default();
        // Mimics the right-click handler in viewport_overlay.
        ctx_menu.world_pos = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        assert_eq!(ctx_menu.world_pos, [5.0, 0.0, -3.0]);
    }

    #[test]
    fn context_menu_falls_back_to_origin_when_cursor_none() {
        let cursor_world = ViewportCursorWorld { pos: None };
        let mut ctx_menu = ContextMenuState::default();
        ctx_menu.world_pos = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        assert_eq!(ctx_menu.world_pos, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn gltf_import_state_carries_position_through() {
        let mut gltf = GltfImportState::default();
        // Simulates the context menu setting the position before opening the dialog.
        let world = [7.5, 0.0, -2.1];
        gltf.position = world;
        gltf.open = true;
        assert_eq!(gltf.position, [7.5, 0.0, -2.1]);
    }

    #[test]
    fn gltf_import_default_position_is_origin() {
        let gltf = GltfImportState::default();
        assert_eq!(gltf.position, [0.0, 0.0, 0.0]);
    }
}

pub struct GardenerConsolePlugin;

impl Plugin for GardenerConsolePlugin {
    fn build(&self, app: &mut App) {
        // Domain managers — each owns its state and systems.
        app.add_plugins(crate::navigation_manager::NavigationManagerPlugin);
        app.add_plugins(crate::verse_manager::VerseManagerPlugin);
        app.add_plugins(crate::node_manager::NodeManagerPlugin);
        // UI-only resources (form buffers, dialog flags, etc.)
        app.init_resource::<SidebarState>();
        app.init_resource::<ToolState>();
        app.init_resource::<InspectorState>();
        app.init_resource::<DashboardState>();
        app.init_resource::<CreateDialogState>();
        app.init_resource::<ContextMenuState>();
        app.init_resource::<GltfImportState>();
        app.init_resource::<CameraFocusTarget>();
        app.init_resource::<ViewportCursorWorld>();
        app.init_resource::<ViewportRect>();
        app.init_resource::<InviteDialogState>();
        app.init_resource::<JoinDialogState>();
        app.init_resource::<PeerDebugPanelState>();
        app.init_resource::<NodeOptionsState>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        app.add_systems(
            Update,
            (
                apply_camera_focus,
                strip_gltf_embedded_cameras,
                update_viewport_cursor_world,
            ),
        );
    }
}

/// Phase F: bundle of P2P dialog state to avoid exceeding Bevy's 16-param limit.
/// Also carries NodeManager so the toolbar deselect button can route through it.
#[derive(bevy::ecs::system::SystemParam)]
struct P2pDialogParams<'w> {
    invite_dialog: ResMut<'w, InviteDialogState>,
    join_dialog: ResMut<'w, JoinDialogState>,
    sync_status: Option<Res<'w, fe_sync::SyncStatus>>,
    peer_debug: ResMut<'w, PeerDebugPanelState>,
    node_mgr: ResMut<'w, crate::node_manager::NodeManager>,
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut sidebar: ResMut<SidebarState>,
    mut tool: ResMut<ToolState>,
    mut inspector: ResMut<InspectorState>,
    mut nav: ResMut<crate::navigation_manager::NavigationManager>,
    dashboard: Res<DashboardState>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    mut create_dialog: ResMut<CreateDialogState>,
    mut context_menu: ResMut<ContextMenuState>,
    mut gltf_import: ResMut<GltfImportState>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut camera_focus: ResMut<CameraFocusTarget>,
    cursor_world: Res<ViewportCursorWorld>,
    mut node_options: ResMut<NodeOptionsState>,
    mut p2p: P2pDialogParams,
    mut viewport_rect: ResMut<ViewportRect>,
) {
    let Ok(ectx) = ctx.ctx_mut() else { return };

    let rect = panels::gardener_console(
        ectx,
        &mut sidebar,
        &mut tool,
        &mut inspector,
        &mut nav,
        &dashboard,
        &mut verse_mgr,
        &mut create_dialog,
        &mut context_menu,
        &mut gltf_import,
        &db_sender.0,
        &mut camera_focus,
        &cursor_world,
        &mut node_options,
        &mut p2p.invite_dialog,
        &mut p2p.join_dialog,
        p2p.sync_status.as_deref(),
        &mut p2p.peer_debug,
        &mut p2p.node_mgr,
    );
    viewport_rect.0 = rect;
    let role_label = if inspector.is_admin {
        "admin"
    } else {
        "member"
    };
    role_chip::role_chip_hud(ectx, role_label);
}

/// Bevy's GLTF loader can produce embedded `Camera3d`/`Camera` entities when a
/// `.glb` scene contains a camera node. These become secondary active cameras
/// that render into the same window and cause the "duplicate / ghost image"
/// visual artefact. This system removes any non-orbit camera added during the
/// frame so only the viewport's `OrbitCameraController` remains.
fn strip_gltf_embedded_cameras(
    added: Query<
        Entity,
        (
            Added<Camera>,
            Without<fe_renderer::camera::OrbitCameraController>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in added.iter() {
        bevy::log::debug!(
            "Despawning GLB-embedded camera entity={:?} (not the orbit camera)",
            entity
        );
        commands.entity(entity).despawn();
    }
}

fn apply_camera_focus(
    mut focus_target: ResMut<CameraFocusTarget>,
    mut query: Query<&mut fe_renderer::camera::OrbitCameraController>,
) {
    if let Some(pos) = focus_target.target.take() {
        if let Ok(mut controller) = query.single_mut() {
            controller.focus = Vec3::new(pos[0], pos[1], pos[2]);
            controller.distance = 5.0;
        }
    }
}

/// Projects the cursor position onto the Y=0 world plane each frame.
fn update_viewport_cursor_world(
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    mut cursor_world: ResMut<ViewportCursorWorld>,
    mut egui_ctx: EguiContexts,
) {
    // Only suppress the world cursor when egui is actively consuming pointer
    // input (e.g. dragging a slider, clicking a button).  The old check used
    // `is_pointer_over_area()` which returns true for the transparent
    // CentralPanel (the 3-D viewport), so the world position was *never*
    // computed and every placed model landed at 0,0,0.
    let Ok(ectx) = egui_ctx.ctx_mut() else {
        cursor_world.pos = None;
        return;
    };
    if ectx.is_using_pointer() {
        cursor_world.pos = None;
        return;
    }
    let Ok(window) = windows.single() else {
        cursor_world.pos = None;
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        cursor_world.pos = None;
        return;
    };
    let Ok((camera, cam_tx)) = cameras.single() else {
        cursor_world.pos = None;
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tx, cursor) else {
        cursor_world.pos = None;
        return;
    };
    // Intersect the ray with the infinite Y=0 plane.
    let ground_origin = Vec3::ZERO;
    let ground_normal = Dir3::Y;
    if let Some(point) =
        ray.plane_intersection_point(ground_origin, InfinitePlane3d::new(ground_normal))
    {
        cursor_world.pos = Some([point.x, 0.0, point.z]);
    } else {
        cursor_world.pos = None;
    }
}
