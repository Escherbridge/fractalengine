use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Submit a SurrealQL query via the API gateway.
    SubmitQuery { sql: String, scope: String },
    /// Request loading properties for selected node.
    LoadNodeProperties { node_id: String },
    /// Set a property value on a node.
    SetNodeProperty { node_id: String, key: String, value: serde_json::Value },
    /// Delete a property from a node.
    DeleteNodeProperty { node_id: String, key: String },
    // Hexon Manager actions
    HexonInstallFromFile(PathBuf),
    HexonRemoveTileset(String),
    HexonToggleSeeding(String, bool),
    HexonStartDownload(String),
    HexonCancelDownload(String),
    HexonRefreshList,
    HexonOpenStorageDir,
    // Petal Manifest actions
    PetalManifestSave { petal_id: String, manifest: PetalManifest },
    PetalManifestOpen { petal_id: String, petal_name: String },
}

// ---------------------------------------------------------------------------
// Hexon Manager types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexonManagerTab {
    Installed,
    Available,
    Downloads,
}

#[derive(Debug, Clone)]
pub struct InstalledTilesetDto {
    pub hexon_id: String,
    pub region_name: String,
    pub bounds: [f64; 4],
    pub zoom_range: (u8, u8),
    pub tile_count: u32,
    pub size_bytes: u64,
    pub seeding_enabled: bool,
    pub installed_at: String,
}

#[derive(Debug, Clone)]
pub struct AvailableTilesetDto {
    pub hexon_id: String,
    pub region_name: String,
    pub bounds: [f64; 4],
    pub zoom_range: (u8, u8),
    pub tile_count: u32,
    pub approx_size_bytes: u64,
    pub peer_count: u32,
    pub already_installed: bool,
}

#[derive(Debug, Clone)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Verifying,
    Complete,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub tileset_id: String,
    pub chunks_received: u32,
    pub total_chunks: u32,
    pub bytes_received: u64,
    pub total_bytes_estimate: u64,
    pub status: DownloadStatus,
}

#[derive(Debug, Clone)]
pub struct StorageInfoDto {
    pub base_dir: String,
    pub total_bytes: u64,
    pub count: u32,
}

// ---------------------------------------------------------------------------
// Petal Manifest types
// ---------------------------------------------------------------------------

/// A single hexon requirement in a petal's manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestHexonEntry {
    pub hexon_id: String,
    pub hexon_type: String,
    pub required: bool,
}

/// Parsed petal manifest — mirrors the JSON stored in `petal.hexon_manifest`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PetalManifest {
    #[serde(default)]
    pub hexons: Vec<ManifestHexonEntry>,
    #[serde(default = "default_render_distance")]
    pub render_distance: f32,
    #[serde(default = "default_fallback")]
    pub fallback: String,
}

