//! UI action queue: `UiAction` + `UiManager` + the draining system that
//! dispatches each action to its domain handler. See `fe-ui/src/AGENTS.md`
//! §actions.

mod asset;
mod hexon;
mod node_props;
pub(crate) mod portal;
mod query;

use std::path::PathBuf;

use bevy::prelude::*;
use fe_webview::ipc::BrowserCommand;

use crate::asset_ops::PendingAssetOps;
use crate::dialogs::ActiveDialog;
use crate::plugin::InspectorFormState;
use crate::portal::PortalState;
use crate::terrain_map::{InstalledTilesetDto, PendingHexonOps, PetalManifest, PetalMapState};

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
    /// Set (Some) or clear (None) the active petal's map tileset. See AGENTS.md §terrain-map.
    PetalSetMap { petal_id: String, tileset: Option<InstalledTilesetDto> },
    // Petal Manifest actions
    PetalManifestSave { petal_id: String, manifest: PetalManifest },
    PetalManifestOpen { petal_id: String, petal_name: String },
    /// Download the given node's asset. Queued for the main binary; see
    /// `crate::asset_ops` for the pending-ops/result-status contract.
    DownloadNodeAsset { node_id: String },
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

/// Drains all UiActions queued during the egui pass and processes them.
/// Replaces: forward_webview_open_request, drain_portal_panel_actions, handle_url_save.
pub(crate) fn process_ui_actions(
    mut ui_mgr: ResMut<UiManager>,
    inspector: Res<InspectorFormState>,
    node_mgr: Res<crate::node_manager::NodeManager>,
    mut browser_commands: MessageWriter<BrowserCommand>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut petal_map: ResMut<PetalMapState>,
    mut hexon_ops: ResMut<PendingHexonOps>,
    mut asset_ops: ResMut<PendingAssetOps>,
    nav: Res<crate::navigation_manager::NavigationManager>,
) {
    // Auto-close portal when the selected entity changes or is deselected.
    if portal::should_auto_close(&ui_mgr.portal, node_mgr.selected_entity()) {
        ui_mgr.portal = PortalState::Closed;
        browser_commands.write(BrowserCommand::Close);
    }

    let actions = ui_mgr.drain_actions();
    for action in actions {
        match action {
            UiAction::OpenPortal { url } => match portal::compute_open_portal(&node_mgr, &url) {
                portal::OpenPortalOutcome::Navigate(new_state, parsed) => {
                    bevy::log::info!("Portal: forwarding Navigate for URL: {parsed}");
                    ui_mgr.portal = new_state;
                    browser_commands.write(BrowserCommand::Navigate { url: parsed });
                }
                portal::OpenPortalOutcome::NoSelection => {}
                portal::OpenPortalOutcome::InvalidUrl(e) => {
                    bevy::log::warn!("UiAction::OpenPortal invalid URL: {e}");
                }
            },
            UiAction::ClosePortal => {
                ui_mgr.portal = PortalState::Closed;
                browser_commands.write(BrowserCommand::Close);
            }
            UiAction::PortalGoBack => {
                browser_commands.write(BrowserCommand::GoBack);
            }
            UiAction::SaveUrl => {
                if let Some((node_id, url)) = portal::compute_save_url(&node_mgr, &inspector) {
                    verse_mgr.update_node_url(&node_id, url.clone());
                    if db_sender
                        .0
                        .send(fe_runtime::messages::DbCommand::UpdateNodeUrl { node_id, url })
                        .is_err()
                    {
                        bevy::log::warn!("db_sender channel closed — UpdateNodeUrl not persisted");
                    }
                }
            }
            UiAction::SubmitQuery { sql, scope: _ } => {
                query::submit(&db_sender, sql);
            }
            UiAction::LoadNodeProperties { node_id } => {
                node_props::load(&db_sender, node_id);
            }
            UiAction::SetNodeProperty { node_id, key, value } => {
                node_props::set(&db_sender, node_id, key, value);
            }
            UiAction::DeleteNodeProperty { node_id, key } => {
                node_props::delete(&db_sender, node_id, key);
            }
            UiAction::HexonInstallFromFile(path) => {
                hexon::install_from_file(&mut hexon_ops, path);
            }
            UiAction::HexonRemoveTileset(id) => {
                hexon::remove_tileset(&mut hexon_ops, id);
            }
            UiAction::HexonToggleSeeding(id, enabled) => {
                hexon::toggle_seeding(
                    &mut hexon_ops,
                    sync_sender.as_deref(),
                    nav.active_verse_id.as_deref(),
                    id,
                    enabled,
                );
            }
            UiAction::HexonStartDownload(id) => {
                hexon::start_download(&mut ui_mgr, sync_sender.as_deref(), id);
            }
            UiAction::HexonCancelDownload(id) => {
                hexon::cancel_download(&mut ui_mgr, sync_sender.as_deref(), id);
            }
            UiAction::HexonRefreshList => {
                hexon::refresh_list(&mut hexon_ops, sync_sender.as_deref(), nav.active_verse_id.as_deref());
            }
            UiAction::HexonOpenStorageDir => {
                hexon::open_storage_dir(&ui_mgr);
            }
            UiAction::PetalSetMap { petal_id, tileset } => {
                hexon::set_petal_map(&db_sender, &mut petal_map, petal_id, tileset);
            }
            UiAction::PetalManifestSave { petal_id, manifest } => {
                hexon::manifest_save(petal_id, manifest);
            }
            UiAction::PetalManifestOpen { petal_id, petal_name } => {
                hexon::manifest_open(&mut ui_mgr, petal_id, petal_name);
            }
            UiAction::DownloadNodeAsset { node_id } => {
                asset::request_download(&mut asset_ops, node_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_dialog_context_menu_captures_world_pos() {
        // Simulate the context menu capturing a non-zero world position via ActiveDialog.
        let cursor_world = crate::plugin::ViewportCursorWorld {
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
        let cursor_world = crate::plugin::ViewportCursorWorld { pos: None };
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
            entity_type: crate::dialogs::EntitySettingsType::Verse,
            entity_id: "v1".to_string(),
            entity_name: "Test Verse".to_string(),
            parent_verse_id: "v1".to_string(),
            parent_fractal_id: None,
            active_tab: crate::dialogs::SettingsTab::General,
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
