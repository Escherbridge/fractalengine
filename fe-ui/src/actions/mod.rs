//! UI action queue: `UiAction` + `UiManager` + the draining system that
//! dispatches each action to its domain handler. See `fe-ui/src/AGENTS.md`
//! §actions.

// `pub(crate)` on `asset`/`terrain_proposal` so `plugin.rs` can `init_resource`
// the Wave-1 interaction-state stubs they home (StampInteractionState /
// SculptToolState). See the Wave-1 registration scaffold below.
pub(crate) mod asset;
pub(crate) mod gis;
mod gpx;
mod hexon;
pub(crate) mod node;
pub(crate) mod node_props;
pub(crate) mod path;
pub(crate) mod portal;
mod query;
pub(crate) mod terrain_proposal;
mod transform;

use std::path::PathBuf;

use bevy::prelude::*;
use fe_webview::ipc::BrowserCommand;

use crate::asset_ops::PendingAssetOps;
use crate::dialogs::ActiveDialog;
use crate::plugin::InspectorFormState;
use crate::portal::PortalState;
use crate::terrain_map::{InstalledTilesetDto, PendingHexonOps, PetalManifest, PetalMapState};
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp};

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
    /// Create an empty node at a viewport world position in the active petal
    /// (context-menu "Add Empty Node").
    CreateNodeAt {
        position: [f32; 3],
    },
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
    /// Append a bezier anchor with handles (Pen press-drag / Alt-drag commit,
    /// pen_curve_tool_20260722 FR-7).
    PathAppendSmoothPoint {
        track_node_id: String,
        position: [f32; 3],
        handle_in: Option<[f32; 3]>,
        handle_out: Option<[f32; 3]>,
        corner: crate::gis::CornerKind,
        smoothness: f32,
    },
    /// Set anchor `index`'s bezier handles (+ smoothness) in place.
    PathSetAnchorHandles {
        track_node_id: String,
        index: usize,
        handle_in: Option<[f32; 3]>,
        handle_out: Option<[f32; 3]>,
        smoothness: f32,
    },
    /// Set anchor `index`'s corner classification in place.
    PathSetAnchorCorner {
        track_node_id: String,
        index: usize,
        corner: crate::gis::CornerKind,
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
    /// path_interaction_20260716 (FR-4): replace ALL of a track's points with
    /// `points` (world positions in order), keyed on the EXPLICIT track node id
    /// — used when the whole-path gimbal bakes its transform delta into the gpx
    /// points. Applied as one in-place `MovePoint` per index (count-preserving,
    /// timestamps kept by the bridge), independent of `editing_track_id`.
    PathTransformPoints {
        track_node_id: String,
        points: Vec<[f32; 3]>,
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
    // Terrain proposals — see `fe-ui/src/AGENTS.md` §terrain-proposal-editor.
    /// Add a proposed-overlay terrain edit (FR-5) to the active petal and persist
    /// the `proposals` block additively via `SetPetalTerrain`.
    TerrainProposalAdd {
        op: ProposalOp,
        footprint: Vec<[f32; 2]>,
        target_height: Option<f32>,
        delta: Option<f32>,
    },
    /// Delete a terrain proposal by id and re-persist the `proposals` block.
    TerrainProposalDelete {
        id: String,
    },

    // -----------------------------------------------------------------------
    // Wave-1 registration SCAFFOLD (spatial-builder-program-20260725).
    // T6 (shell_ux_sidebar) pre-declares every Wave-1 verb + a dispatch arm to
    // a per-track handler stub so T2/T3/T4 fill ONLY their leaf handler bodies
    // (`actions/{asset,path,node,node_props,terrain_proposal}.rs`) and never
    // touch this enum. Payloads are primitives (String/usize/arrays), NOT
    // Wave-1 types. See the anchor "Slice-time partition corrections".
    // -----------------------------------------------------------------------

    // --- T2 stamped_asset_nodes: individual stamp select + scale/rotate/slide ---
    /// Select a single stamp on a path as an individual (promoting) node
    /// (T2 FR-2). Identified pre-promotion by `(track_node_id, stamp_index)`.
    SelectStamp {
        track_node_id: String,
        stamp_index: usize,
    },
    /// Set a stamp's per-node scale override (T2 FR-3). Position stays
    /// path-derived; only scale/rotation are overridable.
    SetStampScale {
        track_node_id: String,
        stamp_index: usize,
        scale: [f32; 3],
    },
    /// Set a stamp's per-node rotation override (quaternion xyzw) (T2 FR-3).
    SetStampRotation {
        track_node_id: String,
        stamp_index: usize,
        rotation: [f32; 4],
    },
    /// Slide a stamp along its curve by arc-length in petal-local meters
    /// (T2 FR-3, Q-1 ratified) — free translate stays off.
    SlideStampAlongPath {
        track_node_id: String,
        stamp_index: usize,
        arc_length: f32,
    },

    // --- T3 sculpt_earthwork_regions: brush + shape region + delete ---
    /// Apply one distance-sampled freeform stroke within a petal. Metric panel
    /// values are already snapshotted into petal-local world units; the action
    /// persists every dab in one terrain-document update.
    SculptBrushStroke {
        petal_id: String,
        centers: Vec<[f32; 2]>,
        radius: f32,
        strength: f32,
        op: String,
        target_height: Option<f32>,
        delta: Option<f32>,
        material: String,
    },
    /// Create/update a defined-shape earthwork region node (T3 FR-1 shape +
    /// FR-3 region node + FR-4 volume). Footprint is petal-local meters (N-1).
    SculptShapeRegion {
        petal_id: String,
        footprint: Vec<[f32; 2]>,
        op: String,
        target_height: Option<f32>,
        delta: Option<f32>,
        material: String,
    },
    /// Delete an earthwork region node, reverting its baked contribution
    /// (T3 FR-3, Q-2 ratified).
    SculptDeleteRegion {
        region_id: String,
    },

    // --- T4 contextual_controls: object-aware verbs ---
    /// Delete an object via T1's sync-safe tombstone; `cascade` routes parent
    /// deletes through T1's cascade with confirm (T4 FR-2). Fixes the husk bug.
    DeleteNode {
        node_id: String,
        cascade: bool,
    },
    /// Duplicate an object (T4 FR-3).
    DuplicateNode {
        node_id: String,
    },
    /// Rename an object (T4 FR-3).
    RenameNode {
        node_id: String,
        name: String,
    },
    /// Promote an un-promoted stamp to a full node (T4 FR-3 / T2 FR-5), via
    /// T1 FR-5. Identified pre-promotion by `(track_node_id, stamp_index)`.
    PromoteStamp {
        track_node_id: String,
        stamp_index: usize,
    },
    /// Clear an object's custom properties WITHOUT deleting it — the distinct
    /// verb that is NOT delete (T4 FR-2 husk-bug clarification).
    ClearNodeProperties {
        node_id: String,
    },
    /// Copy the object's public API/egress string (T4 FR-3/FR-4). T4/T5 seam —
    /// handler no-ops until `endpoint_api_surface` (T5) lands the seam.
    CopyApiString {
        node_id: String,
    },
    /// Open the object's report/query view (T4 FR-3/FR-4). T4/T5 seam —
    /// handler no-ops until `endpoint_api_surface` (T5) lands the seam.
    ReportObject {
        node_id: String,
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

/// Tool-panel + proposal state PLUS the Wave-1 interaction-state stubs
/// (`StampInteractionState`/`SculptToolState`), bundled so `process_ui_actions`
/// stays under Bevy's 16-`SystemParam` tuple limit while the Wave-1 handler
/// stubs (T2/T3) get their resources threaded in already. See the Wave-1
/// registration scaffold + `plugin.rs` for registration.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ToolStateParams<'w> {
    tool_panel: ResMut<'w, crate::panels::tool_panel::ToolPanelState>,
    proposal_state: ResMut<'w, ProposalEditState>,
    stamp_state: ResMut<'w, asset::StampInteractionState>,
    sculpt_state: ResMut<'w, terrain_proposal::SculptToolState>,
    /// T3: region_id↔node_id bookkeeping for earthwork endpoint rows (D-A8).
    earthwork_map: ResMut<'w, terrain_proposal::EarthworkNodeMap>,
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
    tool_state: ToolStateParams,
) {
    let GisPathParams {
        mut gis_panel,
        mut gpx_ops,
        mut path_state,
        mut path_ops,
    } = gis;
    let ToolStateParams {
        mut tool_panel,
        mut proposal_state,
        mut stamp_state,
        mut sculpt_state,
        mut earthwork_map,
    } = tool_state;
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
                if let Err(reason) =
                    transform::apply(&inspector, &mut node_mgr, &mut transform_query)
                {
                    ui_mgr.show_toast(format!("Transform not applied — {reason}"), now_secs);
                }
            }
            UiAction::CreateNodeAt { position } => match nav.active_petal_id.as_deref() {
                Some(petal_id) => {
                    node::create_at(&db_sender, petal_id.to_string(), position);
                }
                None => {
                    ui_mgr.show_toast("Navigate to a petal first", now_secs);
                }
            },
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
            UiAction::PathAppendSmoothPoint {
                track_node_id,
                position,
                handle_in,
                handle_out,
                corner,
                smoothness,
            } => {
                path::append_smooth_point(
                    &mut path_ops,
                    &mut path_state,
                    track_node_id,
                    crate::gis::PathPointRow {
                        position,
                        time_seconds: None,
                        handle_in,
                        handle_out,
                        corner,
                        smoothness,
                    },
                );
            }
            UiAction::PathSetAnchorHandles {
                track_node_id,
                index,
                handle_in,
                handle_out,
                smoothness,
            } => {
                path::set_anchor_handles(
                    &mut path_ops,
                    &mut path_state,
                    track_node_id,
                    index,
                    handle_in,
                    handle_out,
                    smoothness,
                );
            }
            UiAction::PathSetAnchorCorner {
                track_node_id,
                index,
                corner,
            } => {
                path::set_anchor_corner(
                    &mut path_ops,
                    &mut path_state,
                    track_node_id,
                    index,
                    corner,
                );
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
            UiAction::PathTransformPoints {
                track_node_id,
                points,
            } => {
                path::transform_points(&mut path_ops, &mut path_state, track_node_id, points);
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
                // Persist the descriptor on the track node; the property load feeds
                // `PathAssetCache` and `materialize_path_assets` (verse_manager) stamps.
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
            UiAction::TerrainProposalAdd {
                op,
                footprint,
                target_height,
                delta,
            } => {
                terrain_proposal::add(
                    &db_sender,
                    &mut petal_map,
                    &mut proposal_state,
                    nav.active_petal_id.clone(),
                    op,
                    footprint,
                    target_height,
                    delta,
                );
            }
            UiAction::TerrainProposalDelete { id } => {
                terrain_proposal::delete(
                    &db_sender,
                    &mut petal_map,
                    &mut proposal_state,
                    nav.active_petal_id.clone(),
                    id,
                );
            }

            // ---- Wave-1 dispatch — each arm calls its per-track handler
            // (T2/T3/T4 leaf logic + integration). See the enum block above. ----
            // T2 stamped_asset_nodes (asset.rs):
            UiAction::SelectStamp {
                track_node_id,
                stamp_index,
            } => {
                asset::handle_select_stamp(&mut stamp_state, track_node_id, stamp_index);
                // T2 FR-5 / N-9: the FIRST individual select of an un-promoted
                // stamp queues exactly one PromoteStamp (drained here so the
                // marker can never fire twice; dispatched next drain pass).
                if let Some(pending) = stamp_state.take_pending_promotion() {
                    ui_mgr.push_action(UiAction::PromoteStamp {
                        track_node_id: pending.track_node_id,
                        stamp_index: pending.stamp_index,
                    });
                }
            }
            UiAction::SetStampScale {
                track_node_id,
                stamp_index,
                scale,
            } => {
                asset::handle_set_stamp_scale(
                    &db_sender,
                    &mut stamp_state,
                    track_node_id,
                    stamp_index,
                    scale,
                );
            }
            UiAction::SetStampRotation {
                track_node_id,
                stamp_index,
                rotation,
            } => {
                asset::handle_set_stamp_rotation(
                    &db_sender,
                    &mut stamp_state,
                    track_node_id,
                    stamp_index,
                    rotation,
                );
            }
            UiAction::SlideStampAlongPath {
                track_node_id,
                stamp_index,
                arc_length,
            } => {
                // Arc-length slide is curve/path-domain → routed to path.rs (T2).
                path::handle_slide_stamp(
                    &mut path_state,
                    &mut stamp_state,
                    track_node_id,
                    stamp_index,
                    arc_length,
                );
            }
            // T3 sculpt_earthwork_regions (terrain_proposal.rs):
            UiAction::SculptBrushStroke {
                petal_id,
                centers,
                radius,
                strength,
                op,
                target_height,
                delta,
                material,
            } => {
                terrain_proposal::handle_brush_stroke(
                    &db_sender,
                    &mut petal_map,
                    &mut sculpt_state,
                    &mut earthwork_map,
                    petal_id,
                    centers,
                    radius,
                    strength,
                    op,
                    target_height,
                    delta,
                    material,
                );
            }
            UiAction::SculptShapeRegion {
                petal_id,
                footprint,
                op,
                target_height,
                delta,
                material,
            } => {
                terrain_proposal::handle_shape_region(
                    &db_sender,
                    &mut petal_map,
                    &mut sculpt_state,
                    &mut earthwork_map,
                    petal_id,
                    footprint,
                    op,
                    target_height,
                    delta,
                    material,
                );
            }
            UiAction::SculptDeleteRegion { region_id } => {
                terrain_proposal::handle_delete_region(
                    &db_sender,
                    &mut petal_map,
                    &mut sculpt_state,
                    &mut earthwork_map,
                    nav.active_petal_id.clone(),
                    region_id,
                );
            }
            // T4 contextual_controls (node.rs / node_props.rs):
            UiAction::DeleteNode { node_id, cascade } => {
                node::handle_delete(&db_sender, node_id, cascade);
            }
            UiAction::DuplicateNode { node_id } => {
                node::handle_duplicate(&db_sender, node_id);
            }
            UiAction::RenameNode { node_id, name } => {
                node::handle_rename(&db_sender, node_id, name);
            }
            UiAction::PromoteStamp {
                track_node_id,
                stamp_index,
            } => {
                node::handle_promote_stamp(
                    &db_sender,
                    &mut stamp_state,
                    nav.active_petal_id.as_deref(),
                    track_node_id,
                    stamp_index,
                );
            }
            UiAction::ClearNodeProperties { node_id } => {
                node_props::handle_clear(&db_sender, node_id);
            }
            UiAction::CopyApiString { node_id } => {
                // Wave 1 / needs T5: endpoint_api_surface supplies the address→
                // string seam; handler no-ops (shown disabled-with-hint by T4).
                node::handle_copy_api(&mut ui_mgr, node_id, now_secs);
            }
            UiAction::ReportObject { node_id } => {
                // Wave 1 / needs T5: endpoint_api_surface supplies the report
                // seam; handler no-ops (shown disabled-with-hint by T4).
                node::handle_report(&mut ui_mgr, node_id, now_secs);
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
            target: None,
            pending_delete: false,
            descendant_count: None,
        });
        if let ActiveDialog::ContextMenu {
            world_pos, target, ..
        } = &mgr.active_dialog
        {
            assert_eq!(*world_pos, [5.0, 0.0, -3.0]);
            assert!(
                target.is_none(),
                "opens unclassified; context_pick fills it"
            );
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
            target: None,
            pending_delete: false,
            descendant_count: None,
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