fn default_render_distance() -> f32 { 500.0 }
fn default_fallback() -> String { "sign".to_string() }

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
    HexonManager {
        installed_tilesets: Vec<InstalledTilesetDto>,
        available_tilesets: Vec<AvailableTilesetDto>,
        download_progress: HashMap<String, DownloadProgress>,
        filter_text: String,
        active_tab: HexonManagerTab,
        storage_info: StorageInfoDto,
        loading: bool,
        pending_remove: Option<String>,
    },
    PetalManifest {
        petal_id: String,
        petal_name: String,
        manifest: PetalManifest,
        /// Hexon IDs available locally (from the global hexon store).
        available_hexon_ids: Vec<String>,
        add_hexon_id_buf: String,
        add_hexon_type_buf: String,
        render_distance_buf: String,
        dirty: bool,
    },
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
    /// DID of the node that minted this token.
    pub sub: String,
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
        app.init_resource::<fe_sync::TilesetEventBuffer>();
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
        app.add_systems(Update, drain_tileset_events.in_set(UiSet::ProcessActions));

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
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
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
            UiAction::SubmitQuery { sql, scope: _ } => {
                if db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::RawQuery {
                        sql,
                        vars: std::collections::HashMap::new(),
                    })
                    .is_err()
                {
                    bevy::log::warn!("db_sender channel closed — RawQuery not dispatched");
                }
            }
            UiAction::LoadNodeProperties { node_id } => {
                if db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::GetNodeProperties { node_id })
                    .is_err()
                {
                    bevy::log::warn!("db_sender channel closed — GetNodeProperties not dispatched");
                }
            }
            UiAction::SetNodeProperty { node_id, key, value } => {
                if db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::SetNodeProperty { node_id, key, value })
                    .is_err()
                {
                    bevy::log::warn!("db_sender channel closed — SetNodeProperty not dispatched");
                }
            }
            UiAction::DeleteNodeProperty { node_id, key } => {
                if db_sender
                    .0
                    .send(fe_runtime::messages::DbCommand::DeleteNodeProperty { node_id, key })
                    .is_err()
                {
                    bevy::log::warn!("db_sender channel closed — DeleteNodeProperty not dispatched");
                }
            }
            // Hexon Manager actions — wire to sync thread for P2P distribution.
            UiAction::HexonInstallFromFile(path) => {
                bevy::log::info!("Hexon: install from file {:?}", path);
                // Read file and install via db command (async install via API
                // gateway is a future improvement; for now log the path).
                // TODO: POST /api/v1/hexons/tilesets/install with file bytes
            }
            UiAction::HexonRemoveTileset(id) => {
                bevy::log::info!("Hexon: remove tileset {}", id);
                // TODO: DELETE /api/v1/hexons/tilesets/{id}
            }
            UiAction::HexonToggleSeeding(id, enabled) => {
                bevy::log::info!("Hexon: toggle seeding {}={}", id, enabled);
                // TODO: PATCH /api/v1/hexons/tilesets/{id}/seeding
                // After toggling, re-advertise to peers
                if let Some(ref sender) = sync_sender {
                    sender.0.send(fe_sync::SyncCommand::AdvertiseTilesets {
                        advertisements_json: String::new(), // refreshed by terrain layer
                    }).ok();
                }
            }
            UiAction::HexonStartDownload(id) => {
                bevy::log::info!("Hexon: start P2P download for tileset {}", id);
                // Initialize a download tracker in the dialog state
                if let ActiveDialog::HexonManager {
                    ref mut download_progress, ..
                } = ui_mgr.active_dialog
                {
                    download_progress.insert(id.clone(), DownloadProgress {
                        tileset_id: id.clone(),
                        chunks_received: 0,
                        total_chunks: 0,
                        bytes_received: 0,
                        total_bytes_estimate: 0,
                        status: DownloadStatus::Queued,
                    });
                }
                // Request metadata from any peer that has it
                if let Some(ref sender) = sync_sender {
                    sender.0.send(fe_sync::SyncCommand::RequestTilesetMeta {
                        peer_id: String::new(), // sync thread picks best peer
                        tileset_id: id,
                    }).ok();
                }
            }
            UiAction::HexonCancelDownload(id) => {
                bevy::log::info!("Hexon: cancel download {}", id);
                if let Some(ref sender) = sync_sender {
                    sender.0.send(fe_sync::SyncCommand::CancelTilesetDownload {
                        tileset_id: id.clone(),
                    }).ok();
                }
                // Update status in dialog
                if let ActiveDialog::HexonManager {
                    ref mut download_progress, ..
                } = ui_mgr.active_dialog
                {
                    download_progress.remove(&id);
                }
            }
            UiAction::HexonRefreshList => {
                bevy::log::info!("Hexon: refresh tileset list");
                // Re-advertise our tilesets to trigger peer exchange
                if let Some(ref sender) = sync_sender {
                    sender.0.send(fe_sync::SyncCommand::AdvertiseTilesets {
                        advertisements_json: String::new(),
                    }).ok();
                }
            }
            UiAction::HexonOpenStorageDir => {
                if let ActiveDialog::HexonManager { ref storage_info, .. } = ui_mgr.active_dialog {
                    let dir = &storage_info.base_dir;
                    if !dir.is_empty() {
                        #[cfg(target_os = "windows")]
                        { let _ = std::process::Command::new("explorer").arg(dir).spawn(); }
                        #[cfg(target_os = "macos")]
                        { let _ = std::process::Command::new("open").arg(dir).spawn(); }
                        #[cfg(target_os = "linux")]
                        { let _ = std::process::Command::new("xdg-open").arg(dir).spawn(); }
                    }
                }
            }
            UiAction::PetalManifestSave { petal_id, manifest } => {
                bevy::log::info!("PetalManifest: save requested for petal {petal_id} ({} hexons)", manifest.hexons.len());
                // TODO: PATCH /api/v1/petals/{petal_id}/manifest
            }
            UiAction::PetalManifestOpen { petal_id, petal_name } => {
                bevy::log::info!("PetalManifest: open dialog for petal {petal_id} ({petal_name})");
                ui_mgr.active_dialog = ActiveDialog::PetalManifest {
                    petal_id,
                    petal_name,
                    manifest: PetalManifest::default(),
                    available_hexon_ids: Vec::new(),
                    add_hexon_id_buf: String::new(),
                    add_hexon_type_buf: String::new(),
                    render_distance_buf: "500".to_string(),
                    dirty: false,
                };
            }
        }
    }
}

