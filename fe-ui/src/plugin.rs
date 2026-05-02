use crate::{atlas::DashboardState, panels, panels::Tool, role_chip};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use fe_database::RoleLevel;
use fe_webview::ipc::BrowserCommand;

/// Actions queued by the egui render pass, drained by a single Update system.
/// Replaces scattered one-frame signal fields.
#[derive(Debug, Clone)]
pub enum UiAction {
    /// Open the portal webview for the given URL (replaces WebViewOpenRequest.url).
    OpenPortal { url: String },
    /// Close the portal webview (replaces PortalPanelState.close).
    ClosePortal,
    /// Navigate back in portal history (replaces PortalPanelState.go_back).
    PortalGoBack,
    /// Save URL for the selected node (replaces InspectorFormState.url_save_pending).
    SaveUrl,
}

/// Portal webview lifecycle state (replaces PortalPanelState).
#[derive(Debug, Clone, Default)]
pub enum PortalState {
    #[default]
    Closed,
    Open {
        current_url: String,
        cached_hostname: String,
        opened_for_entity: Entity,
    },
}

/// Which floating dialog is currently open. At most one at a time.
/// Replaces 7 separate dialog-state resources.
#[derive(Debug, Clone, Default)]
pub enum ActiveDialog {
    #[default]
    None,
    CreateEntity {
        kind: CreateKind,
        parent_id: String,
        name_buf: String,
    },
    ContextMenu {
        screen_pos: [f32; 2],
        world_pos: [f32; 3],
    },
    GltfImport {
        file_path_buf: String,
        name_buf: String,
        position: [f32; 3],
    },
    NodeOptions {
        node_id: String,
        node_name_buf: String,
        webpage_url_buf: String,
    },
    InviteDialog {
        invite_string: String,
        include_write_cap: bool,
        expiry_hours: u32,
    },
    JoinDialog {
        invite_buf: String,
    },
    PeerDebug,
    EntitySettings {
        entity_type: EntitySettingsType,
        entity_id: String,
        entity_name: String,
        /// Parent verse ID (always set; needed for correct scope strings).
        parent_verse_id: String,
        /// Parent fractal ID (set when entity is a Petal).
        parent_fractal_id: Option<String>,
        active_tab: SettingsTab,
        // General tab state
        name_buf: String,
        default_access_buf: Option<String>,
        description_buf: Option<String>,
        // Access tab state
        peer_roles: Vec<PeerRoleEntry>,
        roles_loading: bool,
        invite_role_buf: String,
        invite_expiry_buf: u32,
        generated_invite_link: Option<String>,
        // Confirmation state
        pending_delete: bool,
        // API Access tab state
        api_tokens: Vec<ApiTokenEntry>,
        api_tokens_loading: bool,
        api_token_scope_buf: String,
        api_token_role_buf: String,
        api_token_expiry_buf: u32,
        generated_api_token: Option<String>,
        /// Tokens scoped to this entity's scope tree (admin view).
        scoped_api_tokens: Vec<ApiTokenEntry>,
        scoped_tokens_loading: bool,
    },
}

/// Centralized UI state resource.
#[derive(Resource)]
pub struct UiManager {
    /// Actions queued during egui rendering, drained each frame in Update.
    actions: Vec<UiAction>,
    /// Portal webview lifecycle.
    pub portal: PortalState,
    /// Sidebar open state — derived from portal/inspector each frame.
    pub sidebar_open: bool,
    /// Which floating dialog is currently open (at most one).
    pub active_dialog: ActiveDialog,
    /// Toast message with spawn time (seconds since startup).
    toast: Option<(String, f64)>,
}

/// Duration in seconds for toast visibility.
const TOAST_DURATION: f64 = 2.0;

impl Default for UiManager {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            portal: PortalState::Closed,
            sidebar_open: true,
            active_dialog: ActiveDialog::None,
            toast: None,
        }
    }
}

impl UiManager {
    pub fn push_action(&mut self, action: UiAction) {
        self.actions.push(action);
    }

    pub fn drain_actions(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn portal_is_open(&self) -> bool {
        matches!(self.portal, PortalState::Open { .. })
    }

    pub fn portal_url(&self) -> &str {
        match &self.portal {
            PortalState::Open { current_url, .. } => current_url,
            PortalState::Closed => "",
        }
    }

    pub fn portal_hostname(&self) -> &str {
        match &self.portal {
            PortalState::Open { cached_hostname, .. } => cached_hostname,
            PortalState::Closed => "",
        }
    }

    pub fn any_dialog_open(&self) -> bool {
        !matches!(self.active_dialog, ActiveDialog::None)
    }

    pub fn open_dialog(&mut self, dialog: ActiveDialog) {
        self.active_dialog = dialog;
    }

    pub fn close_dialog(&mut self) {
        self.active_dialog = ActiveDialog::None;
    }

    /// Show a brief toast message (fades after TOAST_DURATION seconds).
    pub fn show_toast(&mut self, msg: impl Into<String>, now_secs: f64) {
        self.toast = Some((msg.into(), now_secs));
    }

    /// Returns (message, alpha 0.0..1.0) if a toast is currently visible.
    pub fn active_toast(&self, now_secs: f64) -> Option<(&str, f32)> {
        let (ref msg, spawn_t) = self.toast.as_ref()?;
        let elapsed = now_secs - spawn_t;
        if elapsed > TOAST_DURATION {
            return None;
        }
        // Fade in over first 0.2s, fade out over last 0.5s
        let alpha = if elapsed < 0.2 {
            (elapsed / 0.2) as f32
        } else if elapsed > TOAST_DURATION - 0.5 {
            ((TOAST_DURATION - elapsed) / 0.5) as f32
        } else {
            1.0
        };
        Some((msg, alpha.clamp(0.0, 1.0)))
    }
}

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
}

