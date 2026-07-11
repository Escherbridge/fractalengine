//! Crate entry plugin: registers all fe-ui resources/systems and re-exports
//! the public API. See `fe-ui/src/AGENTS.md` §plugin for the module map and
//! §compat for the re-export shims kept for `fractalengine`/`fe-webview`.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use fe_database::RoleLevel;

use crate::{atlas::DashboardState, panels, panels::toolbar::Tool, role_chip};

// ---------------------------------------------------------------------------
// Compat re-exports — symbols reachable at `fe_ui::plugin::*` before the
// module decomposition. `fractalengine` and `fe-webview` import these paths
// directly and must not need edits.
// ---------------------------------------------------------------------------

pub use crate::actions::{UiAction, UiManager};
pub use crate::dialogs::ActiveDialog;
pub use crate::terrain_map::{HexonOp, InstalledTilesetDto, PendingHexonOps, PetalMapState, StorageInfoDto};

// ---------------------------------------------------------------------------
// UI-only resources (form buffers, tool state, role cache).
// ---------------------------------------------------------------------------

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
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            open: true,
            tag_filter_buf: String::new(),
        }
    }
}

/// Currently active editor tool.
#[derive(Resource, Default)]
pub struct ToolState {
    pub active_tool: Tool,
}

/// Which tab is active in the inspector panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    Properties,
    ApiAccess,
    Query,
}

/// Default page size for paginated API token listings.
pub const API_TOKEN_PAGE_SIZE: u32 = 20;

/// A single field definition (property schema entry) for display in the inspector.
#[derive(Debug, Clone)]
pub struct FieldDefEntry {
    pub field_def_id: String,
    pub key: String,
    pub value_type: String,
    pub description: String,
    pub required: bool,
    pub default_val: Option<serde_json::Value>,
}

/// Inspector panel state: form buffers for transform editing & URL fields.
/// Selection state lives in [`crate::node_manager::NodeManager`] — this
/// resource only holds the mutable text buffers that the egui widgets edit.
#[derive(Resource)]
pub struct InspectorFormState {
    pub active_tab: InspectorTab,
    pub external_url: String,
    pub config_url: String,
    pub pos: [String; 3],
    pub rot: [String; 3],
    pub scale: [String; 3],
    // API Access tab state
    pub api_token_scope_buf: String,
    pub api_token_role_buf: String,
    pub api_token_expiry_buf: u32,
    pub generated_api_token: Option<String>,
    pub api_tokens: Vec<crate::dialogs::ApiTokenEntry>,
    pub api_tokens_loading: bool,
    pub api_tokens_page: u32,
    pub api_tokens_total: u64,
    // Query tab state
    pub query_sql_buf: String,
    pub query_result: Option<String>,
    pub query_loading: bool,
    // Property value editing state
    pub node_properties: serde_json::Value,
    pub node_properties_loading: bool,
    pub prop_add_key_buf: String,
    pub prop_add_value_buf: String,
    pub prop_add_type_buf: String,
    // Annotation card state (gis.annotation.* reserved property editor)
    pub annotation_title_buf: String,
    pub annotation_body_buf: String,
    pub annotation_color_buf: String,
    // Field definition (schema) editing state
    pub field_defs: Vec<FieldDefEntry>,
    pub field_defs_loading: bool,
    pub field_def_add_key_buf: String,
    pub field_def_add_type_buf: String,
    pub field_def_add_desc_buf: String,
    pub field_def_add_required: bool,
}

