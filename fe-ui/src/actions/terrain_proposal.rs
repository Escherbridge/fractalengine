//! Terrain-proposal action handling (terrain_editor_overhaul FR-5): add/delete
//! proposed-overlay records and persist the whole set ADDITIVELY under the petal
//! terrain config's `proposals` key — without clobbering the tileset/layer
//! config. Mirrors `actions::gis::set_layer`'s "mutate one field of the stored
//! terrain JSON, then round-trip via SetPetalTerrain" idiom. See
//! `fe-ui/src/AGENTS.md` §terrain-proposal-editor.

use bevy::prelude::{MessageReader, Res, ResMut, Resource};
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand};

use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp, ProposalRecord};

/// Additively embed `records` under the terrain config's `proposals` key,
/// preserving every other field (origin, layers, tileset uris, world_scale, …).
/// A `None`/non-object base seeds the same complete baseline shape as
/// `terrain_map::tileset_to_terrain_json` (never a bare `{"proposals": [...]}`
/// skeleton) — see `fe-ui/src/AGENTS.md` §terrain-proposal-editor for why. Pure
/// so the additive-merge contract (NFR-1: never clobber tileset config) is
/// testable.
pub(crate) fn embed_proposals(
    base: Option<&serde_json::Value>,
    records: &[ProposalRecord],
) -> serde_json::Value {
    let mut doc = match base {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => baseline_terrain_doc(),
    };
    doc["proposals"] = crate::terrain_proposal_state::to_json(records);
    doc
}

/// Complete, well-formed terrain doc for a petal with no map assigned yet:
/// `enabled: false` (honest — no real tileset backs it) plus every field a
/// terrain-JSON reader might expect (mirrors
/// `terrain_map::tileset_to_terrain_json`'s shape). Ensures a proposals-only
/// edit never leaves `PetalMapState.terrain_json` in a shape some consumer
/// doesn't expect (`ui_ux.md §6` — no silent-failure surfaces).
fn baseline_terrain_doc() -> serde_json::Value {
    serde_json::json!({
        "enabled": false,
        "origin": { "origin_lat": 0.0, "origin_lon": 0.0, "origin_ele": 0.0 },
        "tile_source_url": "",
        "layers": [],
        "tileset_hexon_uris": [],
        "world_scale": 1.0,
    })
}

/// Persist the current proposal set on the active petal's terrain config.
/// Optimistically updates `petal_map.terrain_json` only after the command is
/// queued (mirrors `hexon::set_petal_map`; `PetalTerrainLoaded` confirms).
fn persist(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    proposals: &[ProposalRecord],
    petal_id: String,
) {
    let terrain = embed_proposals(petal_map.terrain_json.as_ref(), proposals);
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(terrain.clone()),
    }) {
        Ok(()) => {
            petal_map.petal_id = Some(petal_id);
            petal_map.terrain_json = Some(terrain);
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain (proposals) not dispatched; local state unchanged"
            );
        }
    }
}

/// Add a proposal to the active petal and re-persist the block additively.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    proposals: &mut ProposalEditState,
    active_petal: Option<String>,
    op: ProposalOp,
    footprint: Vec<[f32; 2]>,
    target_height: Option<f32>,
    delta: Option<f32>,
) {
    let Some(petal_id) = active_petal else {
        bevy::log::warn!("TerrainProposalAdd ignored — no active petal");
        return;
    };
    proposals.push_new(op, footprint, target_height, delta);
    persist(db_sender, petal_map, &proposals.proposals, petal_id);
}

/// Delete a proposal by id and re-persist the (now-smaller) block.
pub(crate) fn delete(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    proposals: &mut ProposalEditState,
    active_petal: Option<String>,
    id: String,
) {
    let Some(petal_id) = active_petal else {
        bevy::log::warn!("TerrainProposalDelete ignored — no active petal");
        return;
    };
    proposals.remove(&id);
    persist(db_sender, petal_map, &proposals.proposals, petal_id);
}

// ---------------------------------------------------------------------------
// Wave-1 sculpt & earthwork (T3 sculpt_earthwork_regions). The sculpt tool
// EVOLVES the proposal path (FR-6): a defined-shape/brush earthwork edit is
// persisted as an enriched record in the SAME `terrain.proposals` block (adds a
// `material` tag; volume is derived at report time), so it round-trips through
// the existing `SetPetalTerrain` path with no fork and no new config surface.
// Q-2: delete REVERTS the baked contribution — because proposals are a
// non-destructive overlay recomputed from the record set, dropping the record
// IS the revert (the true `TerrainHeightField` was never written, NFR-1). The
// region JSON mirrors `fe_terrain::sculpt::EarthworkRegion` by contract (fe-ui
// must NOT depend on fe-terrain). See `fe-ui/src/AGENTS.md` §terrain-proposal-editor.
// ---------------------------------------------------------------------------

/// Which area-selection footprint the sculpt tool builds (D-A8: a defined shape
/// is what makes the region "an actual shape you can report on"). fe-ui-local
/// (same idiom as `panels::tool_panel::TerrainToolMode`) — no fe-terrain dep.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SculptShapeMode {
    /// Freeform brush disc (tactile).
    #[default]
    Brush,
    /// Defined circle (reportable).
    Circle,
    /// Defined axis-aligned rectangle.
    Rect,
    /// Defined polygon from the in-progress `region_draft` points.
    Polygon,
}