/// Default page size for paginated API token listings.
pub const API_TOKEN_PAGE_SIZE: u32 = 20;

/// Inspector panel state: form buffers for transform editing & URL fields.
/// Selection state lives in [`NodeManager`] — this resource only holds
/// the mutable text buffers that the egui widgets edit.
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
    pub api_tokens: Vec<ApiTokenEntry>,
    pub api_tokens_loading: bool,
    pub api_tokens_page: u32,
    pub api_tokens_total: u64,
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

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum CreateKind {
    #[default]
    Verse,
    Fractal,
    Petal,
    Node,
}

/// Which entity type the settings dialog is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitySettingsType {
    Verse,
    Fractal,
    Petal,
}

/// Active tab in the EntitySettings dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Access,
    ApiAccess,
}

/// A peer's resolved role at a specific scope, for display in the Access tab.
#[derive(Debug, Clone)]
pub struct PeerRoleEntry {
    pub peer_did: String,
    pub display_name: String,
    pub role: String,
    pub is_online: bool,
}

/// An API token record for display in the API Access tab.
#[derive(Debug, Clone)]
pub struct ApiTokenEntry {
    pub jti: String,
    pub scope: String,
    pub max_role: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub revoked: bool,
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

    #[test]
    fn active_dialog_context_menu_captures_world_pos() {
        // Simulate the context menu capturing a non-zero world position via ActiveDialog.
        let cursor_world = ViewportCursorWorld {
            pos: Some([5.0, 0.0, -3.0]),
        };
        let world = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        let mut mgr = UiManager::default();
        mgr.open_dialog(ActiveDialog::ContextMenu {
            screen_pos: [100.0, 200.0],
            world_pos: world,
        });
        if let ActiveDialog::ContextMenu { world_pos, .. } = &mgr.active_dialog {
            assert_eq!(*world_pos, [5.0, 0.0, -3.0]);
        } else {
            panic!("expected ContextMenu");
        }
    }

    #[test]
    fn active_dialog_context_menu_falls_back_to_origin() {
        let cursor_world = ViewportCursorWorld { pos: None };
        let world = cursor_world.pos.unwrap_or([0.0, 0.0, 0.0]);
        let mut mgr = UiManager::default();
        mgr.open_dialog(ActiveDialog::ContextMenu {
            screen_pos: [0.0, 0.0],
            world_pos: world,
        });
        if let ActiveDialog::ContextMenu { world_pos, .. } = &mgr.active_dialog {
            assert_eq!(*world_pos, [0.0, 0.0, 0.0]);
        } else {
            panic!("expected ContextMenu");
        }
    }

    #[test]
    fn active_dialog_gltf_import_carries_position() {
        let mut mgr = UiManager::default();
        let world = [7.5, 0.0, -2.1];
        mgr.open_dialog(ActiveDialog::GltfImport {
            file_path_buf: String::new(),
            name_buf: String::new(),
            position: world,
        });
        if let ActiveDialog::GltfImport { position, .. } = &mgr.active_dialog {
            assert_eq!(*position, [7.5, 0.0, -2.1]);
        } else {
            panic!("expected GltfImport");
        }
    }

    #[test]
    fn active_dialog_default_is_none() {
        let mgr = UiManager::default();
        assert!(matches!(mgr.active_dialog, ActiveDialog::None));
        assert!(!mgr.any_dialog_open());
    }

    #[test]
    fn active_dialog_close_resets_to_none() {
        let mut mgr = UiManager::default();
        mgr.open_dialog(ActiveDialog::PeerDebug);
        assert!(mgr.any_dialog_open());
        mgr.close_dialog();
        assert!(!mgr.any_dialog_open());
        assert!(matches!(mgr.active_dialog, ActiveDialog::None));
    }

    #[test]
    fn active_dialog_mutual_exclusion() {
        let mut mgr = UiManager::default();
        mgr.open_dialog(ActiveDialog::PeerDebug);
        assert!(matches!(mgr.active_dialog, ActiveDialog::PeerDebug));
        // Opening a different dialog replaces the previous one.
        mgr.open_dialog(ActiveDialog::JoinDialog {
            invite_buf: String::new(),
        });
        assert!(matches!(mgr.active_dialog, ActiveDialog::JoinDialog { .. }));
    }