impl Default for InspectorFormState {
    fn default() -> Self {
        Self {
            active_tab: InspectorTab::Properties,
            external_url: String::new(),
            config_url: String::new(),
            pos: ["0.00".into(), "0.00".into(), "0.00".into()],
            rot: ["0.00".into(), "0.00".into(), "0.00".into()],
            scale: ["1.00".into(), "1.00".into(), "1.00".into()],
            api_token_scope_buf: String::new(),
            api_token_role_buf: "viewer".into(),
            api_token_expiry_buf: 720,
            generated_api_token: None,
            api_tokens: Vec::new(),
            api_tokens_loading: false,
            api_tokens_page: 0,
            api_tokens_total: 0,
            query_sql_buf: String::new(),
            query_result: None,
            query_loading: false,
            node_properties: serde_json::Value::Object(Default::default()),
            node_properties_loading: false,
            prop_add_key_buf: String::new(),
            prop_add_value_buf: String::new(),
            prop_add_type_buf: "string".into(),
            annotation_title_buf: String::new(),
            annotation_body_buf: String::new(),
            annotation_color_buf: String::new(),
            field_defs: Vec::new(),
            field_defs_loading: false,
            field_def_add_key_buf: String::new(),
            field_def_add_type_buf: "string".into(),
            field_def_add_desc_buf: String::new(),
            field_def_add_required: false,
        }
    }
}

/// The local user's resolved role at the current scope.
/// Populated by a system that queries RoleManager.
#[derive(Resource, Debug, Default)]
pub struct LocalUserRole {
    pub role: Option<RoleLevel>,
}

impl LocalUserRole {
    /// Check if the local user can manage (assign roles, create entities).
    pub fn can_manage(&self) -> bool {
        self.role.map_or(false, |r| r.can_manage())
    }

    /// Check if the local user can edit content.
    pub fn can_edit(&self) -> bool {
        self.role.map_or(false, |r| r.can_edit())
    }
}

// ---------------------------------------------------------------------------
// Camera focus target (set by sidebar click, consumed by camera system)
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct CameraFocusTarget {
    pub target: Option<[f32; 3]>,
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
}

// ---------------------------------------------------------------------------
// Plugin + system ordering
// ---------------------------------------------------------------------------

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSet {
    /// Drains UI actions, processes portal/URL side effects.
    ProcessActions,
    /// NodeManager selection, gimbal, transform broadcast.
    Selection,
    /// Systems that read finalized state (portal sync, camera focus, cursor).
    PostSelection,
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
        app.init_resource::<InspectorFormState>();
        app.init_resource::<LocalUserRole>();
        app.init_resource::<DashboardState>();
        app.init_resource::<CameraFocusTarget>();
        app.init_resource::<ViewportCursorWorld>();
        app.init_resource::<ViewportRect>();
        app.init_resource::<UiManager>();
        app.init_resource::<PetalMapState>();
        // Guarantee the renderer scale resource exists so fe-ui can drive it
        // (idempotent with CameraControllerPlugin's own init).
        app.init_resource::<fe_renderer::camera::CameraScaleSettings>();
        app.init_resource::<PendingHexonOps>();
        app.init_resource::<crate::asset_ops::PendingAssetOps>();
        app.init_resource::<crate::asset_ops::AssetDownloadStatus>();
        app.init_resource::<crate::gis::GisPanelState>();
        app.init_resource::<fe_sync::TilesetEventBuffer>();
        // Register BrowserCommand so MessageWriter<BrowserCommand> is usable.
        // fe-webview's WebViewPlugin also registers this; calling add_message
        // twice is idempotent.
        app.add_message::<fe_webview::ipc::BrowserCommand>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        app.configure_sets(
            Update,
            (UiSet::ProcessActions, UiSet::Selection, UiSet::PostSelection).chain(),
        );

        app.add_systems(Update, crate::actions::process_ui_actions.in_set(UiSet::ProcessActions));
        app.add_systems(Update, resolve_local_role_on_nav_change.in_set(UiSet::ProcessActions));
        // Surface asset-download outcomes (written by the main binary's bridge) as toasts.
        app.add_systems(Update, crate::asset_ops::surface_asset_download_status);
        app.add_systems(
            Update,
            crate::terrain_map::load_petal_terrain_on_nav_change
                .before(crate::actions::process_ui_actions)
                .in_set(UiSet::ProcessActions),
        );
        app.add_systems(Update, crate::terrain_map::drain_tileset_events.in_set(UiSet::ProcessActions));
        // Mirror the active petal's world scale into the renderer's camera settings.
        app.add_systems(
            Update,
            crate::terrain_map::sync_camera_scale_from_petal_map.in_set(UiSet::PostSelection),
        );

        app.add_systems(
            Update,
            (
                apply_camera_focus,
                strip_gltf_embedded_cameras,
                update_viewport_cursor_world,
            )
                .in_set(UiSet::PostSelection),
        );
    }
}