impl SculptShapeMode {
    pub fn label(self) -> &'static str {
        match self {
            SculptShapeMode::Brush => "Brush",
            SculptShapeMode::Circle => "Circle",
            SculptShapeMode::Rect => "Rectangle",
            SculptShapeMode::Polygon => "Polygon",
        }
    }
    pub const ALL: [SculptShapeMode; 4] = [
        SculptShapeMode::Brush,
        SculptShapeMode::Circle,
        SculptShapeMode::Rect,
        SculptShapeMode::Polygon,
    ];
}

/// The sculpt operation applied within a region (FR-2). Mirrors
/// `fe_terrain::sculpt::SculptOp`'s snake tags via [`SculptOpKind::to_snake`];
/// kept fe-ui-local so this panel has no fe-terrain dependency.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SculptOpKind {
    #[default]
    Raise,
    Lower,
    Level,
    Smooth,
}

impl SculptOpKind {
    pub fn label(self) -> &'static str {
        match self {
            SculptOpKind::Raise => "Raise",
            SculptOpKind::Lower => "Lower",
            SculptOpKind::Level => "Level",
            SculptOpKind::Smooth => "Smooth",
        }
    }
    /// Snake tag carried in `UiAction::Sculpt*` `op` strings (region JSON contract).
    pub fn to_snake(self) -> &'static str {
        match self {
            SculptOpKind::Raise => "raise",
            SculptOpKind::Lower => "lower",
            SculptOpKind::Level => "level",
            SculptOpKind::Smooth => "smooth",
        }
    }
    pub const ALL: [SculptOpKind; 4] = [
        SculptOpKind::Raise,
        SculptOpKind::Lower,
        SculptOpKind::Level,
        SculptOpKind::Smooth,
    ];
}

/// Per-frame sculpt-tool state (T3 FR-1/FR-2): the armed shape mode + op, brush
/// radius/strength, level target + delta, material tag (single-material this
/// landing, Q-3), and the in-progress polygon `region_draft`. Sculpt UI actions
/// are buffered in `pending_actions` (drained by `process_ui_actions`, mirroring
/// `ToolPanelState.drain_pending` — the sculpt section has no `ui_mgr` handle).
/// `pub` (not `pub(crate)`): flows through the `pub` `gardener_console` /
/// `render_right_sidebar` render path — must be at least as visible as they are.
#[derive(Resource)]
pub struct SculptToolState {
    pub shape_mode: SculptShapeMode,
    pub op: SculptOpKind,
    /// Brush/defined-circle radius (petal-local meters, N-1).
    pub radius: f32,
    /// Brush relaxation strength `[0,1]` (Smooth pull; brush dab weight).
    pub strength: f32,
    /// Absolute target height for Level (petal-local meters).
    pub target_height: f32,
    /// Signed delta for Raise/Lower (petal-local meters).
    pub delta: f32,
    /// Material tag baked into the region record (Q-3 single-material default).
    pub material: String,
    /// In-progress polygon footprint points `[x, z]` (Polygon shape mode).
    pub region_draft: Vec<[f32; 2]>,
    /// Monotonic id counter for minted region ids (no `uuid`/`rand` dep).
    /// `pub(crate)`: reachable from `panels::terrain_tools_panel`'s `..Default`
    /// struct-update in tests (FRU needs every field visible; E0451 otherwise).
    pub(crate) next_region_id: u64,
    /// Sculpt-UI actions queued during the egui pass; drained by
    /// `process_ui_actions` (the sculpt section has no `ui_mgr`, mirroring
    /// `ToolPanelState.pending_actions`). `pub(crate)` for the same FRU reason
    /// as `next_region_id`.
    pub(crate) pending_actions: Vec<crate::actions::UiAction>,
}

impl Default for SculptToolState {
    fn default() -> Self {
        Self {
            shape_mode: SculptShapeMode::default(),
            op: SculptOpKind::default(),
            radius: 5.0,
            strength: 0.5,
            target_height: 0.0,
            delta: 1.0,
            material: "earth".to_string(),
            region_draft: Vec::new(),
            next_region_id: 0,
            pending_actions: Vec::new(),
        }
    }
}

impl SculptToolState {
    /// Queue a sculpt `UiAction` for the drain in `process_ui_actions`.
    pub fn queue_action(&mut self, action: crate::actions::UiAction) {
        self.pending_actions.push(action);
    }

    /// Drain queued sculpt actions (called by `process_ui_actions`, mirroring
    /// `ToolPanelState.drain_pending`).
    pub fn drain_pending(&mut self) -> Vec<crate::actions::UiAction> {
        std::mem::take(&mut self.pending_actions)
    }

    /// Mint a fresh region id (`r{n}`), monotonic so ids never collide with a
    /// rehydrated one within a session.
    fn mint_region_id(&mut self) -> String {
        self.next_region_id += 1;
        format!("r{}", self.next_region_id)
    }
}

