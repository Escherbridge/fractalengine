//! UI action queue: `UiAction` + `UiManager` + the draining system that
//! dispatches each action to its domain handler. See `fe-ui/src/AGENTS.md`
//! §actions.

mod asset;
pub(crate) mod gis;
mod gpx;
mod hexon;
pub(crate) mod node_props;
pub(crate) mod path;
pub(crate) mod portal;
mod query;
mod transform;

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
    OpenPortal {
        url: String,
    },
    /// Close the portal webview (replaces PortalPanelState.close).
    ClosePortal,
    /// Navigate back in portal history (replaces PortalPanelState.go_back).
    PortalGoBack,
    /// Save URL for the selected node (replaces InspectorFormState.url_save_pending).
    SaveUrl,
    /// Apply the inspector's Position/Rotation/Scale text buffers to the
    /// selected node's `Transform`. See AGENTS.md §inspector-transform.
    ApplyNodeTransform,
    /// Submit a SurrealQL query via the API gateway.
    SubmitQuery {
        sql: String,
        scope: String,
    },
    /// Request loading properties for selected node.
    LoadNodeProperties {
        node_id: String,
    },
    /// Set a property value on a node.
    SetNodeProperty {
        node_id: String,
        key: String,
        value: serde_json::Value,
    },
    /// Delete a property from a node.
    DeleteNodeProperty {
        node_id: String,
        key: String,
    },
    // Hexon Manager actions
    HexonInstallFromFile(PathBuf),
    HexonRemoveTileset(String),
    HexonToggleSeeding(String, bool),
    HexonStartDownload(String),
    HexonCancelDownload(String),
    HexonRefreshList,
    HexonOpenStorageDir,
    /// Set (Some) or clear (None) the active petal's map tileset. See AGENTS.md §terrain-map.
    PetalSetMap {
        petal_id: String,
        tileset: Option<InstalledTilesetDto>,
    },
    /// Set the active petal's map world scale (world units per real meter). See AGENTS.md §terrain-map.
    PetalSetMapScale {
        petal_id: String,
        tileset: InstalledTilesetDto,
        world_scale: f64,
    },
    // Petal Manifest actions
    PetalManifestSave {
        petal_id: String,
        manifest: PetalManifest,
    },
    PetalManifestOpen {
        petal_id: String,
        petal_name: String,
    },
    /// Download the given node's asset. Queued for the main binary; see
    /// `crate::asset_ops` for the pending-ops/result-status contract.
    DownloadNodeAsset {
        node_id: String,
    },
    // GIS query panel + layer manager actions — see AGENTS.md §gis-query-ui.
    /// Run the "nodes with annotations" query for a petal.
    GisQueryAnnotated {
        petal_id: String,
    },
    /// Run the property key/value filter query for a petal.
    GisQueryPropertyFilter {
        petal_id: String,
        key: String,
        value: serde_json::Value,
    },
    /// Toggle a terrain layer's visibility/opacity on the active petal's map.
    GisSetLayer {
        petal_id: String,
        layer_name: String,
        visible: Option<bool>,
        opacity: Option<f32>,
    },
    /// Set the active petal's splat view mode (`"mesh"|"splats"|"hybrid"`).
    GisSetViewMode {
        petal_id: String,
        view_mode: String,
    },
    /// Queue a GPX track file for import into the given petal. Resolved by
    /// the main binary — see `crate::gpx_ops` for the pending-ops/status
    /// contract (mirrors `DownloadNodeAsset`/`asset_ops`).
    GpxImportFile {
        petal_id: String,
        path: PathBuf,
    },
    // Path editor actions — see AGENTS.md §path-editor.
    /// Run the "track nodes" query for a petal (Paths tab track list).
    PathQueryTracks {
        petal_id: String,
    },
    /// Select a track for editing and read back its persisted `gpx_points`
    /// via `GetNodeProperties`/`NodePropertiesLoaded`.
    PathSelectTrack {
        track_node_id: String,
    },
    /// Create a new (empty) track node named `name` under `petal_id`.
    /// `correlation_id` is `Some` only for the Pen auto-create (so its deferred
    /// `NodeCreated` flush can match by id); the manual "New Path" button sends
    /// `None`. See `crate::path_ops::PathOp::CreateTrack`.
    PathCreateTrack {
        petal_id: String,
        name: String,
        correlation_id: Option<String>,
    },
    /// Delete a track node and its persisted points.
    PathDeleteTrack {
        track_node_id: String,
    },
    /// Append a point at the current 3D cursor world position.
    PathAppendPoint {
        track_node_id: String,
        position: [f32; 3],
    },
    /// Remove the point at `index` from a track's point list.
    PathRemovePoint {
        track_node_id: String,
        index: usize,
    },
    /// Move the point at `index` to a new world position (viewport drag commit).
    PathMovePoint {
        track_node_id: String,
        index: usize,
        position: [f32; 3],
    },
    /// Create a waypoint annotation at point `index`'s position.
    PathAnnotatePoint {
        track_node_id: String,
        index: usize,
        title: String,
        body: String,
        color: String,
    },
    /// Queue a GPX export for a track node. Resolved by the main binary —
    /// see `crate::path_ops` for the pending-ops/status contract.
    PathExportGpx {
        track_node_id: String,
    },
    /// track_styling_20260713: set per-track render style. Each `Some` field is
    /// written to its `gis.track.*` node property via `SetNodeProperty`; the
    /// gpx bridge re-reads and restyles the ribbon live. `None` fields are left
    /// unchanged (only the control the user touched writes).
    PathSetStyle {
        track_node_id: String,
        color: Option<[f32; 4]>,
        width: Option<f32>,
        visible: Option<bool>,
    },
    /// Write a path-asset stamp descriptor to a track node's `path_asset`
    /// property (via `SetNodeProperty`). The `reconcile_path_asset` system
    /// then stamps the model along the track's `gpx_points`. See
    /// `fe-ui/src/verse_manager/AGENTS.md` §path-asset-stamp.
    PathAssetApply {
        track_node_id: String,
        descriptor: fe_sdk::path_asset::PathAssetDescriptor,
    },
    // Pen-tool phase-2 actions (curves + shapes) — see
    // `node_manager/AGENTS.md` §pen-tool. Both re-express the result as
    // existing Remove/Append `PathOp`s so no gpx-bridge change is needed.
    /// Resample the currently-edited track's control points via
    /// `node_manager::curve::resample` and REPLACE the track's points with the
    /// result (clear-then-append). No-op if no track is being edited.
    PathSmoothCurrent {
        mode: crate::node_manager::curve::PenMode,
        tension: f32,
        samples_per_segment: usize,
    },
    /// Append pre-generated shape points (ellipse/circle/rectangle from
    /// `node_manager::curve`) to the currently-edited track. No-op if no track
    /// is being edited. Shape math runs panel-side; this only carries points.
    PathAppendShape {
        points: Vec<[f32; 3]>,
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
            PortalState::Open {
                cached_hostname, ..
            } => cached_hostname,
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

/// Bundle of GIS/GPX/path-editor queue+state params to keep
/// `process_ui_actions` under Bevy's 16-param `SystemParam` tuple limit as
/// new panel surfaces (Query/Annotations/Layers/Paths tabs) land — mirrors
/// `plugin::MiscUiParams`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GisPathParams<'w> {
    gis_panel: ResMut<'w, crate::gis::GisPanelState>,
    gpx_ops: ResMut<'w, crate::gpx_ops::PendingGpxOps>,
    path_state: ResMut<'w, crate::gis::PathEditorState>,
    path_ops: ResMut<'w, crate::path_ops::PendingPathOps>,
}