/// Phase F: bundle of P2P-related params to avoid exceeding Bevy's 16-param limit.
/// Also carries NodeManager so the toolbar deselect button can route through it.
#[derive(bevy::ecs::system::SystemParam)]
struct P2pDialogParams<'w> {
    sync_status: Option<Res<'w, fe_sync::SyncStatus>>,
    node_mgr: ResMut<'w, crate::node_manager::NodeManager>,
    ui_mgr: ResMut<'w, UiManager>,
    portal_rect: ResMut<'w, fe_webview::plugin::PortalPanelRect>,
    peer_registry: Res<'w, fe_runtime::PeerRegistry>,
    // TODO: add node_identity once fe-identity is a dependency of fe-ui
    // node_identity: Res<'w, fe_identity::NodeIdentity>,
}

/// Small bundle for miscellaneous read/write-once resources that don't fit an
/// existing group — keeps `gardener_ui_system`'s own param list under Bevy's
/// 16-param `SystemParam` tuple limit as new cross-cutting UI surfaces land.
#[derive(bevy::ecs::system::SystemParam)]
struct MiscUiParams<'w> {
    asset_status: Res<'w, crate::asset_ops::AssetDownloadStatus>,
    gis_panel: ResMut<'w, crate::gis::GisPanelState>,
}

fn gardener_ui_system(
    mut ctx: EguiContexts,
    mut sidebar: ResMut<SidebarState>,
    mut tool: ResMut<ToolState>,
    mut inspector: ResMut<InspectorFormState>,
    mut nav: ResMut<crate::navigation_manager::NavigationManager>,
    dashboard: Res<DashboardState>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut camera_focus: ResMut<CameraFocusTarget>,
    cursor_world: Res<ViewportCursorWorld>,
    mut p2p: P2pDialogParams,
    mut viewport_rect: ResMut<ViewportRect>,
    local_role: Res<LocalUserRole>,
    mut petal_map: ResMut<PetalMapState>,
    mut misc: MiscUiParams,
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
        &db_sender.0,
        &mut camera_focus,
        &cursor_world,
        p2p.sync_status.as_deref(),
        &mut p2p.node_mgr,
        &mut p2p.ui_mgr,
        &local_role,
        &mut petal_map,
        &misc.asset_status,
        &mut misc.gis_panel,
    );
    viewport_rect.0 = rect;

    // Tell the webview plugin where the right panel is so the popup tracks it.
    // Inset for the portal toolbar header and status bar; see
    // `crate::portal::compute_portal_rect` for the exact math + its tests.
    let screen = ectx.viewport_rect();
    let insets = crate::portal::compute_portal_rect(screen, rect);
    p2p.portal_rect.x = insets.x;
    p2p.portal_rect.y = insets.y;
    p2p.portal_rect.width = insets.width;
    p2p.portal_rect.height = insets.height;

    let role_label = match &local_role.role {
        Some(role) => role.to_string(),
        None => "viewer".to_string(),
    };
    role_chip::role_chip_hud(ectx, &role_label);
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

/// Sends ResolveLocalRole when the navigation scope changes.
fn resolve_local_role_on_nav_change(
    nav: Res<crate::navigation_manager::NavigationManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    mut last_scope: Local<Option<String>>,
) {
    let current_scope = nav.active_verse_id.as_ref().map(|vid| {
        fe_database::build_scope(
            vid,
            nav.active_fractal_id.as_deref(),
            nav.active_petal_id.as_deref(),
        )
    });

    if *last_scope == current_scope {
        return;
    }
    *last_scope = current_scope.clone();

    if let Some(scope) = current_scope {
        db_sender.0.send(fe_runtime::messages::DbCommand::ResolveLocalRole { scope }).ok();
    }
}