/// Mint a region id that is unused in the current terrain doc's `proposals`
/// array (the counter restarts per session; rehydrated `r{n}` ids must not be
/// reused — the node map keys on them). Pure over the doc.
fn mint_unused_region_id(
    state: &mut SculptToolState,
    terrain: Option<&serde_json::Value>,
) -> String {
    loop {
        let id = state.mint_region_id();
        let taken = terrain
            .and_then(|t| t.get("proposals"))
            .and_then(|p| p.as_array())
            .is_some_and(|arr| {
                arr.iter()
                    .any(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            });
        if !taken {
            return id;
        }
    }
}

// ---------------------------------------------------------------------------
// Earthwork region NODE rows (D-A8/N-10): every committed region is also an
// addressable node whose property bag mirrors fe-query's literal read contract
// (node_kind="earthwork_region", region_id, material, cut/fill volumes). The
// map below is the region↔node bookkeeping + the volume changed-value gate.
// See `fe-ui/src/actions/AGENTS.md` §sculpt.
// ---------------------------------------------------------------------------

/// `node_kind` contract value for earthwork region nodes (mirror of fe-query's
/// literal key — do not import fe-query).
pub(crate) const EARTHWORK_NODE_KIND: &str = "earthwork_region";
/// `CreateNode.correlation_id` prefix binding a created node to its region.
pub(crate) const EARTHWORK_CORRELATION_PREFIX: &str = "earthwork:";
/// Property keys for the real-unit volume contract (fe-query sums these).
pub(crate) const KEY_CUT_VOLUME: &str = "cut_volume_m3";
pub(crate) const KEY_FILL_VOLUME: &str = "fill_volume_m3";

/// region_id ↔ node_id bookkeeping for earthwork region nodes, plus the
/// pending-material stash consumed on `NodeCreated` (the Pen tool's
/// pending-correlation idiom) and the last-persisted volume cache (the DB
/// write gate for bake re-fires).
#[derive(Resource, Default)]
pub struct EarthworkNodeMap {
    nodes: std::collections::HashMap<String, String>,
    pending_materials: std::collections::HashMap<String, String>,
    last_sent_volumes: std::collections::HashMap<String, (f64, f64)>,
}

impl EarthworkNodeMap {
    /// Stash the material tag until the region's `NodeCreated` echo arrives.
    pub fn stash_pending_material(&mut self, region_id: &str, material: &str) {
        self.pending_materials
            .insert(region_id.to_string(), material.to_string());
    }

    /// Consume the stashed material for `region_id` (once, on `NodeCreated`).
    pub fn take_pending_material(&mut self, region_id: &str) -> Option<String> {
        self.pending_materials.remove(region_id)
    }

    /// Bind `region_id` to its created/hydrated node.
    pub fn record(&mut self, region_id: &str, node_id: &str) {
        self.nodes
            .insert(region_id.to_string(), node_id.to_string());
    }

    /// The node backing `region_id`, when known.
    pub fn node_for(&self, region_id: &str) -> Option<&str> {
        self.nodes.get(region_id).map(String::as_str)
    }

    /// Drop all bookkeeping for a deleted region; returns its node id (the
    /// tombstone target) when one was known.
    pub fn forget_region(&mut self, region_id: &str) -> Option<String> {
        self.pending_materials.remove(region_id);
        self.last_sent_volumes.remove(region_id);
        self.nodes.remove(region_id)
    }

    /// Changed-value gate: `true` when `(cut, fill)` differs from the last
    /// persisted pair (bake re-fires per revision are deterministic for
    /// unchanged inputs, so exact comparison is the correct no-spam gate).
    pub fn volume_changed(&self, region_id: &str, cut_m3: f64, fill_m3: f64) -> bool {
        self.last_sent_volumes.get(region_id) != Some(&(cut_m3, fill_m3))
    }

    /// Record a successfully-persisted volume pair (also seeded on hydration).
    pub fn mark_volume_sent(&mut self, region_id: &str, cut_m3: f64, fill_m3: f64) {
        self.last_sent_volumes
            .insert(region_id.to_string(), (cut_m3, fill_m3));
    }
}

/// Vertex-mean centroid of a footprint `[x, z]` (planning-grade node anchor).
pub(crate) fn footprint_centroid(footprint: &[[f32; 2]]) -> [f32; 2] {
    if footprint.is_empty() {
        return [0.0, 0.0];
    }
    let n = footprint.len() as f32;
    let (sx, sz) = footprint
        .iter()
        .fold((0.0f32, 0.0f32), |(sx, sz), [x, z]| (sx + x, sz + z));
    [sx / n, sz / n]
}

/// Extract the region id from an `earthwork:{region_id}` correlation id.
pub(crate) fn earthwork_region_id_from_correlation(correlation_id: &str) -> Option<&str> {
    correlation_id
        .strip_prefix(EARTHWORK_CORRELATION_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Display name for a region node, e.g. `"Earthwork raise r3"`.
pub(crate) fn earthwork_display_name(op: &str, region_id: &str) -> String {
    format!("Earthwork {op} {region_id}")
}

/// Initial property bag for a freshly-created region node (volumes start 0.0;
/// the bake report updates them async). Pure — the endpoint contract is testable.
pub(crate) fn earthwork_node_properties(
    region_id: &str,
    material: &str,
) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("node_kind", serde_json::json!(EARTHWORK_NODE_KIND)),
        ("material", serde_json::json!(material)),
        ("region_id", serde_json::json!(region_id)),
        (KEY_CUT_VOLUME, serde_json::json!(0.0)),
        (KEY_FILL_VOLUME, serde_json::json!(0.0)),
    ]
}

/// Send the `CreateNode` making a committed region an addressable endpoint
/// (D-A8/N-10): anchored at the footprint centroid (raw petal-local meters,
/// N-1), correlated `earthwork:{region_id}` for the `NodeCreated` bind.
fn create_region_node(
    db_sender: &DbCommandSender,
    map: &mut EarthworkNodeMap,
    petal_id: &str,
    region_id: &str,
    op: &str,
    footprint: &[[f32; 2]],
    material: &str,
) {
    let [cx, cz] = footprint_centroid(footprint);
    map.stash_pending_material(region_id, material);
    if db_sender
        .0
        .send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: earthwork_display_name(op, region_id),
            position: [cx, 0.0, cz],
            correlation_id: Some(format!("{EARTHWORK_CORRELATION_PREFIX}{region_id}")),
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — earthwork region node not created");
    }
}