    #[test]
    fn active_dialog_entity_settings_opens_and_closes() {
        let mut mgr = UiManager::default();
        mgr.open_dialog(ActiveDialog::EntitySettings {
            entity_type: EntitySettingsType::Verse,
            entity_id: "v1".to_string(),
            entity_name: "Test Verse".to_string(),
            parent_verse_id: "v1".to_string(),
            parent_fractal_id: None,
            active_tab: SettingsTab::General,
            name_buf: "Test Verse".to_string(),
            default_access_buf: Some("viewer".to_string()),
            description_buf: None,
            peer_roles: vec![],
            roles_loading: false,
            invite_role_buf: "viewer".to_string(),
            invite_expiry_buf: 24,
            generated_invite_link: None,
            pending_delete: false,
            api_tokens: vec![],
            api_tokens_loading: false,
            api_token_scope_buf: String::new(),
            api_token_role_buf: "viewer".to_string(),
            api_token_expiry_buf: 24,
            generated_api_token: None,
            scoped_api_tokens: vec![],
            scoped_tokens_loading: false,
        });
        assert!(mgr.any_dialog_open());
        assert!(matches!(mgr.active_dialog, ActiveDialog::EntitySettings { .. }));
        mgr.close_dialog();
        assert!(!mgr.any_dialog_open());
    }
}

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
        // Register BrowserCommand so MessageWriter<BrowserCommand> is usable.
        // fe-webview's WebViewPlugin also registers this; calling add_message
        // twice is idempotent.
        app.add_message::<BrowserCommand>();
        app.add_systems(EguiPrimaryContextPass, gardener_ui_system);
        app.configure_sets(
            Update,
            (UiSet::ProcessActions, UiSet::Selection, UiSet::PostSelection).chain(),
        );

        app.add_systems(Update, process_ui_actions.in_set(UiSet::ProcessActions));
        app.add_systems(Update, resolve_local_role_on_nav_change.in_set(UiSet::ProcessActions));

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
    );
    viewport_rect.0 = rect;

    // Tell the webview plugin where the right panel is so the popup tracks it.
    // Inset for the portal toolbar header (~36px) and status bar (~22px).
    // Left padding leaves room for the panel resize handle so it isn't blocked
    // by the webview overlay.
    let screen = ectx.viewport_rect();
    let toolbar_header = 36.0_f32;
    let status_bar = 22.0_f32;
    let left_pad = 6.0_f32;
    p2p.portal_rect.x = rect.right() + left_pad;
    p2p.portal_rect.y = rect.top() + toolbar_header;
    p2p.portal_rect.width = (screen.right() - rect.right() - left_pad).max(1.0);
    p2p.portal_rect.height = (rect.height() - toolbar_header - status_bar).max(1.0);

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

/// Drains all UiActions queued during the egui pass and processes them.
/// Replaces: forward_webview_open_request, drain_portal_panel_actions, handle_url_save.
fn process_ui_actions(
    mut ui_mgr: ResMut<UiManager>,
    inspector: Res<InspectorFormState>,
    node_mgr: Res<crate::node_manager::NodeManager>,
    mut browser_commands: MessageWriter<BrowserCommand>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
) {
    // Auto-close portal when the selected entity changes or is deselected.
    if let PortalState::Open { opened_for_entity, .. } = ui_mgr.portal {
        let selected = node_mgr.selected_entity();
        let entity_changed = selected != Some(opened_for_entity);
        if selected.is_none() || entity_changed {
            ui_mgr.portal = PortalState::Closed;
            browser_commands.write(BrowserCommand::Close);
        }
    }

    let actions = ui_mgr.drain_actions();
    for action in actions {
        match action {
            UiAction::OpenPortal { url } => {
                match url.parse::<url::Url>() {
                    Ok(parsed) => {
                        if let Some(entity) = node_mgr.selected_entity() {
                            bevy::log::info!("Portal: forwarding Navigate for URL: {parsed}");
                            let cached_hostname = parsed
                                .host_str()
                                .unwrap_or("")
                                .to_string();
                            ui_mgr.portal = PortalState::Open {
                                current_url: parsed.to_string(),
                                cached_hostname,
                                opened_for_entity: entity,
                            };
                            browser_commands.write(BrowserCommand::Navigate { url: parsed });
                        }
                    }
                    Err(e) => {
                        bevy::log::warn!("UiAction::OpenPortal invalid URL: {e}");
                    }
                }
            }
            UiAction::ClosePortal => {
                ui_mgr.portal = PortalState::Closed;
                browser_commands.write(BrowserCommand::Close);
            }
            UiAction::PortalGoBack => {
                browser_commands.write(BrowserCommand::GoBack);
            }
            UiAction::SaveUrl => {
                let node_id = node_mgr.selected.as_ref().map(|s| &s.node_id);
                let Some(node_id) = node_id else {
                    continue;
                };
                let url = if inspector.external_url.trim().is_empty() {
                    None
                } else {
                    Some(inspector.external_url.clone())
                };

                verse_mgr.update_node_url(node_id, url.clone());

                if db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::UpdateNodeUrl {
                        node_id: node_id.clone(),
                        url,
                    })
                    .is_err()
                {
                    bevy::log::warn!("db_sender channel closed — UpdateNodeUrl not persisted");
                }
            }
        }
    }
}