/// Drains tileset distribution events from the sync thread and updates
/// the Hexon Manager dialog's available/download state.
fn drain_tileset_events(
    mut ui_mgr: ResMut<UiManager>,
    mut tileset_buf: ResMut<fe_sync::TilesetEventBuffer>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    if tileset_buf.events.is_empty() {
        return;
    }

    let events: Vec<fe_sync::SyncEvent> = tileset_buf.events.drain(..).collect();

    for evt in events {
        match evt {
            fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id,
                advertisements_json,
            } => {
                // Parse advertisements and merge into available tilesets
                let Ok(ads): Result<Vec<serde_json::Value>, _> =
                    serde_json::from_str(&advertisements_json)
                else {
                    bevy::log::warn!("Failed to parse peer tileset advertisements");
                    continue;
                };

                if let ActiveDialog::HexonManager {
                    ref mut available_tilesets,
                    ref installed_tilesets,
                    ..
                } = ui_mgr.active_dialog
                {
                    for ad in ads {
                        let Some(tileset_id) = ad.get("tileset_id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        // Skip if we already have it in available list
                        if available_tilesets.iter().any(|t| t.hexon_id == tileset_id) {
                            // Increment peer count
                            if let Some(existing) = available_tilesets.iter_mut().find(|t| t.hexon_id == tileset_id) {
                                existing.peer_count += 1;
                            }
                            continue;
                        }
                        let already_installed = installed_tilesets.iter().any(|t| t.hexon_id == tileset_id);
                        available_tilesets.push(AvailableTilesetDto {
                            hexon_id: tileset_id.to_string(),
                            region_name: ad.get("region_name").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string(),
                            bounds: ad.get("bounds").and_then(|v| {
                                serde_json::from_value::<[f64; 4]>(v.clone()).ok()
                            }).unwrap_or([0.0; 4]),
                            zoom_range: (
                                ad.get("min_zoom").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                                ad.get("max_zoom").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                            ),
                            tile_count: ad.get("tile_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            approx_size_bytes: ad.get("approx_size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                            peer_count: 1,
                            already_installed,
                        });
                    }
                }
                bevy::log::info!(
                    "Received tileset advertisements from peer {}",
                    peer_id,
                );
            }
            fe_sync::SyncEvent::TilesetMetaReceived {
                tileset_id,
                total_chunks,
                approx_size_bytes,
                ..
            } => {
                // Update the download tracker with chunk count and start requesting chunks
                if let ActiveDialog::HexonManager {
                    ref mut download_progress, ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.total_chunks = total_chunks;
                        dl.total_bytes_estimate = approx_size_bytes;
                        dl.status = DownloadStatus::Downloading;
                    }
                }
                // Request the first chunk
                if let Some(ref sender) = sync_sender {
                    sender.0.send(fe_sync::SyncCommand::RequestChunk {
                        peer_id: String::new(),
                        tileset_id,
                        chunk_seq: 0,
                    }).ok();
                }
            }
            fe_sync::SyncEvent::ChunkReceived {
                tileset_id,
                chunk_seq,
                chunk_bytes,
            } => {
                let chunk_size = chunk_bytes.len() as u64;
                let mut request_next = None;

                if let ActiveDialog::HexonManager {
                    ref mut download_progress, ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.chunks_received += 1;
                        dl.bytes_received += chunk_size;

                        if dl.chunks_received >= dl.total_chunks {
                            dl.status = DownloadStatus::Verifying;
                        } else {
                            // Request next missing chunk
                            request_next = Some((tileset_id.clone(), dl.chunks_received));
                        }
                    }
                }

                // Request next chunk if needed
                if let (Some((ts_id, next_seq)), Some(ref sender)) = (request_next, &sync_sender) {
                    sender.0.send(fe_sync::SyncCommand::RequestChunk {
                        peer_id: String::new(),
                        tileset_id: ts_id,
                        chunk_seq: next_seq,
                    }).ok();
                }

                bevy::log::debug!(
                    "Chunk {chunk_seq} received for tileset {tileset_id} ({chunk_size} bytes)"
                );
            }
            fe_sync::SyncEvent::ChunkFailed {
                tileset_id,
                chunk_seq,
                reason,
            } => {
                bevy::log::warn!(
                    "Chunk {chunk_seq} failed for tileset {tileset_id}: {reason}"
                );
                if let ActiveDialog::HexonManager {
                    ref mut download_progress, ..
                } = ui_mgr.active_dialog
                {
                    if let Some(dl) = download_progress.get_mut(&tileset_id) {
                        dl.status = DownloadStatus::Failed(format!(
                            "Chunk {} failed: {}",
                            chunk_seq, reason
                        ));
                    }
                }
            }
            _ => {} // other SyncEvent variants handled elsewhere
        }
    }
}
