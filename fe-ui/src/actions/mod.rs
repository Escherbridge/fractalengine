//! UI action queue: `UiAction` + `UiManager` + the draining system that
//! dispatches each action to its domain handler. See `fe-ui/src/AGENTS.md`
//! §actions.

// `pub(crate)` on `asset`/`terrain_proposal` so `plugin.rs` can `init_resource`
// the Wave-1 interaction-state stubs they home (StampInteractionState /
// SculptToolState). See the Wave-1 registration scaffold below.
pub(crate) mod asset;
/// HS-P6-CALLOUT: promote a completed Measure reading into an addressable
/// `dimension_callout` node. See AGENTS.md §dimension-callout.
pub(crate) mod dimension_callout;
pub(crate) mod gis;
mod gpx;
pub(crate) mod hexon;
pub(crate) mod node;
pub(crate) mod node_props;
pub(crate) mod path;
pub(crate) mod portal;
mod query;
pub(crate) use query::next_ui_query_id;
pub(crate) mod terrain_proposal;
mod transform;

use std::collections::VecDeque;

use bevy::prelude::*;
use fe_webview::ipc::BrowserCommand;

use crate::asset_ops::PendingAssetOps;
use crate::dialogs::ActiveDialog;
use crate::notice::{Notice, Severity};
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
    HexonInstallFromFile(crate::file_dialogs::DialogPath),
    HexonRemoveTileset(String),
    HexonToggleSeeding(String, bool),
    HexonStartDownload(String),
    HexonCancelDownload(String),
    HexonRefreshList,
    HexonOpenStorageDir,
    /// PAS-P5 S7b: install a hexon by URI from the configured hosted registry.
    HexonInstallFromRegistry(String),
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
    /// Session-only residency override for `petal_id` (S6) — a local-only
    /// knob, never persisted; `None` clears it. See `settings::PetalRenderOverride`.
    PetalSetRenderOverride {
        petal_id: String,
        render_distance: Option<f32>,
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
        path: crate::file_dialogs::DialogPath,
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
    PathMovePointFromGesture {track_node_id:String,index:usize,position:[f32;3],origin:crate::undo::PointEditOrigin},
    PathMovePointUntracked {track_node_id:String,index:usize,position:[f32;3]},
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
        closed: Option<bool>,
        fill_color: Option<[f32; 4]>,
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
    /// D16 (ui_semantics_unification_20260808 finding #4): select a proposal
    /// for editing — the context menu's `EditRegionParams` verb pairs this
    /// with `RevealSection { slug: "terrain" }` so the Terrain section opens
    /// already aimed at the region the user right-clicked. A pure state write
    /// (`ProposalEditState.selected`), same pattern as the panel's own
    /// select-on-click (`panels::terrain_tools_panel`).
    TerrainProposalSelect {
        id: String,
    },
    /// Select an existing active-Petal terrain target and reveal its measured report.
    OpenTerrainReport {
        petal_id: String,
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
    /// FR-3 region node + FR-4 volume). `footprint`/`target_height`/`delta`
    /// are queued in petal-local meters (N-1) — the handler
    /// (`actions::terrain_proposal::handle_shape_region`) converts them to
    /// world units via `PetalMapState.world_scale` before persisting,
    /// mirroring `BrushSnapshot::from_state`'s conversion, so every
    /// downstream consumer of `terrain.proposals` (context-menu pick, report
    /// totals, sibling Brush regions) sees consistent world units (finding
    /// #11 fix, `ui_semantics_unification_20260808`).
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
    /// Promote a completed Measure reading (Tape or closed Area) into a
    /// persistent, addressable `dimension_callout` node (HS-P6-CALLOUT). The
    /// Options panel's "Save as callout" button computes every field from
    /// the SAME same-frame measurement it is already displaying, so a later
    /// edit to the live `MeasureState` can't retroactively change what gets
    /// persisted. Carries plain data rather than
    /// `dimension_callout::CalloutGeometry` — that type is `pub(crate)`
    /// (owned by a `pub(crate)` submodule), and this enum is `pub`. `mode` is
    /// `"tape"`/`"area"` (`dimension_callout::mode_str`); `anchors`/`closed`
    /// are the same shape `dimension_callout::callout_properties` persists.
    SaveMeasureAsCallout {
        petal_id: String,
        name: String,
        mode: String,
        anchors: Vec<[f64; 3]>,
        closed: bool,
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

    // --- ui_semantics_unification_20260808: addressable UI surfaces ---
    /// D11: reveal a right-sidebar surface by its stable machine handle
    /// (`RightSidebarSection::slug`). The ONE addressable reveal path — panel
    /// cross-links, the context menu, and later fe-api/MCP all address a
    /// surface by string rather than reaching into `RightSidebarState`.
    /// Idempotent by design (a reveal never closes what it addressed); the
    /// topbar/rail keep their local `toggle`, and `ToolState::activate` keeps
    /// its same-frame direct write. An unknown slug is a logged no-op.
    RevealSection {
        slug: String,
    },

    // --- hierarchy_visibility_groups Phase 3 ---
    /// Set a node's tri-state visibility override: optimistic session write +
    /// authed `DbCommand::SetNodeView` (D-15). Produced by the sidebar eye, the
    /// H key and the row menu's Hide others / Show all. See
    /// `visibility::cache::set_node_view`.
    SetNodeView {
        node_id: String,
        state: crate::visibility::OverrideState,
    },

    // --- hierarchy_visibility_groups Phases 4-5 ---
    /// Replace the active petal's whole Group registry: optimistic session
    /// write + authed `SetPetalVisibilityGroups` (D-3). Produced by the right
    /// sidebar Groups section, the left sidebar Group eyes and the Layer
    /// Manager. See `visibility::groups::set_group_registry`.
    SetGroupRegistry {
        petal_id: String,
        groups: Vec<crate::visibility::Group>,
    },
    /// Open/close a verse/fractal/petal eye: optimistic + authed
    /// `SetScopeHidden` (D-1). The active petal's eye is never offered (D-4).
    SetScopeHidden {
        level: fe_runtime::messages::ScopeLevel,
        id: String,
        hidden: bool,
    },
    /// Replace one node's Group membership (`view.groups`) via the authed
    /// `SetNodeView { groups }` — "Add selection to group" / "Remove from group".
    SetNodeGroups {
        node_id: String,
        groups: Vec<String>,
    },
    /// Toggle a ruler overlay row of the unified overlay registry
    /// (`OverlayKey::ScaleBar` / `WorldGrid`) in `AppSettings`.
    SetAppOverlay {
        key: crate::gis::OverlayKey,
        visible: bool,
    },
}

/// In-flight round-trip of a dialog that stays open until its DB reply lands
/// (OD-2) — shared by Join (`join_request`) and glTF Import
/// (`gltf_import_request`). The dialog `start`s it on a successful send; the
/// `db_results` reply arm `finish`es it. See `fe-ui/src/actions/AGENTS.md`
/// §notices "Dialog requests".
#[derive(Debug, Clone, Default)]
pub struct DialogRequestState {
    pub pending: bool,
    /// Humanized failure for the dialog to render inline.
    pub error: Option<String>,
    /// When the request was sent (seconds since startup).
    pub sent_at: f64,
}

impl DialogRequestState {
    /// Marks a request in flight and clears the previous attempt's error.
    pub fn start(&mut self, now_secs: f64) {
        self.pending = true;
        self.error = None;
        self.sent_at = now_secs;
    }

    /// Ends the in-flight request; `true` when one was pending, so only the
    /// reply to a dialog-sent request closes or annotates the dialog.
    pub fn finish(&mut self) -> bool {
        std::mem::take(&mut self.pending)
    }

    /// Forgets the request (dialog closed or cancelled).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Pending and younger than `timeout_secs` — the bound for a dialog whose
    /// failure reply is an uncorrelated `DbResult::Error` that never
    /// `finish`es it.
    pub fn waiting_within(&self, now_secs: f64, timeout_secs: f64) -> bool {
        self.pending && now_secs - self.sent_at < timeout_secs
    }
}

/// Max notices simultaneously queued (OD-1). A push first drops expired
/// notices, then (still at cap) evicts the oldest of the lowest severity —
/// but never one that outranks the incoming notice: an `Info` arriving into
/// four live `Error`s is dropped instead (it could not surface before they
/// expire anyway).
const NOTICE_QUEUE_CAP: usize = 4;

/// Max retained entries in the `Error` history ring (OD-1).
const ERROR_HISTORY_CAP: usize = 20;

/// Centralized UI state resource.
#[derive(Resource)]
pub struct UiManager {
    pub file_dialogs: crate::file_dialogs::FileDialogs,
    pub gltf_file_lease: Option<crate::file_dialogs::DialogPath>,
    /// Actions queued during egui rendering, drained each frame in Update.
    actions: Vec<UiAction>,
    /// Portal webview lifecycle.
    pub portal: PortalState,
    /// Sidebar open state — derived from portal/inspector each frame.
    pub sidebar_open: bool,
    /// Which floating dialog is currently open (at most one).
    pub active_dialog: ActiveDialog,
    /// Live notice queue (OD-1), capped at `NOTICE_QUEUE_CAP`.
    notices: VecDeque<Notice>,
    /// Ring of the most recently shown `Error` notices, newest-pushed
    /// first, capped at `ERROR_HISTORY_CAP` (OD-1; the status-bar error
    /// popover W1-S2 adds reads this).
    error_history: VecDeque<Notice>,
    /// The id of the notice currently pinned open by hover (OD-1: an Error
    /// "holds while hovered"). Exempt from expiry until dismissed.
    held_notice: Option<u64>,
    /// Newest `Error` id the user has dismissed; the status-bar segment
    /// shows only history entries newer than this.
    acknowledged_error_id: Option<u64>,
    /// Persistent status chips (X-05): a condition clears when set to
    /// `None`. A `Vec` rather than a `HashMap` because the chip row renders
    /// in insertion order and there are at most a handful at once.
    conditions: Vec<(&'static str, String)>,
    next_notice_id: u64,
    /// Join dialog's in-flight/denied state (OD-2); owned here so `join.rs`
    /// and `db_results/hierarchy.rs` share one source of truth instead of
    /// each guessing the other's state from `ActiveDialog`.
    pub join_request: DialogRequestState,
    /// glTF Import dialog's in-flight state (D-06); `gltf_import.rs` starts
    /// it, `db_results`' `GltfImported` arm finishes it.
    pub gltf_import_request: DialogRequestState,
}

impl Default for UiManager {
    fn default() -> Self {
        Self {
            file_dialogs: crate::file_dialogs::FileDialogs::default(),
            gltf_file_lease: None,
            actions: Vec::new(),
            portal: PortalState::Closed,
            sidebar_open: true,
            active_dialog: ActiveDialog::None,
            notices: VecDeque::new(),
            error_history: VecDeque::new(),
            held_notice: None,
            acknowledged_error_id: None,
            conditions: Vec::new(),
            next_notice_id: 0,
            join_request: DialogRequestState::default(),
            gltf_import_request: DialogRequestState::default(),
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
        self.replace_dialog(dialog);
    }

    pub fn close_dialog(&mut self) {
        self.replace_dialog(ActiveDialog::None);
    }

    fn replace_dialog(&mut self, dialog: ActiveDialog) {
        self.gltf_file_lease = None;
        self.active_dialog = dialog;
    }

    /// Queues a severity-tagged notice and returns its id. Makes room per
    /// `NOTICE_QUEUE_CAP`'s rule (see its doc comment) when already at cap,
    /// and always mirrors an `Error` into `error_history`.
    fn push_notice(
        &mut self,
        severity: Severity,
        text: String,
        detail: Option<String>,
        now_secs: f64,
    ) -> u64 {
        let id = self.next_notice_id;
        self.next_notice_id += 1;
        let notice = Notice {
            id,
            severity,
            text,
            spawned_at: now_secs,
            detail,
        };
        if severity == Severity::Error {
            self.error_history.push_front(notice.clone());
            self.error_history.truncate(ERROR_HISTORY_CAP);
        }
        let held = self.held_notice;
        self.notices.retain(|queued| {
            held == Some(queued.id)
                || now_secs - queued.spawned_at <= queued.severity.duration_secs()
        });
        if self.notices.len() < NOTICE_QUEUE_CAP || self.evict_for_new_notice(severity) {
            self.notices.push_back(notice);
        }
        id
    }

    /// OD-1 eviction order: the oldest `Info`, else the oldest `Advisory`,
    /// else the oldest `Error` — but never a notice that outranks
    /// `incoming`. Returns whether room was made.
    fn evict_for_new_notice(&mut self, incoming: Severity) -> bool {
        for target in [Severity::Info, Severity::Advisory, Severity::Error] {
            if target > incoming {
                return false;
            }
            if let Some(pos) = self.notices.iter().position(|n| n.severity == target) {
                let _ = self.notices.remove(pos);
                return true;
            }
        }
        false
    }

    /// True while `notice` is still visible: unexpired for its severity, or
    /// held open by hover regardless of elapsed time.
    fn is_live(&self, notice: &Notice, now_secs: f64) -> bool {
        self.held_notice == Some(notice.id)
            || now_secs - notice.spawned_at <= notice.severity.duration_secs()
    }

    /// Fade alpha for a live notice: 0..1 ramp over the first 0.2s, full
    /// opacity while held or mid-lifetime, ramp down over the last 0.5s.
    fn alpha_for(&self, notice: &Notice, now_secs: f64) -> f32 {
        if self.held_notice == Some(notice.id) {
            return 1.0;
        }
        let duration = notice.severity.duration_secs();
        let elapsed = now_secs - notice.spawned_at;
        let alpha = if elapsed < 0.2 {
            (elapsed / 0.2) as f32
        } else if elapsed > duration - 0.5 {
            ((duration - elapsed) / 0.5) as f32
        } else {
            1.0
        };
        alpha.clamp(0.0, 1.0)
    }

    /// Show a brief Info notice (unchanged signature — frozen contract: the
    /// W1 `panels/mod.rs::render_toast` call site still compiles against
    /// `show_toast`/`active_toast` unmodified; W3-S1 replaces the renderer).
    pub fn show_toast(&mut self, msg: impl Into<String>, now_secs: f64) {
        self.push_notice(Severity::Info, msg.into(), None, now_secs);
    }

    /// Show an Advisory notice (OD-1: 4s, non-blocking guidance — e.g. "no
    /// selection", not a failure).
    pub fn show_advisory(&mut self, msg: impl Into<String>, now_secs: f64) {
        self.push_notice(Severity::Advisory, msg.into(), None, now_secs);
    }

    /// Show an Error notice (OD-1: 7s, holds while hovered, mirrored into
    /// `error_history`).
    pub fn show_error(&mut self, msg: impl Into<String>, now_secs: f64) {
        self.push_notice(Severity::Error, msg.into(), None, now_secs);
    }

    /// Show a `DbResult::Error`-style failure: the humanized copy on screen,
    /// the raw string kept as the history entry's `detail`.
    pub fn show_db_error(&mut self, raw: &str, ctx: &crate::notice::DenialContext, now_secs: f64) {
        let text = crate::notice::humanize_db_error(raw, ctx);
        self.push_notice(Severity::Error, text, Some(raw.to_string()), now_secs);
    }

    /// Returns the highest-priority live notice and its fade alpha. `Error`
    /// outranks `Advisory` outranks `Info` (ties break on the most recent
    /// `spawned_at`), so a later `Info` can never visually bump an
    /// unexpired `Error` off the single on-screen slot.
    pub fn active_notice(&self, now_secs: f64) -> Option<(&Notice, f32)> {
        let best = self
            .notices
            .iter()
            .filter(|n| self.is_live(n, now_secs))
            .max_by(|a, b| {
                a.severity.cmp(&b.severity).then_with(|| {
                    a.spawned_at
                        .partial_cmp(&b.spawned_at)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            })?;
        let alpha = self.alpha_for(best, now_secs);
        Some((best, alpha))
    }

    /// Returns (message, alpha 0.0..1.0) for the active notice (unchanged
    /// signature — see `show_toast`'s doc comment).
    pub fn active_toast(&self, now_secs: f64) -> Option<(&str, f32)> {
        let (notice, alpha) = self.active_notice(now_secs)?;
        Some((notice.text.as_str(), alpha))
    }

    /// Pins a notice open past its normal expiry while the pointer hovers
    /// it (OD-1). A no-op for an id that is no longer queued.
    pub fn hold_notice(&mut self, id: u64, _now_secs: f64) {
        if self.notices.iter().any(|n| n.id == id) {
            self.held_notice = Some(id);
        }
    }

    /// Dismisses a notice immediately (the renderer's ✕ or the status bar's
    /// Dismiss) and releases any hold on it. Dismissing an `Error`
    /// acknowledges it and every older one, so the status-bar segment clears.
    pub fn dismiss_notice(&mut self, id: u64) {
        self.notices.retain(|n| n.id != id);
        if self.held_notice == Some(id) {
            self.held_notice = None;
        }
        if self.error_history.iter().any(|n| n.id == id) {
            self.acknowledged_error_id = self.acknowledged_error_id.max(Some(id));
        }
    }

    /// The `Error` history ring, newest-pushed first, capped at
    /// `ERROR_HISTORY_CAP` (OD-1).
    pub fn error_history(&self) -> impl Iterator<Item = &Notice> {
        self.error_history.iter()
    }

    /// The newest `Error` not yet dismissed — what the status-bar segment
    /// shows (it adds its own age bound).
    pub fn latest_unacknowledged_error(&self) -> Option<&Notice> {
        self.error_history
            .front()
            .filter(|notice| Some(notice.id) > self.acknowledged_error_id)
    }

    /// Sets (or clears, on `None`) a persistent status chip (X-05's
    /// mesh-budget indicator and similar ongoing conditions). Updating an
    /// existing key in place preserves its row position.
    pub fn set_condition(&mut self, key: &'static str, text: Option<String>) {
        match text {
            Some(text) => {
                if let Some(entry) = self.conditions.iter_mut().find(|(k, _)| *k == key) {
                    entry.1 = text;
                } else {
                    self.conditions.push((key, text));
                }
            }
            None => self.conditions.retain(|(k, _)| *k != key),
        }
    }

    /// Iterates the active persistent condition chips, in the order they
    /// were first set.
    pub fn conditions(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.conditions.iter().map(|(k, v)| (*k, v.as_str()))
    }
}

/// Sends a `DbCommand`, toasting an Error and warning instead of silently
/// dropping it on a closed channel (AGENTS.md anti-pattern 6: no bare
/// `.send(..).ok()`). `what` names the attempted action in the failure
/// notice ("save the URL") and the log line. Returns whether the send
/// succeeded, for a caller that has more to do only on success.
pub(crate) fn send_or_toast(
    tx: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    cmd: fe_runtime::messages::DbCommand,
    ui_mgr: &mut UiManager,
    now_secs: f64,
    what: &str,
) -> bool {
    if tx.send(cmd).is_err() {
        bevy::log::warn!("send_or_toast: channel closed — {what} not sent");
        ui_mgr.show_error(format!("Couldn't {what} — not connected"), now_secs);
        false
    } else {
        true
    }
}

/// `UiAction::OpenPortal` (P-01): opens the portal for the selected node, or
/// explains why not (Advisory for no selection, Error for a bad URL).
/// Returns the URL the browser should navigate to.
fn open_portal(
    node_mgr: &crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
    url: &str,
    now_secs: f64,
) -> Option<url::Url> {
    match portal::compute_open_portal(node_mgr, url) {
        portal::OpenPortalOutcome::Navigate(new_state, parsed) => {
            bevy::log::info!("Portal: forwarding Navigate for URL: {parsed}");
            ui_mgr.portal = new_state;
            Some(parsed)
        }
        portal::OpenPortalOutcome::NoSelection => {
            ui_mgr.show_advisory("No node selected", now_secs);
            None
        }
        portal::OpenPortalOutcome::InvalidUrl(e) => {
            bevy::log::warn!("UiAction::OpenPortal invalid URL: {e}");
            ui_mgr.show_error("Not a valid URL — include https://", now_secs);
            None
        }
    }
}

/// `UiAction::PathSelectTrack` (D-12, "Edit Path"): activates Pen —
/// `ToolState::activate` does the one-shot Options reveal — loads the path,
/// and names where it opened.
fn open_path_in_options(
    tool_state: &mut crate::plugin::ToolState,
    right_sidebar: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    db_sender: &fe_runtime::app::DbCommandSender,
    path_state: &mut crate::gis::PathEditorState,
    ui_mgr: &mut UiManager,
    track_node_id: String,
    now_secs: f64,
) {
    tool_state.activate(crate::panels::toolbar::Tool::Pen, right_sidebar);
    path::select_track(db_sender, path_state, track_node_id);
    ui_mgr.show_toast("Path open in Options", now_secs);
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
    undo:ResMut<'w,crate::undo::UndoState>,
    tool_panel: ResMut<'w, crate::panels::tool_panel::ToolPanelState>,
    proposal_state: ResMut<'w, ProposalEditState>,
    stamp_state: ResMut<'w, asset::StampInteractionState>,
    sculpt_state: ResMut<'w, terrain_proposal::SculptToolState>,
    /// T3: region_id↔node_id bookkeeping for earthwork endpoint rows (D-A8).
    earthwork_map: ResMut<'w, terrain_proposal::EarthworkNodeMap>,
    /// HS-P6-CALLOUT: pending-stash + node bookkeeping for dimension callout
    /// endpoint rows, mirroring `earthwork_map`.
    callout_map: ResMut<'w, dimension_callout::DimensionCalloutMap>,
    /// D11: the right-sidebar surface `UiAction::RevealSection` addresses.
    /// Bundled here (rather than as a 16th top-level param) so this system
    /// keeps headroom under Bevy's `SystemParam` tuple ceiling; the egui pass
    /// holds the same resource in a different schedule, so there is no
    /// same-frame access conflict.
    right_sidebar: ResMut<'w, crate::ui_shell::right_sidebar::RightSidebarState>,
    local_role: Res<'w, crate::plugin::LocalUserRole>,
    /// `UiAction::SetNodeView`'s optimistic session target (bundled for the
    /// same param-ceiling reason as `right_sidebar`).
    visibility: ResMut<'w, crate::visibility::VisibilityState>,
    /// `UiAction::SetAppOverlay`'s target (ruler overlay rows, VIS-PH5).
    app_settings: ResMut<'w, crate::settings::AppSettings>,
    /// Session-only `UiAction::PetalSetRenderOverride` target.
    render_override: ResMut<'w, crate::settings::PetalRenderOverride>,
    /// `UiAction::HexonStartDownload`/`HexonCancelDownload`'s target (S4:
    /// moved out of `ActiveDialog::HexonManager` — bundled here for the same
    /// param-ceiling reason as `right_sidebar`).
    downloads: ResMut<'w, crate::terrain_map::TilesetDownloads>,
    /// D-12: `UiAction::PathSelectTrack`'s Pen activation. `ToolState` is the
    /// SINGLE writer of the active tool (plugin.rs's `activate` doc comment)
    /// — bundled here for the same param-ceiling reason as `right_sidebar`,
    /// which `activate` also needs for its one-shot Options reveal.
    active_tool_state: ResMut<'w, crate::plugin::ToolState>,
}

/// Drains all UiActions queued during the egui pass and processes them.
/// Replaces: forward_webview_open_request, drain_portal_panel_actions, handle_url_save.
pub(crate) fn process_ui_actions(
    mut ui_mgr: ResMut<UiManager>,
    mut inspector: ResMut<InspectorFormState>,
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
    time: Res<crate::notice::NoticeClock>,
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
        mut undo,
        mut tool_panel,
        mut proposal_state,
        mut stamp_state,
        mut sculpt_state,
        mut earthwork_map,
        mut callout_map,
        mut right_sidebar,
        local_role,
        mut visibility,
        mut app_settings,
        mut render_override,
        mut downloads,
        mut active_tool_state,
    } = tool_state;
    // Fold pen-tool actions queued by `render_tool_panel` into the main queue
    // (the Tools panel has no `ui_mgr` handle — see panels/tool_panel.rs).
    for pen_action in tool_panel.drain_pending() {
        ui_mgr.push_action(pen_action);
    }
    // `NoticeClock` is the clock egui renders notices against.
    let now_secs = time.elapsed_secs_f64();

    // Auto-close portal when the selected entity changes or is deselected.
    if portal::should_auto_close(&ui_mgr.portal, node_mgr.selected_entity()) {
        ui_mgr.portal = PortalState::Closed;
        browser_commands.write(BrowserCommand::Close);
    }

    let actions = ui_mgr.drain_actions();
    for action in actions {
        match action {
            UiAction::OpenPortal { url } => {
                if let Some(parsed) = open_portal(&node_mgr, &mut ui_mgr, &url, now_secs) {
                    browser_commands.write(BrowserCommand::Navigate { url: parsed });
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
                // Every outcome is surfaced to the user (toast) — no silent
                // drops; blocked URLs never reach the DB (see AGENTS.md §portal).
                match portal::compute_save_url(&node_mgr, &inspector) {
                    portal::SaveUrlOutcome::Persist { node_id, url } => {
                        verse_mgr.update_node_url(&node_id, url.clone());
                        if send_or_toast(
                            &db_sender.0,
                            fe_runtime::messages::DbCommand::UpdateNodeUrl { node_id, url },
                            &mut ui_mgr,
                            now_secs,
                            "save the URL",
                        ) {
                            ui_mgr.show_toast("URL saved", now_secs);
                        }
                    }
                    portal::SaveUrlOutcome::Blocked { reason } => {
                        bevy::log::warn!("UiAction::SaveUrl rejected: {reason}");
                        ui_mgr.show_error(format!("URL not saved — {reason}"), now_secs);
                    }
                    portal::SaveUrlOutcome::NoSelection => {
                        ui_mgr.show_advisory("No node selected", now_secs);
                    }
                }
            }
            UiAction::ApplyNodeTransform => {
                if undo.is_pending() || node_mgr.gesture_active {ui_mgr.show_advisory("Finish the gesture or wait for the previous edit to save",now_secs);continue;}
                if let Err(reason) =
                    transform::apply(&inspector, &mut node_mgr, &mut transform_query)
                {
                    ui_mgr.show_error(format!("Transform not applied — {reason}"), now_secs);
                }
            }
            UiAction::CreateNodeAt { position } => match nav.active_petal_id.as_deref() {
                Some(petal_id) => {
                    node::create_at(&db_sender, petal_id.to_string(), position);
                }
                None => {
                    ui_mgr.show_advisory("Navigate to a petal first", now_secs);
                }
            },
            UiAction::SubmitQuery { sql, scope } => {
                query::submit(&db_sender, &mut inspector, sql, scope);
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
                hexon::start_download(&mut downloads, sync_sender.as_deref(), id);
            }
            UiAction::HexonCancelDownload(id) => {
                hexon::cancel_download(&mut downloads, sync_sender.as_deref(), id);
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
            UiAction::HexonInstallFromRegistry(uri) => {
                hexon::install_from_registry(&mut hexon_ops, uri);
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
                if let Err(reason) = crate::dialogs::requirements_state::save(&mut ui_mgr, &nav, &db_sender.0, &petal_id, manifest, time.elapsed_secs_f64()) {
                    ui_mgr.show_error(reason, time.elapsed_secs_f64());
                }
            }
            UiAction::PetalSetRenderOverride {
                petal_id,
                render_distance,
            } => {
                hexon::set_render_override(&mut render_override, petal_id, render_distance);
            }
            UiAction::PetalManifestOpen {
                petal_id,
                petal_name,
            } => {
                hexon::manifest_open(&mut ui_mgr, petal_id, petal_name, nav.active_scope());
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
            UiAction::PathSelectTrack { track_node_id } => open_path_in_options(
                &mut active_tool_state,
                &mut right_sidebar,
                &db_sender,
                &mut path_state,
                &mut ui_mgr,
                track_node_id,
                now_secs,
            ),
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
                // Synchronous close (finding #17 fix,
                // `ui_semantics_unification_20260808`): closing the editing
                // session must not wait on the DB round trip — the DB-result
                // path also closes it, but only after the round trip, which
                // leaves a same-frame window where `editing_track_id` still
                // points at a track that's mid-delete.
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
            UiAction::PathMovePointFromGesture {track_node_id,index,position,origin}=>{
                if !origin.matches(&path_state,&nav){ui_mgr.show_advisory("Path changed during the gesture; move skipped",now_secs);continue;}
                queue_checked_point(&mut undo,&nav,&verse_mgr,&mut path_ops,&path_state,track_node_id,index,position,&mut ui_mgr,now_secs);
            }
            UiAction::PathMovePoint {track_node_id,index,position}=>{
                queue_checked_point(&mut undo,&nav,&verse_mgr,&mut path_ops,&path_state,track_node_id,index,position,&mut ui_mgr,now_secs);
            }
            UiAction::PathMovePointUntracked {track_node_id,index,position}=>{
                if undo.is_pending(){ui_mgr.show_advisory("Saving the previous edit; wait for confirmation",now_secs);continue;}
                undo.invalidate_redo();path::move_point(&mut path_ops,&mut path_state,track_node_id,index,position);
            }
            UiAction::PathTransformPoints {
                track_node_id,
                points,
            } => {
                if undo.is_pending(){ui_mgr.show_advisory("Saving the previous edit; wait for confirmation",now_secs);continue;}
                undo.invalidate_redo();
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
                closed,
                fill_color,
            } => {
                path::set_style(
                    &db_sender,
                    track_node_id,
                    color,
                    width,
                    visible,
                    closed,
                    fill_color,
                );
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
            UiAction::TerrainProposalSelect { id } => {
                proposal_state.selected = Some(id);
            }
            UiAction::OpenTerrainReport { petal_id, id } => {
                if !crate::panels::volume_report_panel::open_terrain_report(
                    &petal_id,
                    &id,
                    &nav,
                    &petal_map,
                    &mut proposal_state,
                    &mut right_sidebar,
                    local_role.role.is_some_and(|role| role.can_view()),
                ) {
                    ui_mgr.show_advisory(
                        "Report target unavailable in the active Petal or Viewer access required",
                        now_secs,
                    );
                }
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
            // HS-P6-CALLOUT (dimension_callout.rs):
            UiAction::SaveMeasureAsCallout {
                petal_id,
                name,
                mode,
                anchors,
                closed,
            } => {
                dimension_callout::save_measure_as_callout(
                    &db_sender,
                    &mut callout_map,
                    petal_id,
                    name,
                    &mode,
                    anchors,
                    closed,
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
            UiAction::RevealSection { slug } => {
                // D11: the whole handler is `reveal_slug` — the slug table and
                // its round-trip live with the enum in `right_sidebar.rs`.
                if !right_sidebar.reveal_slug(&slug) {
                    bevy::log::warn!("UiAction::RevealSection: unknown surface slug {slug:?}");
                }
            }
            UiAction::SetNodeView { node_id, state } => {
                crate::visibility::cache::set_node_view(
                    &db_sender,
                    &mut visibility,
                    node_id,
                    state,
                );
            }
            UiAction::SetGroupRegistry { petal_id, groups } => {
                crate::visibility::groups::set_group_registry(
                    &db_sender,
                    &mut visibility,
                    petal_id,
                    groups,
                );
            }
            UiAction::SetScopeHidden { level, id, hidden } => {
                crate::visibility::groups::set_scope_hidden(
                    &db_sender,
                    &mut visibility,
                    level,
                    id,
                    hidden,
                );
            }
            UiAction::SetNodeGroups { node_id, groups } => {
                crate::visibility::groups::set_node_groups(
                    &db_sender,
                    &mut visibility,
                    node_id,
                    groups,
                );
            }
            UiAction::SetAppOverlay { key, visible } => {
                if !crate::gis::apply_app_overlay(&mut app_settings, &key, visible) {
                    bevy::log::warn!("UiAction::SetAppOverlay: {key:?} is not an app overlay");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- W1-S1: notice queue (OD-1) ---------------------------------------

    #[test]
    fn notice_queue_error_survives_later_info() {
        // (severity, spawn time, text) pushed in order, then queried at
        // `query_at` — asserts which text is on-screen at that instant.
        struct Case {
            pushes: Vec<(Severity, f64, &'static str)>,
            query_at: f64,
            expected: &'static str,
        }
        let cases: Vec<Case> = vec![
            // Both live (Error spawned first): severity alone decides.
            Case {
                pushes: vec![
                    (Severity::Error, 0.0, "boom"),
                    (Severity::Info, 1.0, "saved"),
                ],
                query_at: 1.5,
                expected: "boom",
            },
            // The newer Info (2.5s lifetime) has already faded by t=5, but
            // the Error (7s lifetime) hasn't — the acceptance case this test
            // exists for: an Info arriving after an Error never hides it
            // early just by being more recent.
            Case {
                pushes: vec![
                    (Severity::Error, 0.0, "boom"),
                    (Severity::Info, 1.0, "saved"),
                ],
                query_at: 5.0,
                expected: "boom",
            },
            // Once the Error itself expires (t=7.5 > 7s), the still-live
            // Info takes over — expiry is still respected per severity.
            Case {
                pushes: vec![
                    (Severity::Error, 0.0, "boom"),
                    (Severity::Info, 6.0, "saved"),
                ],
                query_at: 7.5,
                expected: "saved",
            },
        ];
        for case in &cases {
            let mut mgr = UiManager::default();
            for (severity, spawn_at, text) in &case.pushes {
                match severity {
                    Severity::Info => mgr.show_toast(*text, *spawn_at),
                    Severity::Advisory => mgr.show_advisory(*text, *spawn_at),
                    Severity::Error => mgr.show_error(*text, *spawn_at),
                }
            }
            let (msg, _alpha) = mgr
                .active_toast(case.query_at)
                .unwrap_or_else(|| panic!("expected a live notice at t={}", case.query_at));
            assert_eq!(msg, case.expected, "query_at={}", case.query_at);
        }
    }

    #[test]
    fn notice_queue_caps_at_four_evicting_oldest_info_first() {
        let mut mgr = UiManager::default();
        mgr.show_advisory("adv-old", 0.0);
        mgr.show_toast("info-old", 0.0);
        mgr.show_toast("info-new", 0.0);
        mgr.show_advisory("adv-new", 0.0);
        // Queue is now at NOTICE_QUEUE_CAP (4). This 5th push must evict the
        // oldest Info ("info-old"), NOT the oldest overall ("adv-old") —
        // OD-1's eviction order.
        mgr.show_error("err", 0.0);

        let mut seen = Vec::new();
        while let Some((notice, _)) = mgr.active_notice(0.0) {
            seen.push(notice.text.clone());
            let id = notice.id;
            mgr.dismiss_notice(id);
        }
        assert!(
            !seen.contains(&"info-old".to_string()),
            "oldest Info must be evicted first, not retained: {seen:?}"
        );
        assert!(
            seen.contains(&"adv-old".to_string()),
            "adv-old predates info-old but is Advisory, so it must survive: {seen:?}"
        );
        assert!(seen.contains(&"err".to_string()));
    }

    /// A full queue of live Errors is never thinned by a later Info (the Info
    /// is dropped); once they expire, the same Info push is queued normally.
    #[test]
    fn notice_queue_never_evicts_a_live_error_for_an_info() {
        for (info_at, info_queued) in [(1.0, false), (8.0, true)] {
            let mut mgr = UiManager::default();
            for text in ["e1", "e2", "e3", "e4"] {
                mgr.show_error(text, 0.0);
            }
            mgr.show_toast("saved", info_at);
            let mut seen = Vec::new();
            while let Some((notice, _)) = mgr.active_notice(info_at) {
                seen.push(notice.text.clone());
                let id = notice.id;
                mgr.dismiss_notice(id);
            }
            let expected: Vec<&str> = if info_queued {
                vec!["saved"]
            } else {
                vec!["e4", "e3", "e2", "e1"]
            };
            assert_eq!(seen, expected, "info_at={info_at}");
        }
    }

    /// The status-bar segment shows the newest Error until it is dismissed;
    /// a later Error brings it back. A DB error keeps its raw text.
    #[test]
    fn dismissed_error_clears_the_status_segment_until_the_next_one() {
        let mut mgr = UiManager::default();
        let ctx = crate::notice::DenialContext {
            role_label: "Viewer",
            verse_name: None,
        };
        mgr.show_db_error("Load hierarchy failed: disk full", &ctx, 0.0);
        let first = mgr.latest_unacknowledged_error().expect("segment shown");
        assert_eq!(
            first.detail.as_deref(),
            Some("Load hierarchy failed: disk full")
        );
        let first_id = first.id;

        mgr.dismiss_notice(first_id);
        assert!(mgr.latest_unacknowledged_error().is_none());
        assert_eq!(mgr.error_history().count(), 1, "dismiss keeps the history");

        mgr.show_error("Transform not applied — locked", 1.0);
        assert_eq!(
            mgr.latest_unacknowledged_error().map(|n| n.text.as_str()),
            Some("Transform not applied — locked")
        );
    }

    #[test]
    fn held_error_notice_outlives_its_duration_until_dismissed() {
        let mut mgr = UiManager::default();
        mgr.show_error("boom", 0.0);
        let id = mgr.active_notice(0.0).expect("just shown").0.id;
        mgr.hold_notice(id, 0.0);
        // Past the normal 7s Error duration, the held notice is still live.
        assert!(
            mgr.active_notice(10.0).is_some(),
            "a held Error must survive past its normal duration"
        );
        mgr.dismiss_notice(id);
        assert!(
            mgr.active_notice(10.0).is_none(),
            "dismiss must release the hold, not just re-arm the timer"
        );
    }

    // ---- W1-S1: process_ui_actions handler arms ---------------------------
    //
    // `process_ui_actions` takes ~15 `SystemParam`s (several without
    // `Default`), so these drive the arms' extracted bodies — `open_portal`
    // and `open_path_in_options`, which the dispatcher calls verbatim —
    // with the real `UiManager`/`NodeManager`/`ToolState`/`PathEditorState`.

    #[test]
    fn open_portal_invalid_url_shows_error_notice() {
        let mgr = crate::node_manager::NodeManager::default();
        let mut ui_mgr = UiManager::default();
        assert!(open_portal(&mgr, &mut ui_mgr, "not a url", 0.0).is_none());
        let (notice, _alpha) = ui_mgr.active_notice(0.0).expect("error notice shown");
        assert_eq!(notice.text, "Not a valid URL — include https://");
        assert_eq!(notice.severity, Severity::Error);
    }

    #[test]
    fn edit_path_action_activates_pen_and_reveals_options() {
        use crate::plugin::ToolState;
        use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};

        let (tx, _rx) = crossbeam::channel::bounded(8);
        let db_sender = fe_runtime::app::DbCommandSender(tx);
        let mut tool = ToolState::default();
        let mut right = RightSidebarState::default();
        let mut ui_mgr = UiManager::default();
        let mut path_state = crate::gis::PathEditorState::default();

        open_path_in_options(
            &mut tool,
            &mut right,
            &db_sender,
            &mut path_state,
            &mut ui_mgr,
            "track-1".to_string(),
            0.0,
        );

        assert_eq!(tool.active_tool, crate::panels::toolbar::Tool::Pen);
        assert_eq!(right.requested, Some(RightSidebarSection::Options));
        assert_eq!(path_state.editing_track_id.as_deref(), Some("track-1"));
        let (msg, _alpha) = ui_mgr.active_toast(0.0).expect("toast shown");
        assert_eq!(msg, "Path open in Options");
    }

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
        let root = std::env::temp_dir().join(format!(
            "fe-dialog-replace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("model.glb");
        std::fs::write(&source, b"fixture").unwrap();
        let provider =
            std::sync::Arc::new(crate::file_dialogs::QaFileDialogs::new(root.clone()).unwrap());
        provider.set_listener("127.0.0.1:8765".parse().unwrap());
        mgr.file_dialogs = crate::file_dialogs::FileDialogs(provider);
        let selected = mgr
            .file_dialogs
            .lease_path(&source, fe_runtime::qa_debug::DialogKind::Open, Some("v"))
            .unwrap();
        let in_flight_read = selected.read_lease().unwrap();
        mgr.gltf_file_lease = Some(selected);
        mgr.open_dialog(ActiveDialog::PeerDebug);
        assert!(
            mgr.gltf_file_lease.is_none(),
            "dialog replacement releases its abandoned selection"
        );
        assert_eq!(
            std::sync::Arc::strong_count(&in_flight_read),
            1,
            "only the actual DB read retains its own complete lease"
        );
        drop(in_flight_read);
        std::fs::rename(&source, root.join("released.glb")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
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
        mgr.open_dialog(crate::dialogs::entity_details::dialog(
            crate::dialogs::EntitySettingsType::Verse,
            "v1",
            "Test Verse",
            "v1",
            None,
        ));
        assert!(mgr.any_dialog_open());
        assert!(matches!(
            mgr.active_dialog,
            ActiveDialog::EntitySettings { .. }
        ));
        let (tx, rx) = crossbeam::channel::bounded(8);
        let queue = |manager: &mut UiManager, now| {
            crate::dialogs::entity_details::request_if_needed(manager, &tx, now);
            match rx.try_recv().unwrap() {
                DbCommand::GetEntityDetails {
                    entity_type,
                    entity_id,
                    request_id,
                } => (entity_type, entity_id, request_id),
                other => panic!("expected target details read, got {other:?}"),
            }
        };
        let (kind, id, request_id) = queue(&mut mgr, 1.0);
        assert_eq!(kind, fe_runtime::messages::EntityType::Verse);
        assert_eq!(id, "v1");
        let mut stored = fe_runtime::messages::EntityDetails {
            entity_type: kind,
            entity_id: id.clone(),
            verse_id: "v1".into(),
            fractal_id: None,
            scope: "VERSE#v1".into(),
            name: "Persisted verse".into(),
            default_access: Some("none".into()),
            description: None,
            role: "manager".into(),
        };
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            request_id + 1,
            &Ok(stored.clone()),
            1.1,
        );
        if let ActiveDialog::EntitySettings {
            name_buf,
            default_access_buf,
            details,
            ..
        } = &mgr.active_dialog
        {
            assert!(
                name_buf.is_empty() && default_access_buf.is_none() && details.loaded.is_none(),
                "a wrong nonce cannot hydrate guessed defaults"
            );
        }
        let mut foreign = stored.clone();
        foreign.verse_id = "other".into();
        crate::dialogs::entity_details::apply(&mut mgr, kind, &id, request_id, &Ok(foreign), 1.1);
        if let ActiveDialog::EntitySettings { details, .. } = &mgr.active_dialog {
            assert!(details.error.is_some() && details.loaded.is_none());
        }
        crate::dialogs::entity_details::refresh_target(&mut mgr, kind, &id);
        let (_, _, next) = queue(&mut mgr, 1.2);
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            request_id,
            &Ok(stored.clone()),
            1.25,
        );
        crate::dialogs::entity_details::apply(&mut mgr, kind, &id, next, &Ok(stored.clone()), 1.3);
        if let ActiveDialog::EntitySettings {
            name_buf,
            default_access_buf,
            details,
            ..
        } = &mut mgr.active_dialog
        {
            assert_eq!(name_buf.as_str(), "Persisted verse");
            assert_eq!(default_access_buf.as_deref(), Some("none"));
            assert!(details.can_write());
            *name_buf = "My unsaved draft".into();
        }
        crate::dialogs::entity_details::refresh_target(&mut mgr, kind, &id);
        let (_, _, refresh) = queue(&mut mgr, 2.0);
        stored.name = "Changed elsewhere".into();
        stored.default_access = Some("viewer".into());
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            refresh,
            &Ok(stored.clone()),
            2.1,
        );
        if let ActiveDialog::EntitySettings {
            name_buf,
            default_access_buf,
            description_buf,
            details,
            ..
        } = &mut mgr.active_dialog
        {
            assert_eq!(name_buf.as_str(), "My unsaved draft");
            assert_eq!(default_access_buf.as_deref(), Some("viewer"));
            assert!(details.conflict && !details.can_write());
            details.restore(name_buf, default_access_buf, description_buf);
            assert_eq!(name_buf.as_str(), "Changed elsewhere");
            assert!(details.can_write());
        }
        crate::dialogs::entity_details::refresh_target(&mut mgr, kind, &id);
        let (_, _, failed) = queue(&mut mgr, 3.0);
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            failed,
            &Err("Viewer access was revoked".into()),
            3.1,
        );
        if let ActiveDialog::EntitySettings {
            name_buf, details, ..
        } = &mgr.active_dialog
        {
            assert_eq!(name_buf.as_str(), "Changed elsewhere");
            assert!(details.error.is_some() && !details.can_write());
        }
        mgr.open_dialog(crate::dialogs::entity_details::dialog(
            crate::dialogs::EntitySettingsType::Fractal,
            "f1",
            "Cached label",
            "v1",
            None,
        ));
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            failed,
            &Ok(stored.clone()),
            4.0,
        );
        let (kind, id, fractal_request) = queue(&mut mgr, 5.0);
        stored = fe_runtime::messages::EntityDetails {
            entity_type: kind,
            entity_id: id.clone(),
            verse_id: "v1".into(),
            fractal_id: None,
            scope: "VERSE#v1-FRACTAL#f1".into(),
            name: "Stored fractal".into(),
            default_access: None,
            description: Some("Stored description with \"quotes\" and\nnewlines".into()),
            role: "manager".into(),
        };
        crate::dialogs::entity_details::apply(
            &mut mgr,
            kind,
            &id,
            fractal_request,
            &Ok(stored.clone()),
            5.1,
        );
        if let ActiveDialog::EntitySettings {
            description_buf,
            details,
            ..
        } = &mut mgr.active_dialog
        {
            assert_eq!(description_buf.as_deref(), stored.description.as_deref());
            assert!(details.can_write());
            *description_buf = Some("Retain this unsaved description".into());
        }
        crate::dialogs::entity_details::refresh_target(&mut mgr, kind, &id);
        let (_, _, timed_out) = queue(&mut mgr, 6.0);
        crate::dialogs::entity_details::request_if_needed(&mut mgr, &tx, 37.0);
        crate::dialogs::entity_details::apply(&mut mgr, kind, &id, timed_out, &Ok(stored), 37.1);
        if let ActiveDialog::EntitySettings {
            description_buf,
            details,
            ..
        } = &mgr.active_dialog
        {
            assert_eq!(
                description_buf.as_deref(),
                Some("Retain this unsaved description")
            );
            assert!(details.error.is_some() && !details.can_write());
        }
        assert!(
            rx.try_recv().is_err(),
            "a timed-out request never auto-retries indefinitely"
        );
        mgr.close_dialog();
        assert!(!mgr.any_dialog_open());

        use fe_runtime::entity_access::{EntityAccessAction as Access,EntityAccessSnapshot,EntityMintedToken};
        mgr.open_dialog(crate::dialogs::entity_details::dialog(crate::dialogs::EntitySettingsType::Verse,"access-v","Access verse","access-v",None));
        let (kind,id,nonce)=queue(&mut mgr,40.0);
        let details=fe_runtime::messages::EntityDetails {entity_type:kind,entity_id:id.clone(),verse_id:id.clone(),fractal_id:None,scope:"VERSE#access-v".into(),name:"Access verse".into(),default_access:Some("none".into()),description:None,role:"owner".into()};
        crate::dialogs::entity_details::apply(&mut mgr,kind,&id,nonce,&Ok(details),40.1);
        let mut peers=fe_runtime::PeerRegistry::default();peers.insert(fe_runtime::PeerEntry::new("did:key:known-peer".into()));
        if let ActiveDialog::EntitySettings {active_tab,access,..}=&mut mgr.active_dialog {
            crate::dialogs::entity_access::select_tab(active_tab,access,crate::dialogs::SettingsTab::Access);
        }
        crate::dialogs::entity_access::service(&mut mgr,&tx,Some(&peers));
        let DbCommand::EntityAccess {target,request_id:read_id,action:Access::Load {peer_dids}}=rx.try_recv().unwrap() else{panic!("Expected target-bound known-peer read")};
        assert_eq!(peer_dids,vec!["did:key:known-peer"]);
        let snapshot=EntityAccessSnapshot {local_did:"did:key:owner".into(),local_role:"owner".into(),roles:vec![("did:key:known-peer".into(),"viewer".into())],tokens:vec![],tokens_truncated:false,notice:None,invite_link:None,minted:None};
        crate::dialogs::entity_access::apply(&mut mgr,&target,"foreign-nonce",&Ok(snapshot.clone()));
        if let ActiveDialog::EntitySettings {access,..}=&mgr.active_dialog {assert!(access.snapshot.is_none()&&access.waiting());}
        crate::dialogs::entity_access::apply(&mut mgr,&target,&read_id,&Ok(snapshot.clone()));
        crate::dialogs::entity_access::submit(&mut mgr,Access::Revoke {peer_did:"did:key:known-peer".into()},&tx);
        let DbCommand::EntityAccess {request_id:change_id,..}=rx.try_recv().unwrap() else{unreachable!()};
        crate::dialogs::entity_access::apply(&mut mgr,&target,&change_id,&Err("unconfirmed write".into()));
        crate::dialogs::entity_access::submit(&mut mgr,Access::Revoke {peer_did:"did:key:known-peer".into()},&tx);
        assert!(rx.try_recv().is_err(),"No mutation retry before explicit authoritative read");
        if let ActiveDialog::EntitySettings {access,..}=&mut mgr.active_dialog {access.refresh();}
        crate::dialogs::entity_access::service(&mut mgr,&tx,Some(&peers));
        let DbCommand::EntityAccess {request_id:refresh_id,action:Access::Load {..},..}=rx.try_recv().unwrap() else{panic!("Only a read may retry")};
        crate::dialogs::entity_access::apply(&mut mgr,&target,&refresh_id,&Ok(snapshot.clone()));
        if let ActiveDialog::EntitySettings {active_tab,access,..}=&mut mgr.active_dialog {
            crate::dialogs::entity_access::select_tab(active_tab,access,crate::dialogs::SettingsTab::ApiAccess);
        }
        crate::dialogs::entity_access::service(&mut mgr,&tx,Some(&peers));
        let DbCommand::EntityAccess {request_id:api_read,..}=rx.try_recv().unwrap() else{unreachable!()};
        crate::dialogs::entity_access::apply(&mut mgr,&target,&api_read,&Ok(snapshot.clone()));
        crate::dialogs::entity_access::submit(&mut mgr,Access::MintToken {max_role:"viewer".into(),ttl_hours:1,client_did:None},&tx);
        let DbCommand::EntityAccess {request_id:mint_id,..}=rx.try_recv().unwrap() else{unreachable!()};
        if let ActiveDialog::EntitySettings {active_tab,..}=&mut mgr.active_dialog {*active_tab=crate::dialogs::SettingsTab::General;}
        let mut minted=snapshot.clone();minted.minted=Some(EntityMintedToken {token:"fake.secret.for-lifecycle-proof".into(),client:"did:key:client".into(),expires_at:"2027-01-01T00:00:00Z".into()});
        crate::dialogs::entity_access::apply(&mut mgr,&target,&mint_id,&Ok(minted));
        if let ActiveDialog::EntitySettings {access,..}=&mgr.active_dialog {assert!(access.snapshot.as_ref().is_none_or(|snapshot|snapshot.minted.is_none()),"Programmatic tab leave cannot retain a late secret");}
        if let ActiveDialog::EntitySettings {active_tab,access,..}=&mut mgr.active_dialog {
            crate::dialogs::entity_access::select_tab(active_tab,access,crate::dialogs::SettingsTab::ApiAccess);
        }
        crate::dialogs::entity_access::service(&mut mgr,&tx,Some(&peers));
        let DbCommand::EntityAccess {request_id:next_read,..}=rx.try_recv().unwrap() else{unreachable!()};
        crate::dialogs::entity_access::apply(&mut mgr,&target,&next_read,&Ok(snapshot.clone()));
        crate::dialogs::entity_access::submit(&mut mgr,Access::MintToken {max_role:"viewer".into(),ttl_hours:1,client_did:None},&tx);
        let DbCommand::EntityAccess {request_id:next_mint,..}=rx.try_recv().unwrap() else{unreachable!()};
        let mut minted=snapshot.clone();minted.minted=Some(EntityMintedToken {token:"fake.secret.for-lifecycle-proof".into(),client:"did:key:client".into(),expires_at:"2027-01-01T00:00:00Z".into()});
        crate::dialogs::entity_access::apply(&mut mgr,&target,&next_mint,&Ok(minted));
        let ctx=bevy_egui::egui::Context::default();
        let painted=ctx.run(bevy_egui::egui::RawInput::default(),|ctx| {
            bevy_egui::egui::CentralPanel::default().show(ctx,|ui| {
                if let ActiveDialog::EntitySettings {access,api_token_role_buf,api_token_expiry_buf,api_token_client_buf,..}=&mut mgr.active_dialog {
                    let _=crate::dialogs::entity_access::api_controls(ui,access,api_token_role_buf,api_token_expiry_buf,api_token_client_buf,Some("127.0.0.1:8882".parse().unwrap()));
                }
            });
        });
        let mut shapes:Vec<_>=painted.shapes.iter().map(|clipped|&clipped.shape).collect();
        while let Some(shape)=shapes.pop() {
            match shape {
                bevy_egui::egui::Shape::Text(text)=>assert!(!text.galley.job.text.contains("fake.secret.for-lifecycle-proof"),"Default token field and curl preview paint no raw secret"),
                bevy_egui::egui::Shape::Vec(children)=>shapes.extend(children.iter()),
                _=>{},
            }
        }
        if let ActiveDialog::EntitySettings {active_tab,access,..}=&mut mgr.active_dialog {
            crate::dialogs::entity_access::select_tab(active_tab,access,crate::dialogs::SettingsTab::General);
            assert!(access.snapshot.as_ref().is_none_or(|snapshot|snapshot.minted.is_none()));
        }
        let example=crate::dialogs::entity_access::scoped_example("127.0.0.1:8882".parse().unwrap(),&target,"<TOKEN>").unwrap();
        assert!(example.contains("http://127.0.0.1:8882/api/v1/query")&&!example.contains("/health")&&!example.contains("/api/v1/nodes'"));
        let mut petal=target.clone();petal.entity_type=fe_runtime::messages::EntityType::Petal;petal.entity_id="p/one'quote".into();
        let example=crate::dialogs::entity_access::scoped_example("[::1]:8882".parse().unwrap(),&petal,"<TOKEN>").unwrap();
        assert!(example.contains("http://[::1]:8882/api/v1/petals/p%2Fone")&&example.contains("/nodes"));
        mgr.close_dialog();
    }

    // ---- D11: UiAction::RevealSection ------------------------------------

    /// The `RevealSection` arm of `process_ui_actions`, verbatim. Mirrored here
    /// so the routing is testable without spinning a Bevy `App` — if the
    /// handler body changes, this must change with it.
    fn apply_reveal(
        right: &mut crate::ui_shell::right_sidebar::RightSidebarState,
        action: UiAction,
    ) -> bool {
        match action {
            UiAction::RevealSection { slug } => right.reveal_slug(&slug),
            other => panic!("expected RevealSection, got {other:?}"),
        }
    }

    #[test]
    fn reveal_section_routes_every_surface_through_its_slug() {
        use crate::ui_shell::right_sidebar::{RightSidebarState, ALL_SECTIONS};
        for section in ALL_SECTIONS {
            let mut right = RightSidebarState::default();
            let action = UiAction::RevealSection {
                slug: section.slug().to_string(),
            };
            assert!(apply_reveal(&mut right, action), "{section:?}");
            assert_eq!(right.requested, Some(section));
        }
    }

    #[test]
    fn reveal_section_is_idempotent_and_ignores_unknown_slugs() {
        use crate::ui_shell::right_sidebar::{RightSidebarSection, RightSidebarState};
        let mut right = RightSidebarState::default();
        let terrain = || UiAction::RevealSection {
            slug: RightSidebarSection::TerrainTools.slug().to_string(),
        };
        assert!(apply_reveal(&mut right, terrain()));
        // Addressing the same surface twice must NOT toggle it shut.
        assert!(apply_reveal(&mut right, terrain()));
        assert_eq!(right.requested, Some(RightSidebarSection::TerrainTools));
        // An unknown handle is a no-op, not a state wipe.
        assert!(!apply_reveal(
            &mut right,
            UiAction::RevealSection {
                slug: "path_tools".to_string(),
            }
        ));
        assert_eq!(right.requested, Some(RightSidebarSection::TerrainTools));
    }

    // ---- D16: UiAction::TerrainProposalSelect ------------------------------

    /// The `TerrainProposalSelect` arm of `process_ui_actions`, verbatim
    /// (mirrors `apply_reveal`'s pattern) — a one-line state write, kept
    /// testable without spinning a Bevy `App`.
    fn apply_select_proposal(
        proposal_state: &mut crate::terrain_proposal_state::ProposalEditState,
        action: UiAction,
    ) {
        match action {
            UiAction::TerrainProposalSelect { id } => proposal_state.selected = Some(id),
            other => panic!("expected TerrainProposalSelect, got {other:?}"),
        }
    }

    #[test]
    fn terrain_proposal_select_sets_the_selection() {
        let mut proposal_state = crate::terrain_proposal_state::ProposalEditState::default();
        assert!(proposal_state.selected.is_none());
        apply_select_proposal(
            &mut proposal_state,
            UiAction::TerrainProposalSelect {
                id: "p3".to_string(),
            },
        );
        assert_eq!(proposal_state.selected.as_deref(), Some("p3"));
    }

    #[test]
    fn terrain_proposal_select_overwrites_a_prior_selection() {
        let mut proposal_state = crate::terrain_proposal_state::ProposalEditState::default();
        proposal_state.selected = Some("p1".to_string());
        apply_select_proposal(
            &mut proposal_state,
            UiAction::TerrainProposalSelect {
                id: "p2".to_string(),
            },
        );
        assert_eq!(proposal_state.selected.as_deref(), Some("p2"));
    }

    // ---- Finding #17 fix: PathDeleteTrack synchronously closes a matching
    // editing session ------------------------------------------------------

    /// The `PathDeleteTrack` arm of `process_ui_actions`, verbatim.
    fn apply_path_delete_track(
        path_state: &mut crate::gis::PathEditorState,
        path_ops: &mut crate::path_ops::PendingPathOps,
        track_node_id: String,
    ) {
        if path_state.editing_track_id.as_deref() == Some(track_node_id.as_str()) {
            path_state.stop_editing();
        }
        path::delete_track(path_ops, track_node_id);
    }

    #[test]
    fn path_delete_track_synchronously_closes_the_matching_editing_session() {
        let mut path_state = crate::gis::PathEditorState::default();
        path_state.start_editing("track-1".to_string());
        let mut path_ops = crate::path_ops::PendingPathOps::default();

        apply_path_delete_track(&mut path_state, &mut path_ops, "track-1".to_string());

        assert!(
            path_state.editing_track_id.is_none(),
            "the session must close in the SAME frame, not wait on the DB round trip"
        );
        assert_eq!(path_ops.0.len(), 1, "the delete op still queues");
    }

    #[test]
    fn path_delete_track_leaves_an_unrelated_editing_session_open() {
        let mut path_state = crate::gis::PathEditorState::default();
        path_state.start_editing("track-1".to_string());
        let mut path_ops = crate::path_ops::PendingPathOps::default();

        apply_path_delete_track(&mut path_state, &mut path_ops, "track-2".to_string());

        assert_eq!(
            path_state.editing_track_id.as_deref(),
            Some("track-1"),
            "deleting a DIFFERENT track must not touch the active editing session"
        );
    }
}

fn queue_checked_point(
    undo: &mut crate::undo::UndoState,
    nav: &crate::navigation_manager::NavigationManager,
    hierarchy: &crate::verse_manager::VerseManager,
    ops: &mut crate::path_ops::PendingPathOps,
    state: &crate::gis::PathEditorState,
    track: String,
    index: usize,
    position: [f32; 3],
    ui: &mut UiManager,
    now: f64,
) {
    if state.editing_track_id.as_deref() != Some(track.as_str()) {
        ui.show_advisory("Path selection changed; move skipped", now);
        return;
    }
    let Some(before) = state.points.get(index).map(|row| row.position) else {
        ui.show_advisory("Path point is unavailable", now);
        return;
    };
    match undo.reserve_path(
        nav,
        &track,
        index,
        before,
        position,
        hierarchy.committed_versions.get(&track).copied(),
        &state.points,
    ) {
        Ok(Some(request_id)) => {
            ops.0.push(crate::path_ops::PathOp::CheckedMovePoint {
                track_node_id: track,
                request_id,
            });
            ui.show_toast("Saving point move...", now);
        }
        Ok(None) => {}
        Err(error) => ui.show_advisory(error, now),
    }
}