/// Reload seam (`NodePropertiesLoaded`): a bag whose `node_kind` is
/// `"earthwork_region"` re-binds region_id→node_id and seeds the volume gate
/// from the persisted values (mirrors `asset::hydrate_promoted_stamp`).
pub(crate) fn hydrate_earthwork_region(
    node_id: &str,
    properties: &serde_json::Value,
    map: &mut EarthworkNodeMap,
) {
    if properties.get("node_kind").and_then(|v| v.as_str()) != Some(EARTHWORK_NODE_KIND) {
        return;
    }
    let Some(region_id) = properties.get("region_id").and_then(|v| v.as_str()) else {
        return;
    };
    map.record(region_id, node_id);
    if let (Some(cut), Some(fill)) = (
        properties.get(KEY_CUT_VOLUME).and_then(|v| v.as_f64()),
        properties.get(KEY_FILL_VOLUME).and_then(|v| v.as_f64()),
    ) {
        map.mark_volume_sent(region_id, cut, fill);
    }
}

/// Fill a drained sculpt action's `petal_id` hole from the active petal; the
/// panel/viewport queue side has no petal handle by design. `None` (with a
/// warn, N-8) drops petal-scoped actions when no petal is active; petal-free
/// actions pass through untouched. Pure — the commit line is testable.
pub(crate) fn thread_active_petal(
    action: crate::actions::UiAction,
    active_petal: Option<&str>,
) -> Option<crate::actions::UiAction> {
    use crate::actions::UiAction;
    match action {
        UiAction::SculptBrush {
            center,
            radius,
            strength,
            op,
            ..
        } => match active_petal {
            Some(petal_id) => Some(UiAction::SculptBrush {
                petal_id: petal_id.to_string(),
                center,
                radius,
                strength,
                op,
            }),
            None => {
                bevy::log::warn!("SculptBrush dropped — no active petal (N-8)");
                None
            }
        },
        UiAction::SculptShapeRegion {
            footprint,
            op,
            target_height,
            delta,
            material,
            ..
        } => match active_petal {
            Some(petal_id) => Some(UiAction::SculptShapeRegion {
                petal_id: petal_id.to_string(),
                footprint,
                op,
                target_height,
                delta,
                material,
            }),
            None => {
                bevy::log::warn!("SculptShapeRegion dropped — no active petal (N-8)");
                None
            }
        },
        other => Some(other),
    }
}

/// Persist bake-reported volumes onto the region's node — ONLY when changed vs
/// the last persisted pair (bake re-fires per revision; the DB must not be
/// spammed). Unknown region → debug (the node may not exist yet; the next
/// revision re-fires). Registered in `plugin.rs`.
pub(crate) fn persist_earthwork_volumes(
    mut reports: MessageReader<fe_renderer::terrain_overlay::EarthworkVolumeReport>,
    mut map: ResMut<EarthworkNodeMap>,
    db_sender: Res<DbCommandSender>,
) {
    for report in reports.read() {
        let Some(node_id) = map.node_for(&report.region_id).map(str::to_string) else {
            bevy::log::debug!(
                "earthwork volume for unknown region {} — node not yet created/hydrated",
                report.region_id
            );
            continue;
        };
        if !map.volume_changed(&report.region_id, report.cut_m3, report.fill_m3) {
            continue;
        }
        let mut sent = true;
        for (key, value) in [
            (KEY_CUT_VOLUME, report.cut_m3),
            (KEY_FILL_VOLUME, report.fill_m3),
        ] {
            if db_sender
                .0
                .send(DbCommand::SetNodeProperty {
                    node_id: node_id.clone(),
                    key: key.to_string(),
                    value: serde_json::json!(value),
                })
                .is_err()
            {
                bevy::log::warn!("db_sender channel closed — earthwork volumes not persisted");
                sent = false;
                break;
            }
        }
        if sent {
            map.mark_volume_sent(&report.region_id, report.cut_m3, report.fill_m3);
        }
    }
}

