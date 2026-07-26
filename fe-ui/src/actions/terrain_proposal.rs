//! Terrain-proposal action handling (terrain_editor_overhaul FR-5): add/delete
//! proposed-overlay records and persist the whole set ADDITIVELY under the petal
//! terrain config's `proposals` key — without clobbering the tileset/layer
//! config. Mirrors `actions::gis::set_layer`'s "mutate one field of the stored
//! terrain JSON, then round-trip via SetPetalTerrain" idiom. See
//! `fe-ui/src/AGENTS.md` §terrain-proposal-editor.

use bevy::prelude::Resource;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

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
fn persist_doc(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    terrain: serde_json::Value,
    petal_id: String,
) {
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
                "db_sender channel closed — SetPetalTerrain (sculpt region) not dispatched; local state unchanged"
            );
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
    let id = sculpt_state.mint_region_id();
    let material = sculpt_state.material.clone();
    let region = region_json(&id, &op, &footprint, None, delta, &material);
    let terrain = embed_region(petal_map.terrain_json.as_ref(), region);
    persist_doc(db_sender, petal_map, terrain, petal_id);
}

/// T3 FR-1 shape + FR-3 region + FR-4 volume: create a defined-shape earthwork
/// region record (the reportable BIM node, D-A8). Persisted in the `proposals`
/// block enriched with `material`; the report derives cut/fill volume.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_shape_region(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    sculpt_state: &mut SculptToolState,
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
    let id = sculpt_state.mint_region_id();
    let region = region_json(&id, &op, &footprint, target_height, delta, &material);
    let terrain = embed_region(petal_map.terrain_json.as_ref(), region);
    persist_doc(db_sender, petal_map, terrain, petal_id);
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
    active_petal: Option<String>,
    region_id: String,
) {
    let Some(petal_id) = active_petal else {
        bevy::log::warn!("SculptDeleteRegion ignored — no active petal");
        return;
    };
    let terrain = remove_region(petal_map.terrain_json.as_ref(), &region_id);
    persist_doc(db_sender, petal_map, terrain, petal_id);
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
}