/// Drains all UiActions queued during the egui pass and processes them.
/// Replaces: forward_webview_open_request, drain_portal_panel_actions, handle_url_save.
pub(crate) fn process_ui_actions(
    mut ui_mgr: ResMut<UiManager>,
    inspector: Res<InspectorFormState>,
    mut node_mgr: ResMut<crate::node_manager::NodeManager>,
    mut transform_query: Query<&mut Transform>,
    mut browser_commands: MessageWriter<BrowserCommand>,
    mut verse_mgr: ResMut<crate::verse_manager::VerseManager>,
    db_sender: Res<fe_runtime::app::DbCommandSender>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    mut petal_map: ResMut<PetalMapState>,
    mut hexon_ops: ResMut<PendingHexonOps>,
    mut asset_ops: ResMut<PendingAssetOps>,
    nav: Res<crate::navigation_manager::NavigationManager>,
    time: Res<Time>,
    gis: GisPathParams,
    mut tool_panel: ResMut<crate::panels::tool_panel::ToolPanelState>,
) {
    let GisPathParams {
        mut gis_panel,
        mut gpx_ops,
        mut path_state,
        mut path_ops,
    } = gis;
    // Fold pen-tool actions queued by `render_tool_panel` into the main queue
    // (the Tools panel has no `ui_mgr` handle — see panels/tool_panel.rs).
    for pen_action in tool_panel.drain_pending() {
        ui_mgr.push_action(pen_action);
    }
    // egui reads toast time from the same Bevy clock (bevy_egui feeds
    // `raw_input.time` from `Time`), so this is the correct scale for show_toast.
    let now_secs = time.elapsed_secs_f64();

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
                // Every outcome is surfaced to the user (toast) — no silent
                // drops; blocked URLs never reach the DB (see AGENTS.md §portal).
                match portal::compute_save_url(&node_mgr, &inspector) {
                    portal::SaveUrlOutcome::Persist { node_id, url } => {
                        verse_mgr.update_node_url(&node_id, url.clone());
                        if db_sender
                            .0
                            .send(fe_runtime::messages::DbCommand::UpdateNodeUrl { node_id, url })
                            .is_err()
                        {
                            bevy::log::warn!(
                                "db_sender channel closed — UpdateNodeUrl not persisted"
                            );
                        } else {
                            ui_mgr.show_toast("URL saved", now_secs);
                        }
                    }
                    portal::SaveUrlOutcome::Blocked { reason } => {
                        bevy::log::warn!("UiAction::SaveUrl rejected: {reason}");
                        ui_mgr.show_toast(format!("URL not saved — {reason}"), now_secs);
                    }
                    portal::SaveUrlOutcome::NoSelection => {
                        ui_mgr.show_toast("No node selected", now_secs);
                    }
                }
            }
            UiAction::ApplyNodeTransform => {
                transform::apply(&inspector, &mut node_mgr, &mut transform_query);
            }
            UiAction::SubmitQuery { sql, scope: _ } => {
                query::submit(&db_sender, sql);
            }
            UiAction::LoadNodeProperties { node_id } => {
                node_props::load(&db_sender, node_id);
            }
            UiAction::SetNodeProperty {
                node_id,
                key,
                value,
            } => {
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
                hexon::refresh_list(
                    &mut hexon_ops,
                    sync_sender.as_deref(),
                    nav.active_verse_id.as_deref(),
                );
            }
            UiAction::HexonOpenStorageDir => {
                hexon::open_storage_dir(&ui_mgr);
            }
            UiAction::PetalSetMap { petal_id, tileset } => {
                hexon::set_petal_map(&db_sender, &mut petal_map, petal_id, tileset);
            }
            UiAction::PetalSetMapScale {
                petal_id,
                tileset,
                world_scale,
            } => {
                hexon::set_petal_map_scale(
                    &db_sender,
                    &mut petal_map,
                    petal_id,
                    tileset,
                    world_scale,
                );
            }
            UiAction::PetalManifestSave { petal_id, manifest } => {
                hexon::manifest_save(petal_id, manifest);
            }
            UiAction::PetalManifestOpen {
                petal_id,
                petal_name,
            } => {
                hexon::manifest_open(&mut ui_mgr, petal_id, petal_name);
            }
            UiAction::DownloadNodeAsset { node_id } => {
                asset::request_download(&mut asset_ops, node_id);
            }
            UiAction::GisQueryAnnotated { petal_id } => {
                gis::query_annotated(&db_sender, &mut gis_panel, petal_id);
            }
            UiAction::GisQueryPropertyFilter {
                petal_id,
                key,
                value,
            } => {
                gis::query_property_filter(&db_sender, &mut gis_panel, petal_id, key, value);
            }
            UiAction::GisSetLayer {
                petal_id,
                layer_name,
                visible,
                opacity,
            } => {
                gis::set_layer(
                    &db_sender,
                    &mut petal_map,
                    petal_id,
                    layer_name,
                    visible,
                    opacity,
                );
            }
            UiAction::GisSetViewMode {
                petal_id,
                view_mode,
            } => {
                gis::set_view_mode(&db_sender, &mut petal_map, petal_id, view_mode);
            }
            UiAction::GpxImportFile { petal_id, path } => {
                gpx::request_import(&mut gpx_ops, petal_id, path);
            }
            UiAction::PathQueryTracks { petal_id } => {
                path::query_tracks(&db_sender, &mut path_state, petal_id);
            }
            UiAction::PathSelectTrack { track_node_id } => {
                path::select_track(&db_sender, &mut path_state, track_node_id);
            }
            UiAction::PathCreateTrack {
                petal_id,
                name,
                correlation_id,
            } => {
                if let Err(err) = path::create_track(&mut path_ops, petal_id, name, correlation_id)
                {
                    path_state.last_error = Some(err.to_string());
                } else {
                    path_state.last_error = None;
                }
            }
            UiAction::PathDeleteTrack { track_node_id } => {
                if path_state.editing_track_id.as_deref() == Some(track_node_id.as_str()) {
                    path_state.stop_editing();
                }
                path::delete_track(&mut path_ops, track_node_id);
            }
            UiAction::PathAppendPoint {
                track_node_id,
                position,
            } => {
                path::append_point(&mut path_ops, &mut path_state, track_node_id, position);
            }
            UiAction::PathRemovePoint {
                track_node_id,
                index,
            } => {
                path::remove_point(&mut path_ops, &mut path_state, track_node_id, index);
            }
            UiAction::PathMovePoint {
                track_node_id,
                index,
                position,
            } => {
                path::move_point(
                    &mut path_ops,
                    &mut path_state,
                    track_node_id,
                    index,
                    position,
                );
            }
            UiAction::PathAnnotatePoint {
                track_node_id,
                index,
                title,
                body,
                color,
            } => {
                path::annotate_point(&mut path_ops, track_node_id, index, title, body, color);
            }
            UiAction::PathExportGpx { track_node_id } => {
                path::export_gpx(&mut path_ops, track_node_id);
            }
            UiAction::PathSetStyle {
                track_node_id,
                color,
                width,
                visible,
            } => {
                path::set_style(&db_sender, track_node_id, color, width, visible);
            }
            UiAction::PathAssetApply {
                track_node_id,
                descriptor,
            } => {
                // Persist the descriptor on the track node; `reconcile_path_asset`
                // (verse_manager) stamps the model on the resulting property load.
                node_props::set(
                    &db_sender,
                    track_node_id,
                    fe_sdk::path_asset::PATH_ASSET_PROPERTY_KEY.to_string(),
                    descriptor.to_json(),
                );
            }
            UiAction::PathSmoothCurrent {
                mode,
                tension,
                samples_per_segment,
            } => {
                path::smooth_current(
                    &mut path_ops,
                    &mut path_state,
                    mode,
                    tension,
                    samples_per_segment,
                );
            }
            UiAction::PathAppendShape { points } => {
                path::append_shape(&mut path_ops, &mut path_state, points);
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
        assert!(matches!(
            mgr.active_dialog,
            ActiveDialog::EntitySettings { .. }
        ));
        mgr.close_dialog();
        assert!(!mgr.any_dialog_open());
    }
}