/// Build the enriched earthwork-region JSON object (mirrors
/// `fe_terrain::sculpt::EarthworkRegion`'s serde shape by contract). `op` is a
/// snake tag; `material` defaults are the caller's concern. Pure.
fn region_json(
    id: &str,
    op: &str,
    footprint: &[[f32; 2]],
    target_height: Option<f32>,
    delta: Option<f32>,
    material: &str,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "id": id,
        "op": op,
        "footprint": footprint,
        "material": material,
    });
    if let Some(t) = target_height {
        obj["target_height"] = serde_json::json!(t);
    }
    if let Some(d) = delta {
        obj["delta"] = serde_json::json!(d);
    }
    obj
}

/// Additively append `region` to a terrain doc's `proposals` array, preserving
/// every other field (seeds the complete baseline like `embed_proposals` for a
/// `None`/non-object base). Pure — the FR-6 "evolve, don't fork" merge is testable.
fn embed_region(base: Option<&serde_json::Value>, region: serde_json::Value) -> serde_json::Value {
    let mut doc = match base {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => baseline_terrain_doc(),
    };
    let mut arr = match doc.get("proposals") {
        Some(serde_json::Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    arr.push(region);
    doc["proposals"] = serde_json::Value::Array(arr);
    doc
}

/// Remove the region/proposal with `id` from a terrain doc's `proposals` array
/// and return the doc — the Q-2 revert (dropping the record un-bakes the
/// non-destructive overlay). Preserves all other fields. Pure.
fn remove_region(base: Option<&serde_json::Value>, id: &str) -> serde_json::Value {
    let mut doc = match base {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => baseline_terrain_doc(),
    };
    let arr = match doc.get("proposals") {
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter(|r| r.get("id").and_then(|v| v.as_str()) != Some(id))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    doc["proposals"] = serde_json::Value::Array(arr);
    doc
}

/// Persist a full terrain doc on the active petal (mirrors `persist`'s
/// optimistic update: local state advances only after the command is queued).
/// Returns whether the command was queued (gates the region-node follow-ups).
fn persist_doc(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    terrain: serde_json::Value,
    petal_id: String,
) -> bool {
    match db_sender.0.send(DbCommand::SetPetalTerrain {
        petal_id: petal_id.clone(),
        terrain: Some(terrain.clone()),
    }) {
        Ok(()) => {
            petal_map.petal_id = Some(petal_id);
            petal_map.terrain_json = Some(terrain);
            true
        }
        Err(_) => {
            bevy::log::warn!(
                "db_sender channel closed — SetPetalTerrain (sculpt region) not dispatched; local state unchanged"
            );
            false
        }
    }
}

/// T3 FR-1 brush + FR-2 op: one freeform brush dab becomes a small circular
/// region record at `center` (evolving the proposal path). `strength` weights
/// the `delta` so light dabs move less earth; the persisted footprint is the
/// brush disc so it is reportable like any region (D-A8).
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_brush(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    sculpt_state: &mut SculptToolState,
    earthwork_map: &mut EarthworkNodeMap,
    petal_id: String,
    center: [f32; 2],
    radius: f32,
    strength: f32,
    op: String,
) {
    let footprint = brush_disc(center, radius);
    if footprint.len() < 3 {
        bevy::log::warn!("SculptBrush ignored — degenerate brush footprint");
        return;
    }
    // Brush dab moves earth proportional to strength (planning-grade).
    let delta = Some(sculpt_state.delta * strength.clamp(0.0, 1.0));
    let id = mint_unused_region_id(sculpt_state, petal_map.terrain_json.as_ref());
    let material = sculpt_state.material.clone();
    let region = region_json(&id, &op, &footprint, None, delta, &material);
    let terrain = embed_region(petal_map.terrain_json.as_ref(), region);
    if persist_doc(db_sender, petal_map, terrain, petal_id.clone()) {
        // D-A8/N-10: the committed region is also an addressable node row.
        create_region_node(
            db_sender,
            earthwork_map,
            &petal_id,
            &id,
            &op,
            &footprint,
            &material,
        );
    }
}

/// T3 FR-1 shape + FR-3 region + FR-4 volume: create a defined-shape earthwork
/// region record (the reportable BIM node, D-A8). Persisted in the `proposals`
/// block enriched with `material`; the report derives cut/fill volume.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_shape_region(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    sculpt_state: &mut SculptToolState,
    earthwork_map: &mut EarthworkNodeMap,
    petal_id: String,
    footprint: Vec<[f32; 2]>,
    op: String,
    target_height: Option<f32>,
    delta: Option<f32>,
    material: String,
) {
    if footprint.len() < 3 {
        bevy::log::warn!("SculptShapeRegion ignored — footprint has fewer than 3 points");
        return;
    }
    let id = mint_unused_region_id(sculpt_state, petal_map.terrain_json.as_ref());
    let region = region_json(&id, &op, &footprint, target_height, delta, &material);
    let terrain = embed_region(petal_map.terrain_json.as_ref(), region);
    if persist_doc(db_sender, petal_map, terrain, petal_id.clone()) {
        // D-A8/N-10: the committed region is also an addressable node row.
        create_region_node(
            db_sender,
            earthwork_map,
            &petal_id,
            &id,
            &op,
            &footprint,
            &material,
        );
    }
    // The draft has been committed to a region — clear it for the next shape.
    sculpt_state.region_draft.clear();
}

/// T3 FR-3 delete (Q-2 ratified): delete an earthwork region node, REVERTING its
/// baked contribution by dropping the record (the overlay un-bakes; the true
/// heightfield was never written, NFR-1).
pub(crate) fn handle_delete_region(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    _sculpt_state: &mut SculptToolState,
    earthwork_map: &mut EarthworkNodeMap,
    active_petal: Option<String>,
    region_id: String,
) {
    let Some(petal_id) = active_petal else {
        bevy::log::warn!("SculptDeleteRegion ignored — no active petal");
        return;
    };
    let terrain = remove_region(petal_map.terrain_json.as_ref(), &region_id);
    if persist_doc(db_sender, petal_map, terrain, petal_id) {
        // Keep the endpoint contract honest: tombstone the region's node row
        // (sync-safe, N-4) when the map knows it. Auth is `CallerAuth::Local`
        // — the UI asserts no role (N-5).
        if let Some(node_id) = earthwork_map.forget_region(&region_id) {
            if db_sender
                .0
                .send(DbCommand::TombstoneNode {
                    node_id,
                    auth: CallerAuth::Local,
                })
                .is_err()
            {
                bevy::log::warn!("db_sender channel closed — earthwork node tombstone not sent");
            }
        }
    }
}

/// A closed CCW brush disc footprint `[x, z]` (petal-local meters). Empty for a
/// non-positive radius so the caller drops a degenerate dab.
fn brush_disc(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    const SEGMENTS: usize = 24;
    if !(radius.is_finite() && radius > 0.0) {
        return Vec::new();
    }
    (0..SEGMENTS)
        .map(|i| {
            let t = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
            [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str) -> ProposalRecord {
        ProposalRecord {
            id: id.into(),
            op: ProposalOp::Raise,
            footprint: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            target_height: None,
            delta: Some(2.0),
        }
    }

    #[test]
    fn embed_preserves_existing_terrain_config() {
        // A realistic terrain doc (tileset + layers + scale) must survive intact.
        let base = json!({
            "enabled": true,
            "world_scale": 0.001,
            "tileset_hexon_uris": ["ts-1"],
            "layers": [{ "name": "satellite", "visible": true }],
        });
        let out = embed_proposals(Some(&base), &[record("p1")]);
        // Proposals added…
        assert_eq!(out["proposals"][0]["id"], json!("p1"));
        assert_eq!(out["proposals"][0]["op"], json!("raise"));
        // …and NOTHING else clobbered (NFR-1 additive contract).
        assert_eq!(out["world_scale"], json!(0.001));
        assert_eq!(out["tileset_hexon_uris"], json!(["ts-1"]));
        assert_eq!(out["layers"][0]["name"], json!("satellite"));
    }

    #[test]
    fn embed_none_base_yields_complete_baseline_doc() {
        // Regression (H-C1, ui_shell_architecture_20260724 Phase 0): a
        // map-less petal must NOT get a bare `{"proposals": [...]}` skeleton —
        // every field a terrain-JSON reader (fe-ui panels, fe-terrain's
        // `TerrainConfig`) might require must be present with a safe default.
        let out = embed_proposals(None, &[record("p1")]);
        assert!(out.is_object());
        assert_eq!(out["proposals"][0]["id"], json!("p1"));
        assert_eq!(out["enabled"], json!(false), "no real map — honest default");
        assert_eq!(out["tile_source_url"], json!(""));
        assert_eq!(out["layers"], json!([]));
        assert_eq!(out["tileset_hexon_uris"], json!([]));
        assert_eq!(out["world_scale"], json!(1.0));
        assert_eq!(out["origin"]["origin_lat"], json!(0.0));
        assert_eq!(out["origin"]["origin_lon"], json!(0.0));
        assert_eq!(out["origin"]["origin_ele"], json!(0.0));
    }

    #[test]
    fn embed_non_object_base_also_gets_the_complete_baseline() {
        // A non-object base (e.g. a stale/corrupt doc) must not leak through
        // as the seed — same complete-baseline treatment as `None`.
        let out = embed_proposals(Some(&json!([1, 2, 3])), &[record("p1")]);
        assert_eq!(out["enabled"], json!(false));
        assert_eq!(out["tile_source_url"], json!(""));
        assert_eq!(out["proposals"][0]["id"], json!("p1"));
    }

    #[test]
    fn embed_none_base_doc_carries_every_field_terrain_config_requires() {
        // fe_terrain::config::TerrainConfig has NO #[serde(default)] on
        // `enabled`/`origin`/`tile_source_url` — deserializing a doc missing
        // any of them fails. Pin that the baseline always carries all three
        // (fe-ui can't import TerrainConfig itself — boundary rule — so this
        // asserts the JSON shape directly).
        let out = embed_proposals(None, &[]);
        assert!(out.get("enabled").and_then(|v| v.as_bool()).is_some());
        assert!(out
            .get("tile_source_url")
            .and_then(|v| v.as_str())
            .is_some());
        let origin = out.get("origin").expect("origin present");
        assert!(origin.get("origin_lat").and_then(|v| v.as_f64()).is_some());
        assert!(origin.get("origin_lon").and_then(|v| v.as_f64()).is_some());
        assert!(origin.get("origin_ele").and_then(|v| v.as_f64()).is_some());
    }

    #[test]
    fn embed_empty_records_writes_empty_array() {
        let out = embed_proposals(Some(&json!({ "enabled": true })), &[]);
        assert_eq!(out["proposals"], json!([]));
        assert_eq!(out["enabled"], json!(true));
    }

    // --- T3 sculpt & earthwork region helpers ---

    #[test]
    fn region_json_carries_material_and_omits_none_params() {
        let fp = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let r = region_json("r1", "level", &fp, Some(3.0), None, "gravel");
        assert_eq!(r["id"], json!("r1"));
        assert_eq!(r["op"], json!("level"));
        assert_eq!(r["material"], json!("gravel"));
        assert_eq!(r["target_height"], json!(3.0));
        assert!(r.get("delta").is_none(), "None delta omitted");
        assert_eq!(r["footprint"][2], json!([1.0, 1.0]));
    }

    #[test]
    fn embed_region_appends_without_clobbering_config_or_existing_proposals() {
        // A realistic doc with an existing proposal + tileset config: the region
        // is appended, everything else survives (FR-6 evolve, NFR-1 additive).
        let base = json!({
            "enabled": true,
            "world_scale": 0.001,
            "tileset_hexon_uris": ["ts-1"],
            "proposals": [{ "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 }],
        });
        let region = region_json(
            "r1",
            "level",
            &[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]],
            Some(1.0),
            None,
            "earth",
        );
        let out = embed_region(Some(&base), region);
        assert_eq!(out["proposals"][0]["id"], json!("p1"), "existing kept");
        assert_eq!(out["proposals"][1]["id"], json!("r1"), "region appended");
        assert_eq!(out["proposals"][1]["material"], json!("earth"));
        assert_eq!(out["world_scale"], json!(0.001), "config untouched");
        assert_eq!(out["tileset_hexon_uris"], json!(["ts-1"]));
    }

    #[test]
    fn embed_region_none_base_seeds_complete_baseline() {
        let region = region_json("r1", "raise", &[[0.0, 0.0]], None, Some(2.0), "earth");
        let out = embed_region(None, region);
        // Same complete-baseline guarantee as embed_proposals (H-C1 no-regression).
        assert_eq!(out["enabled"], json!(false));
        assert_eq!(out["tile_source_url"], json!(""));
        assert_eq!(out["world_scale"], json!(1.0));
        assert_eq!(out["proposals"][0]["id"], json!("r1"));
    }

    #[test]
    fn remove_region_reverts_by_dropping_the_record() {
        // Q-2: dropping the record un-bakes the non-destructive overlay.
        let base = json!({
            "enabled": true,
            "proposals": [
                { "id": "r1", "op": "level", "footprint": [], "material": "earth" },
                { "id": "r2", "op": "raise", "footprint": [], "delta": 1.0 }
            ],
        });
        let out = remove_region(Some(&base), "r1");
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["r2"], "only r1 removed");
        assert_eq!(out["enabled"], json!(true), "config untouched");
        // Idempotent: removing an absent id is a no-op that still round-trips.
        let again = remove_region(Some(&out), "r1");
        assert_eq!(again["proposals"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn brush_disc_is_closed_ring_or_empty() {
        let disc = brush_disc([5.0, -3.0], 2.0);
        assert_eq!(disc.len(), 24);
        for [x, z] in &disc {
            let r = ((x - 5.0).powi(2) + (z + 3.0).powi(2)).sqrt();
            assert!((r - 2.0).abs() < 1e-4);
        }
        assert!(brush_disc([0.0, 0.0], 0.0).is_empty());
        assert!(brush_disc([0.0, 0.0], f32::NAN).is_empty());
    }

    #[test]
    fn sculpt_state_mints_monotonic_ids_and_drains_actions() {
        let mut s = SculptToolState::default();
        assert_eq!(s.mint_region_id(), "r1");
        assert_eq!(s.mint_region_id(), "r2");
        assert!(s.drain_pending().is_empty());
        s.queue_action(crate::actions::UiAction::SculptDeleteRegion {
            region_id: "r1".into(),
        });
        let drained = s.drain_pending();
        assert_eq!(drained.len(), 1);
        assert!(s.drain_pending().is_empty(), "drain empties the buffer");
    }

    #[test]
    fn sculpt_op_kind_snake_tags_are_distinct() {
        let mut tags: Vec<&str> = SculptOpKind::ALL.iter().map(|o| o.to_snake()).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), SculptOpKind::ALL.len());
    }

    // --- T3 integration: earthwork node rows + the commit line ---

    #[test]
    fn footprint_centroid_is_vertex_mean() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(footprint_centroid(&square), [5.0, 5.0]);
        assert_eq!(footprint_centroid(&[[3.0, -4.0]]), [3.0, -4.0]);
        assert_eq!(footprint_centroid(&[]), [0.0, 0.0]);
    }

    #[test]
    fn earthwork_correlation_round_trips_and_rejects_foreign_ids() {
        let cid = format!("{EARTHWORK_CORRELATION_PREFIX}r7");
        assert_eq!(earthwork_region_id_from_correlation(&cid), Some("r7"));
        assert_eq!(earthwork_region_id_from_correlation("earthwork:"), None);
        assert_eq!(earthwork_region_id_from_correlation("pen:42"), None);
        assert_eq!(earthwork_region_id_from_correlation("r7"), None);
    }

    #[test]
    fn earthwork_node_properties_carry_the_endpoint_contract() {
        let props = earthwork_node_properties("r3", "gravel");
        let bag: std::collections::HashMap<&str, serde_json::Value> = props.into_iter().collect();
        assert_eq!(bag["node_kind"], json!("earthwork_region"));
        assert_eq!(bag["material"], json!("gravel"));
        assert_eq!(bag["region_id"], json!("r3"));
        assert_eq!(bag[KEY_CUT_VOLUME], json!(0.0));
        assert_eq!(bag[KEY_FILL_VOLUME], json!(0.0));
    }

    #[test]
    fn volume_changed_gate_blocks_repeats_until_values_move() {
        let mut map = EarthworkNodeMap::default();
        assert!(
            map.volume_changed("r1", 10.0, 2.0),
            "first pair always sends"
        );
        map.mark_volume_sent("r1", 10.0, 2.0);
        assert!(!map.volume_changed("r1", 10.0, 2.0), "repeat is gated");
        assert!(map.volume_changed("r1", 10.0, 2.5), "moved fill re-sends");
        assert!(
            map.volume_changed("r2", 10.0, 2.0),
            "other region unaffected"
        );
    }

    #[test]
    fn node_map_pending_and_forget_lifecycle() {
        let mut map = EarthworkNodeMap::default();
        map.stash_pending_material("r1", "earth");
        assert_eq!(map.take_pending_material("r1").as_deref(), Some("earth"));
        assert!(map.take_pending_material("r1").is_none(), "consumed once");
        map.record("r1", "node-9");
        map.mark_volume_sent("r1", 1.0, 2.0);
        assert_eq!(map.node_for("r1"), Some("node-9"));
        assert_eq!(map.forget_region("r1").as_deref(), Some("node-9"));
        assert!(map.node_for("r1").is_none());
        assert!(
            map.volume_changed("r1", 1.0, 2.0),
            "gate cleared with region"
        );
    }

    #[test]
    fn hydrate_earthwork_region_binds_and_seeds_gate() {
        let mut map = EarthworkNodeMap::default();
        let props = json!({
            "node_kind": "earthwork_region",
            "region_id": "r4",
            "material": "earth",
            "cut_volume_m3": 12.5,
            "fill_volume_m3": 0.0,
        });
        hydrate_earthwork_region("node-4", &props, &mut map);
        assert_eq!(map.node_for("r4"), Some("node-4"));
        assert!(
            !map.volume_changed("r4", 12.5, 0.0),
            "persisted volumes seed the gate — an unchanged re-bake stays quiet"
        );
        // Non-earthwork / malformed bags are ignored.
        hydrate_earthwork_region("n1", &json!({ "node_kind": "stamp" }), &mut map);
        hydrate_earthwork_region("n2", &json!({ "node_kind": "earthwork_region" }), &mut map);
        assert!(map.node_for("n1").is_none() && map.nodes.len() == 1);
    }

    #[test]
    fn thread_active_petal_fills_hole_or_drops() {
        let brush = crate::actions::UiAction::SculptBrush {
            petal_id: String::new(),
            center: [1.0, 2.0],
            radius: 3.0,
            strength: 0.5,
            op: "raise".into(),
        };
        match thread_active_petal(brush.clone(), Some("petal-1")) {
            Some(crate::actions::UiAction::SculptBrush {
                petal_id, center, ..
            }) => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(center, [1.0, 2.0]);
            }
            other => panic!("expected threaded SculptBrush, got {other:?}"),
        }
        assert!(thread_active_petal(brush, None).is_none(), "no petal drops");
        // Petal-free actions pass through untouched.
        let del = crate::actions::UiAction::SculptDeleteRegion {
            region_id: "r1".into(),
        };
        assert!(matches!(
            thread_active_petal(del, None),
            Some(crate::actions::UiAction::SculptDeleteRegion { .. })
        ));
    }

    #[test]
    fn mint_unused_region_id_skips_rehydrated_ids() {
        let mut s = SculptToolState::default();
        // A reloaded doc already holds r1/r2 from a previous session.
        let terrain = json!({ "proposals": [ { "id": "r1" }, { "id": "r2" } ] });
        assert_eq!(mint_unused_region_id(&mut s, Some(&terrain)), "r3");
        assert_eq!(mint_unused_region_id(&mut s, None), "r4");
    }
}
