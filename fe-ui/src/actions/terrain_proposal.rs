//! Terrain-proposal action handling (terrain_editor_overhaul FR-5): add/delete
//! proposed-overlay records and persist the whole set ADDITIVELY under the petal
//! terrain config's `proposals` key — without clobbering the tileset/layer
//! config. Mirrors `actions::gis::set_layer`'s "mutate one field of the stored
//! terrain JSON, then round-trip via SetPetalTerrain" idiom. See
//! `fe-ui/src/AGENTS.md` §terrain-proposal-editor.

use bevy::prelude::{MessageReader, Res, ResMut, Resource};
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand};

use crate::geometry::meters_to_world;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::{ProposalEditState, ProposalOp, ProposalRecord};

/// Additively embed `records` under the terrain config's `proposals` key,
/// preserving every other field (origin, layers, tileset uris, world_scale, …).
/// A `None`/non-object base seeds the same complete baseline shape as
/// `terrain_map::tileset_to_terrain_json` (never a bare `{"proposals": [...]}`
/// skeleton) — see `fe-ui/src/AGENTS.md` §terrain-proposal-editor for why. Pure
/// so the additive-merge contract (NFR-1: never clobber tileset config) is
/// testable.
///
/// `hydrated` gates HOW `records` reconciles against the doc's existing
/// `proposals` array (data-loss guard, `ui_semantics_unification_20260808`
/// finding #1). The doc's `proposals` array holds TWO record shapes: palette
/// `ProposalRecord`s (`records`, this mirror) and Brush/shape-tool earthwork
/// regions tagged by a `material` key (`region_json`'s shape) — the mirror
/// NEVER absorbs region entries (see `db_results/terrain.rs`'s hydration
/// filter), so this function must never let a palette-only operation touch
/// them either:
///
/// - **Hydrated**: the mirror holds the petal's complete PALETTE set
///   (persisted + local edits), so this is a **shape-preserving merge** —
///   palette-shaped entries in the doc are wholesale-replaced by `records`
///   (so palette add/delete/edit all apply), while every material-tagged
///   region entry passes through the doc byte-identical. No sequence of
///   palette adds/deletes can remove or alter a region (finding #1 invariant).
/// - **Unhydrated**: `records` is only a partial/local view (the mirror
///   hasn't loaded the petal's persisted set yet), so this performs a
///   **tombstone-aware union** like `embed_region`/`remove_region`: every doc
///   entry survives except ids in `pending_deletes` (finding #13 fix — a
///   delete issued before hydration would otherwise be silently resurrected
///   by the "keep every existing entry" rule, since the mirror has no way to
///   know the id was ever persisted). An id present in `records` wins over
///   the doc's copy for that id (handles the case where a locally-minted
///   `p{n}` id happens to collide with an already-persisted one — the local
///   version must not be silently discarded); every other id `records`
///   doesn't mention is appended as new. This is what makes an Add/Delete
///   racing ahead of `PetalTerrainLoaded` safe.
pub(crate) fn embed_proposals(
    base: Option<&serde_json::Value>,
    records: &[ProposalRecord],
    hydrated: bool,
    pending_deletes: &std::collections::HashSet<String>,
) -> serde_json::Value {
    let mut doc = match base {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => baseline_terrain_doc(),
    };
    if hydrated {
        let region_entries: Vec<serde_json::Value> = match doc.get("proposals") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter(|item| item.get("material").is_some())
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let mut merged = match crate::terrain_proposal_state::to_json(records) {
            serde_json::Value::Array(a) => a,
            _ => Vec::new(),
        };
        merged.extend(region_entries);
        doc["proposals"] = serde_json::Value::Array(merged);
        return doc;
    }
    let mut existing: Vec<serde_json::Value> = match doc.get("proposals") {
        Some(serde_json::Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    // Finding #13 fix: drop tombstoned ids first — otherwise the
    // keep-every-existing-entry rule below would resurrect them.
    existing.retain(|item| {
        item.get("id")
            .and_then(|v| v.as_str())
            .is_none_or(|id| !pending_deletes.contains(id))
    });
    let record_by_id: std::collections::HashMap<&str, &ProposalRecord> =
        records.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in existing.iter_mut() {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        // Prefer the mirror's version for any id it currently holds (new OR
        // colliding-with-persisted) — see the doc comment above.
        if let Some(record) = record_by_id.get(id.as_str()) {
            if let Ok(value) = serde_json::to_value(record) {
                *item = value;
            }
        }
        seen_ids.insert(id);
    }
    for record in records {
        if !seen_ids.contains(&record.id) {
            if let Ok(value) = serde_json::to_value(record) {
                existing.push(value);
            }
        }
    }
    doc["proposals"] = serde_json::Value::Array(existing);
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
/// Takes the whole `ProposalEditState` (not just its `proposals` slice) so
/// `embed_proposals` can read `hydrated` and pick the correct reconciliation
/// strategy (see its doc comment).
fn persist(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    proposals: &ProposalEditState,
    petal_id: String,
) {
    let terrain = embed_proposals(
        petal_map.terrain_json.as_ref(),
        &proposals.proposals,
        proposals.hydrated,
        &proposals.pending_deletes,
    );
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
    // Collision guard: an unhydrated mirror hasn't absorbed the doc's ids yet.
    if let Some(existing) = petal_map
        .terrain_json
        .as_ref()
        .and_then(|doc| doc["proposals"].as_array())
    {
        proposals.ensure_ids_beyond(existing.iter().filter_map(|r| r["id"].as_str()));
    }
    proposals.push_new(op, footprint, target_height, delta);
    persist(db_sender, petal_map, proposals, petal_id);
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
    persist(db_sender, petal_map, proposals, petal_id);
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
/// landing, Q-3), and the in-progress polygon `region_draft`.
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
        }
    }
}

pub(crate) const MAX_SCULPT_DISTANCE: f32 = 1_000_000.0;

impl SculptToolState {
    pub(crate) fn sanitized_radius(&self) -> f32 {
        if self.radius.is_finite() {
            self.radius.clamp(0.1, MAX_SCULPT_DISTANCE)
        } else {
            5.0
        }
    }

    /// Repair numeric buffers at the UI/interaction boundary.
    pub(crate) fn sanitize_numeric_state(&mut self) {
        self.radius = self.sanitized_radius();
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.target_height = if self.target_height.is_finite() {
            self.target_height
                .clamp(-MAX_SCULPT_DISTANCE, MAX_SCULPT_DISTANCE)
        } else {
            0.0
        };
        self.delta = if self.delta.is_finite() {
            self.delta.clamp(-MAX_SCULPT_DISTANCE, MAX_SCULPT_DISTANCE)
        } else {
            1.0
        };
        debug_assert!([self.radius, self.strength, self.target_height, self.delta]
            .into_iter()
            .all(f32::is_finite));
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
        persist_earthwork_volume_report(report, &mut map, &db_sender);
    }
}

/// One report's changed-value-gated persistence, extracted for composed seam tests.
fn persist_earthwork_volume_report(
    report: &fe_renderer::terrain_overlay::EarthworkVolumeReport,
    map: &mut EarthworkNodeMap,
    db_sender: &DbCommandSender,
) {
    let Some(node_id) = map.node_for(&report.region_id).map(str::to_string) else {
        bevy::log::debug!(
            "earthwork volume for unknown region {} — node not yet created/hydrated",
            report.region_id
        );
        return;
    };
    if !map.volume_changed(&report.region_id, report.cut_m3, report.fill_m3) {
        return;
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

pub(crate) fn petal_map_enabled(petal_map: &PetalMapState, petal_id: &str) -> bool {
    petal_map.petal_id.as_deref() == Some(petal_id)
        && petal_map
            .terrain_json
            .as_ref()
            .and_then(|doc| doc.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

/// Defense-in-depth cap shared with the viewport sampler.
pub(crate) const MAX_BRUSH_DABS_PER_STROKE: usize = 4_096;

const CORRIDOR_CAP_SEGMENTS: usize = 8;
const CORRIDOR_MITER_LIMIT: f32 = 2.0;
const MAX_CORRIDOR_CENTERLINE_POINTS: usize = 256;

fn signed_area(footprint: &[[f32; 2]]) -> f64 {
    footprint
        .iter()
        .zip(footprint.iter().cycle().skip(1))
        .map(|([ax, az], [bx, bz])| *ax as f64 * *bz as f64 - *bx as f64 * *az as f64)
        .sum::<f64>()
        * 0.5
}

fn orient_2d(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    let ab = [b[0] as f64 - a[0] as f64, b[1] as f64 - a[1] as f64];
    let ac = [c[0] as f64 - a[0] as f64, c[1] as f64 - a[1] as f64];
    ab[0] * ac[1] - ab[1] * ac[0]
}

fn orientation_sign(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> i8 {
    let cross = orient_2d(a, b, c);
    let scale = ((b[0] as f64 - a[0] as f64).hypot(b[1] as f64 - a[1] as f64)
        * (c[0] as f64 - a[0] as f64).hypot(c[1] as f64 - a[1] as f64))
    .max(1.0);
    let epsilon = scale * 1e-10;
    if cross > epsilon {
        1
    } else if cross < -epsilon {
        -1
    } else {
        0
    }
}

fn point_on_segment(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    if orientation_sign(a, b, point) != 0 {
        return false;
    }
    let epsilon = ((b[0] as f64 - a[0] as f64).hypot(b[1] as f64 - a[1] as f64) * 1e-8).max(1e-8);
    let px = point[0] as f64;
    let pz = point[1] as f64;
    px >= (a[0].min(b[0]) as f64 - epsilon)
        && px <= (a[0].max(b[0]) as f64 + epsilon)
        && pz >= (a[1].min(b[1]) as f64 - epsilon)
        && pz <= (a[1].max(b[1]) as f64 + epsilon)
}

fn segments_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let o1 = orientation_sign(a, b, c);
    let o2 = orientation_sign(a, b, d);
    let o3 = orientation_sign(c, d, a);
    let o4 = orientation_sign(c, d, b);
    (o1 != o2 && o1 != 0 && o2 != 0 && o3 != o4 && o3 != 0 && o4 != 0)
        || (o1 == 0 && point_on_segment(c, a, b))
        || (o2 == 0 && point_on_segment(d, a, b))
        || (o3 == 0 && point_on_segment(a, c, d))
        || (o4 == 0 && point_on_segment(b, c, d))
}

fn polygon_is_simple(points: &[[f32; 2]]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let edge_count = points.len();
    for first in 0..edge_count {
        let first_next = (first + 1) % edge_count;
        for second in first + 1..edge_count {
            let second_next = (second + 1) % edge_count;
            // Adjacent edges intentionally share one endpoint, including the
            // closing edge with edge zero.
            if first == second_next || first_next == second {
                continue;
            }
            if segments_intersect(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
            ) {
                return false;
            }
        }
    }
    true
}

fn centerline_is_valid(points: &[[f32; 2]]) -> bool {
    if points.len() < 2 {
        return true;
    }
    for triple in points.windows(3) {
        let incoming = [triple[1][0] - triple[0][0], triple[1][1] - triple[0][1]];
        let outgoing = [triple[2][0] - triple[1][0], triple[2][1] - triple[1][1]];
        let product = incoming[0].hypot(incoming[1]) * outgoing[0].hypot(outgoing[1]);
        let cross = incoming[0] * outgoing[1] - incoming[1] * outgoing[0];
        let dot = incoming[0] * outgoing[0] + incoming[1] * outgoing[1];
        if product > 0.0 && cross.abs() <= product * 1e-5 && dot <= -product * 0.999 {
            return false;
        }
    }
    let segment_count = points.len() - 1;
    for first in 0..segment_count {
        for second in first + 2..segment_count {
            if segments_intersect(
                points[first],
                points[first + 1],
                points[second],
                points[second + 1],
            ) {
                return false;
            }
        }
    }
    true
}

fn push_corridor_join(
    rail: &mut Vec<[f32; 2]>,
    point: [f32; 2],
    previous_normal: [f32; 2],
    next_normal: [f32; 2],
    radius: f32,
    side: f32,
) {
    let previous = [previous_normal[0] * side, previous_normal[1] * side];
    let next = [next_normal[0] * side, next_normal[1] * side];
    let sum = [previous[0] + next[0], previous[1] + next[1]];
    let sum_len = sum[0].hypot(sum[1]);
    if sum_len > 1e-6 {
        let miter = [sum[0] / sum_len, sum[1] / sum_len];
        let denominator = miter[0] * next[0] + miter[1] * next[1];
        if denominator > 1e-4 {
            let length = radius / denominator;
            if length.is_finite() && length <= radius * CORRIDOR_MITER_LIMIT {
                rail.push([point[0] + miter[0] * length, point[1] + miter[1] * length]);
                return;
            }
        }
    }
    rail.push([
        point[0] + previous[0] * radius,
        point[1] + previous[1] * radius,
    ]);
    rail.push([point[0] + next[0] * radius, point[1] + next[1] * radius]);
}

fn simplify_stroke_centerline(points: Vec<[f32; 2]>, tolerance: f32) -> Vec<[f32; 2]> {
    let mut simplified: Vec<[f32; 2]> = Vec::with_capacity(points.len());
    for point in points {
        while simplified.len() >= 2 {
            let previous = simplified[simplified.len() - 2];
            let middle = simplified[simplified.len() - 1];
            let ax = middle[0] - previous[0];
            let az = middle[1] - previous[1];
            let bx = point[0] - middle[0];
            let bz = point[1] - middle[1];
            let baseline = (point[0] - previous[0]).hypot(point[1] - previous[1]);
            let deviation = if baseline > 0.0 {
                (ax * bz - az * bx).abs() / baseline
            } else {
                0.0
            };
            if ax * bx + az * bz >= 0.0 && deviation <= tolerance {
                simplified.pop();
            } else {
                break;
            }
        }
        simplified.push(point);
    }
    simplified
}

fn point_segment_distance_2d(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
    if length_squared <= f32::MIN_POSITIVE {
        return (point[0] - a[0]).hypot(point[1] - a[1]);
    }
    let along = (((point[0] - a[0]) * delta[0] + (point[1] - a[1]) * delta[1]) / length_squared)
        .clamp(0.0, 1.0);
    (point[0] - (a[0] + delta[0] * along)).hypot(point[1] - (a[1] + delta[1] * along))
}

fn bounded_stroke_centerline(points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if points.len() <= MAX_CORRIDOR_CENTERLINE_POINTS {
        return points;
    }

    let mut selected = vec![0usize, points.len() - 1];
    while selected.len() < MAX_CORRIDOR_CENTERLINE_POINTS {
        let mut best: Option<(f32, usize, usize, usize, usize)> = None;
        for (insertion_index, span) in selected.windows(2).enumerate() {
            let start = span[0];
            let end = span[1];
            if end <= start + 1 {
                continue;
            }
            let span_length = end - start;
            let midpoint = start + span_length / 2;
            for index in start + 1..end {
                let deviation =
                    point_segment_distance_2d(points[index], points[start], points[end]);
                let midpoint_distance = index.abs_diff(midpoint);
                let replace = best.is_none_or(
                    |(best_deviation, best_span, best_midpoint_distance, best_index, _)| {
                        let deviation_order = deviation.total_cmp(&best_deviation);
                        deviation_order.is_gt()
                            || (deviation_order.is_eq()
                                && (span_length > best_span
                                    || (span_length == best_span
                                        && (midpoint_distance < best_midpoint_distance
                                            || (midpoint_distance == best_midpoint_distance
                                                && index < best_index)))))
                    },
                );
                if replace {
                    best = Some((
                        deviation,
                        span_length,
                        midpoint_distance,
                        index,
                        insertion_index,
                    ));
                }
            }
        }
        let Some((_, _, _, point_index, insertion_index)) = best else {
            break;
        };
        selected.insert(insertion_index + 1, point_index);
    }

    selected.into_iter().map(|index| points[index]).collect()
}

/// Build one reportable polygon for a complete brush stroke.
pub(crate) fn stroke_corridor_footprint(
    centers: &[[f32; 2]],
    radius: f32,
) -> Option<Vec<[f32; 2]>> {
    if !(radius.is_finite() && radius > 0.0) {
        return None;
    }
    let dedupe_distance = (radius * 1e-4).max(f32::MIN_POSITIVE);
    let mut points = Vec::with_capacity(centers.len().min(MAX_BRUSH_DABS_PER_STROKE));
    for point in centers.iter().take(MAX_BRUSH_DABS_PER_STROKE) {
        if !point[0].is_finite() || !point[1].is_finite() {
            continue;
        }
        if points.last().is_some_and(|last: &[f32; 2]| {
            (point[0] - last[0]).hypot(point[1] - last[1]) <= dedupe_distance
        }) {
            continue;
        }
        points.push(*point);
    }
    let points = bounded_stroke_centerline(simplify_stroke_centerline(points, radius * 0.01));
    if points.is_empty() {
        return None;
    }
    if points.len() == 1 {
        let footprint = brush_disc(points[0], radius);
        return (signed_area(&footprint).is_finite()
            && signed_area(&footprint) != 0.0
            && polygon_is_simple(&footprint))
        .then_some(footprint);
    }
    if !centerline_is_valid(&points) {
        return None;
    }

    let mut normals = Vec::with_capacity(points.len() - 1);
    for pair in points.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dz = pair[1][1] - pair[0][1];
        let length = dx.hypot(dz);
        if !(length.is_finite() && length > 0.0) {
            return None;
        }
        normals.push([-dz / length, dx / length]);
    }

    let mut left = Vec::with_capacity(points.len() * 2);
    let mut right = Vec::with_capacity(points.len() * 2);
    left.push([
        points[0][0] + normals[0][0] * radius,
        points[0][1] + normals[0][1] * radius,
    ]);
    right.push([
        points[0][0] - normals[0][0] * radius,
        points[0][1] - normals[0][1] * radius,
    ]);
    for index in 1..points.len() - 1 {
        push_corridor_join(
            &mut left,
            points[index],
            normals[index - 1],
            normals[index],
            radius,
            1.0,
        );
        push_corridor_join(
            &mut right,
            points[index],
            normals[index - 1],
            normals[index],
            radius,
            -1.0,
        );
    }
    let last_point = points[points.len() - 1];
    let last_normal = normals[normals.len() - 1];
    left.push([
        last_point[0] + last_normal[0] * radius,
        last_point[1] + last_normal[1] * radius,
    ]);
    right.push([
        last_point[0] - last_normal[0] * radius,
        last_point[1] - last_normal[1] * radius,
    ]);

    let mut footprint = left;
    let end_angle = last_normal[1].atan2(last_normal[0]);
    for step in 1..CORRIDOR_CAP_SEGMENTS {
        let angle = end_angle - std::f32::consts::PI * step as f32 / CORRIDOR_CAP_SEGMENTS as f32;
        footprint.push([
            last_point[0] + angle.cos() * radius,
            last_point[1] + angle.sin() * radius,
        ]);
    }
    footprint.extend(right.iter().rev().copied());
    let first_point = points[0];
    let first_normal = normals[0];
    let start_angle = (-first_normal[1]).atan2(-first_normal[0]);
    for step in 1..CORRIDOR_CAP_SEGMENTS {
        let angle = start_angle - std::f32::consts::PI * step as f32 / CORRIDOR_CAP_SEGMENTS as f32;
        footprint.push([
            first_point[0] + angle.cos() * radius,
            first_point[1] + angle.sin() * radius,
        ]);
    }
    let area = signed_area(&footprint);
    if !area.is_finite() || area == 0.0 || footprint.len() < 3 || !polygon_is_simple(&footprint) {
        return None;
    }
    if area < 0.0 {
        footprint.reverse();
    }
    Some(footprint)
}

/// Persist a distance-sampled brush stroke as one terrain document update,
/// then create one addressable endpoint node for its swept corridor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_brush_stroke(
    db_sender: &DbCommandSender,
    petal_map: &mut PetalMapState,
    sculpt_state: &mut SculptToolState,
    earthwork_map: &mut EarthworkNodeMap,
    petal_id: String,
    centers: Vec<[f32; 2]>,
    radius: f32,
    strength: f32,
    op: String,
    target_height: Option<f32>,
    delta: Option<f32>,
    material: String,
) {
    if !petal_map_enabled(petal_map, &petal_id) {
        bevy::log::warn!("SculptBrushStroke ignored: active petal has no enabled map");
        return;
    }
    if centers.is_empty() || !(radius.is_finite() && radius > 0.0) {
        bevy::log::warn!("SculptBrushStroke ignored: empty or degenerate stroke");
        return;
    }
    let strength = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let target_height = target_height.filter(|value| value.is_finite());
    let delta = delta.filter(|value| value.is_finite());
    let (target_height, delta) = match op.as_str() {
        "level" => {
            let Some(target) = target_height else {
                bevy::log::warn!(
                    "SculptBrushStroke ignored: Level requires a finite target height"
                );
                return;
            };
            (Some(target), Some(strength))
        }
        "raise" | "lower" => {
            let Some(magnitude) = delta else {
                bevy::log::warn!("SculptBrushStroke ignored: Raise/Lower requires a finite delta");
                return;
            };
            (None, Some(magnitude.abs() * strength))
        }
        "smooth" => (None, Some(strength)),
        _ => {
            bevy::log::warn!("SculptBrushStroke ignored: unsupported operation {op}");
            return;
        }
    };
    let material = if material.trim().is_empty() {
        "earth"
    } else {
        material.trim()
    };
    let Some(footprint) = stroke_corridor_footprint(&centers, radius) else {
        bevy::log::warn!("SculptBrushStroke ignored: no finite corridor footprint");
        return;
    };
    let id = mint_unused_region_id(sculpt_state, petal_map.terrain_json.as_ref());
    let region = region_json(&id, &op, &footprint, target_height, delta, material);
    let terrain = embed_region(petal_map.terrain_json.as_ref(), region);
    if persist_doc(db_sender, petal_map, terrain, petal_id.clone()) {
        create_region_node(
            db_sender,
            earthwork_map,
            &petal_id,
            &id,
            &op,
            &footprint,
            material,
        );
    }
}

/// T3 FR-1 shape + FR-3 region + FR-4 volume: create a defined-shape earthwork
/// region record (the reportable BIM node, D-A8). Persisted in the `proposals`
/// block enriched with `material`; the report derives cut/fill volume.
///
/// Finding #11 fix (`ui_semantics_unification_20260808`): the caller
/// (`panels::tool_options::render_sculpt_shape_picker`) queues
/// `footprint`/`target_height`/`delta` in petal-local METERS (N-1, same
/// convention `SculptToolState`'s fields document), but every persisted
/// `terrain.proposals` entry — Brush regions (`BrushSnapshot::from_state`'s
/// world-unit snapshot), palette proposals — and every downstream consumer
/// (context-menu PNPOLY pick, report cut/fill totals) assumes WORLD UNITS.
/// This handler converts at the boundary via `PetalMapState.world_scale`,
/// mirroring `BrushSnapshot::from_state`'s conversion exactly, so the
/// persisted record is honest world units like everything else in the array.
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
    let world_scale = petal_map.world_scale;
    let footprint: Vec<[f32; 2]> = footprint
        .iter()
        .map(|[x, z]| {
            [
                meters_to_world(*x, world_scale),
                meters_to_world(*z, world_scale),
            ]
        })
        .collect();
    let target_height = target_height.map(|t| meters_to_world(t, world_scale));
    let delta = delta.map(|d| meters_to_world(d, world_scale));
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
    if !(radius.is_finite() && radius > 0.0 && center[0].is_finite() && center[1].is_finite()) {
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

    /// Empty tombstone set — the common case for tests not exercising the
    /// finding #13 fix.
    fn no_tombstones() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
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
        let out = embed_proposals(Some(&base), &[record("p1")], true, &no_tombstones());
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
        let out = embed_proposals(None, &[record("p1")], true, &no_tombstones());
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
        let out = embed_proposals(
            Some(&json!([1, 2, 3])),
            &[record("p1")],
            true,
            &no_tombstones(),
        );
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
        let out = embed_proposals(None, &[], true, &no_tombstones());
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
        let out = embed_proposals(
            Some(&json!({ "enabled": true })),
            &[],
            true,
            &no_tombstones(),
        );
        assert_eq!(out["proposals"], json!([]));
        assert_eq!(out["enabled"], json!(true));
    }

    // --- Data-loss guard: unhydrated embed is read-modify-write, not overwrite
    // (ui_semantics_unification_20260808 finding #1) ---

    #[test]
    fn embed_proposals_unhydrated_preserves_persisted_entries_the_mirror_never_saw() {
        // A doc already has "p1" persisted from a prior session; this
        // session's mirror never loaded it (still unhydrated) but the user
        // added a brand-new local record "p2" before the load response
        // arrived. The persisted entry must survive.
        let base = json!({
            "enabled": true,
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 5.0 }
            ],
        });
        let out = embed_proposals(Some(&base), &[record("p2")], false, &no_tombstones());
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert!(
            ids.contains(&"p1"),
            "persisted-but-unloaded proposal survives"
        );
        assert!(
            ids.contains(&"p2"),
            "the new local record is still appended"
        );
        assert_eq!(ids.len(), 2);
        assert_eq!(out["enabled"], json!(true), "config untouched");
    }

    #[test]
    fn embed_proposals_unhydrated_does_not_duplicate_an_id_already_in_the_doc() {
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 }
            ],
        });
        let out = embed_proposals(Some(&base), &[record("p1")], false, &no_tombstones());
        assert_eq!(
            out["proposals"].as_array().unwrap().len(),
            1,
            "the mirror's record and the doc's record share an id — no duplicate"
        );
    }

    #[test]
    fn embed_proposals_unhydrated_with_no_base_still_appends_the_local_record() {
        // A brand-new/map-less petal (no base doc at all) is not a "lose
        // data" case — there is nothing persisted to preserve — but the RMW
        // path must still land the local record.
        let out = embed_proposals(None, &[record("p1")], false, &no_tombstones());
        assert_eq!(out["proposals"][0]["id"], json!("p1"));
    }

    #[test]
    fn embed_proposals_hydrated_replaces_wholesale_reflecting_deletions() {
        // Once hydrated, the mirror IS authoritative — a record dropped from
        // the local set (e.g. via `ProposalEditState::remove`) must actually
        // disappear from the doc, not just fail to be re-added.
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 },
                { "id": "p2", "op": "raise", "footprint": [], "delta": 1.0 }
            ],
        });
        let out = embed_proposals(Some(&base), &[record("p1")], true, &no_tombstones());
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["p1"],
            "p2 was deleted from the hydrated mirror — it's gone"
        );
    }

    // --- Finding #1 fix: hydrated embed is a SHAPE-PRESERVING merge — a
    // material-tagged region entry must survive ANY sequence of palette
    // add/delete, hydrated or not ---

    #[test]
    fn embed_proposals_hydrated_preserves_material_tagged_region_entries_untouched() {
        let base = json!({
            "enabled": true,
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 },
                { "id": "r1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 3.0, "material": "gravel" }
            ],
        });
        // The hydrated mirror only ever holds palette entries (finding #1's
        // hydration filter) — here it still has "p1" and adds nothing new.
        let out = embed_proposals(Some(&base), &[record("p1")], true, &no_tombstones());
        let region = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!("r1"))
            .expect("region entry survives a hydrated palette merge");
        assert_eq!(
            region["material"],
            json!("gravel"),
            "region untouched byte-for-byte"
        );
        assert_eq!(region["op"], json!("raise"));
        assert_eq!(region["delta"], json!(3.0));
    }

    #[test]
    fn embed_proposals_hydrated_palette_delete_never_removes_a_region_entry() {
        // The mirror "deletes" p1 by simply not including it in `records`
        // (mirrors `ProposalEditState::remove` then `persist`). The
        // material-tagged region must remain even though it's the ONLY
        // entry left in the doc's proposals array.
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 },
                { "id": "r1", "op": "level", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "target_height": 4.0, "material": "earth" }
            ],
        });
        let out = embed_proposals(Some(&base), &[], true, &no_tombstones());
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["r1"],
            "p1 deleted from the palette; r1 (region) survives"
        );
    }

    #[test]
    fn embed_proposals_no_sequence_of_palette_operations_alters_a_region_entry() {
        // Invariant sweep (finding #1): add, then delete, then add-again on
        // the palette side — the region entry must be byte-identical at
        // every step.
        let region = json!({ "id": "r1", "op": "lower", "footprint": [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]], "delta": 1.5, "material": "sand" });
        let base = json!({ "proposals": [region.clone()] });

        let step1 = embed_proposals(Some(&base), &[record("p1")], true, &no_tombstones());
        let step2 = embed_proposals(
            Some(&step1),
            &[record("p1"), record("p2")],
            true,
            &no_tombstones(),
        );
        let step3 = embed_proposals(Some(&step2), &[], true, &no_tombstones());
        let step4 = embed_proposals(Some(&step3), &[record("p3")], true, &no_tombstones());

        for (label, doc) in [
            ("step1", &step1),
            ("step2", &step2),
            ("step3", &step3),
            ("step4", &step4),
        ] {
            let found = doc["proposals"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == json!("r1"))
                .unwrap_or_else(|| panic!("{label}: region entry missing"));
            assert_eq!(
                *found, region,
                "{label}: region entry must be byte-identical"
            );
        }
    }

    // --- Finding #13 fix: unhydrated merge is tombstone-aware ---

    #[test]
    fn embed_proposals_unhydrated_tombstoned_id_is_excluded_even_though_doc_has_it() {
        // A pre-hydration delete of an already-persisted id must not be
        // resurrected by the "keep every existing entry" union rule.
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 }
            ],
        });
        let mut tombstones = std::collections::HashSet::new();
        tombstones.insert("p1".to_string());
        let out = embed_proposals(Some(&base), &[], false, &tombstones);
        assert!(
            out["proposals"].as_array().unwrap().is_empty(),
            "tombstoned id is dropped, not resurrected"
        );
    }

    #[test]
    fn embed_proposals_unhydrated_prefers_mirror_version_for_a_touched_id() {
        // A locally-minted id happens to collide with an already-persisted
        // one (e.g. a fresh `ProposalEditState` re-mints "p1"). The mirror's
        // version must win, not be silently discarded.
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [[9.0, 9.0]], "delta": 99.0 }
            ],
        });
        let local = record("p1"); // footprint/delta differ from the doc's "p1".
        let out = embed_proposals(
            Some(&base),
            std::slice::from_ref(&local),
            false,
            &no_tombstones(),
        );
        let entries = out["proposals"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "still one entry for id p1 — no duplicate");
        assert_eq!(
            entries[0]["delta"],
            json!(local.delta.unwrap()),
            "mirror's version wins"
        );
    }

    #[test]
    fn embed_proposals_unhydrated_palette_delete_never_touches_a_region_entry() {
        // Same invariant as the hydrated tests above, exercised on the
        // UNHYDRATED path: a pre-hydration palette delete (tombstoning "p1")
        // must never remove or alter the material-tagged region entry, even
        // though both live in the same `proposals` array.
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 },
                { "id": "r1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 3.0, "material": "earth" }
            ],
        });
        let mut tombstones = std::collections::HashSet::new();
        tombstones.insert("p1".to_string());
        // Records is empty — the palette mirror never held regions to begin
        // with (finding #1's hydration filter / regions never enter push_new).
        let out = embed_proposals(Some(&base), &[], false, &tombstones);
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["r1"], "p1 tombstoned away; r1 (region) untouched");
        let region = &out["proposals"][0];
        assert_eq!(region["material"], json!("earth"));
        assert_eq!(region["delta"], json!(3.0));
    }

    #[test]
    fn embed_proposals_unhydrated_tombstone_does_not_affect_other_ids() {
        let base = json!({
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [], "delta": 1.0 },
                { "id": "p2", "op": "raise", "footprint": [], "delta": 2.0 }
            ],
        });
        let mut tombstones = std::collections::HashSet::new();
        tombstones.insert("p1".to_string());
        let out = embed_proposals(Some(&base), &[], false, &tombstones);
        let ids: Vec<&str> = out["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["p2"], "only the tombstoned id is dropped");
    }

    #[test]
    fn add_before_hydration_does_not_destroy_persisted_proposals() {
        // End-to-end reproduction of finding #1 through the real `add` action:
        // the petal's terrain doc already has a persisted proposal, but this
        // session's `ProposalEditState` was never rehydrated (e.g. a race
        // with `PetalTerrainLoaded`). The first Add must not wipe it.
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("petal-1".into()),
            terrain_json: Some(json!({
                "enabled": true,
                "proposals": [
                    { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 3.0 }
                ],
            })),
            ..Default::default()
        };
        let mut proposals = ProposalEditState::default();
        assert!(
            !proposals.hydrated,
            "not rehydrated — the exact bug scenario"
        );

        add(
            &sender,
            &mut petal_map,
            &mut proposals,
            Some("petal-1".into()),
            ProposalOp::Lower,
            vec![[2.0, 2.0], [3.0, 2.0], [3.0, 3.0]],
            None,
            Some(-1.0),
        );

        let doc = match rx.try_recv().expect("terrain write") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected command: {other:?}"),
        };
        let ids: Vec<&str> = doc["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert!(
            ids.contains(&"p1"),
            "persisted proposal survives the unhydrated Add"
        );
        assert_eq!(ids.len(), 2, "plus the newly added one");
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
    fn stroke_corridor_single_point_reuses_disc() {
        let corridor = stroke_corridor_footprint(&[[5.0, -3.0]], 2.0).unwrap();
        assert_eq!(corridor, brush_disc([5.0, -3.0], 2.0));
        assert!(signed_area(&corridor) > 0.0);
    }

    #[test]
    fn stroke_corridor_straight_segment_has_round_caps_and_positive_area() {
        let corridor = stroke_corridor_footprint(&[[0.0, 0.0], [10.0, 0.0]], 2.0).unwrap();
        assert!(corridor.len() >= 18);
        assert!(signed_area(&corridor) > 0.0);
        let min_x = corridor
            .iter()
            .map(|point| point[0])
            .fold(f32::MAX, f32::min);
        let max_x = corridor
            .iter()
            .map(|point| point[0])
            .fold(f32::MIN, f32::max);
        assert!((min_x + 2.0).abs() < 1e-4);
        assert!((max_x - 12.0).abs() < 1e-4);
    }

    #[test]
    fn stroke_corridor_filters_bad_and_duplicate_samples() {
        let corridor = stroke_corridor_footprint(
            &[
                [0.0, 0.0],
                [0.0, 0.0],
                [f32::NAN, 1.0],
                [5.0, 0.0],
                [5.0, 5.0],
            ],
            1.0,
        )
        .unwrap();
        assert!(corridor.iter().flatten().all(|value| value.is_finite()));
        assert!(signed_area(&corridor) > 0.0);
        assert!(stroke_corridor_footprint(&[[f32::NAN, 0.0]], 1.0).is_none());
        assert!(stroke_corridor_footprint(&[[0.0, 0.0]], 0.0).is_none());
    }

    #[test]
    fn stroke_corridor_rejects_figure_eight_centerline() {
        assert!(
            stroke_corridor_footprint(&[[0.0, 0.0], [4.0, 4.0], [0.0, 4.0], [4.0, 0.0]], 0.25,)
                .is_none()
        );
    }

    #[test]
    fn stroke_corridor_rejects_closed_loop_centerline() {
        assert!(stroke_corridor_footprint(
            &[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0],],
            0.25,
        )
        .is_none());
    }

    #[test]
    fn stroke_corridor_rejects_collinear_reversal() {
        assert!(stroke_corridor_footprint(&[[0.0, 0.0], [4.0, 0.0], [1.0, 0.0]], 0.25).is_none());
    }

    #[test]
    fn stroke_corridor_normal_turn_remains_simple() {
        let footprint = stroke_corridor_footprint(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]], 0.25)
            .expect("right-angle stroke");
        assert!(polygon_is_simple(&footprint));
        assert!(signed_area(&footprint) > 0.0);
    }

    #[test]
    fn stroke_corridor_bounds_high_sample_centerline_and_output() {
        let samples: Vec<[f32; 2]> = (0..MAX_BRUSH_DABS_PER_STROKE)
            .map(|index| [index as f32, (index as f32 * 0.001).sin()])
            .collect();
        let bounded = bounded_stroke_centerline(samples.clone());
        assert!(bounded.len() <= MAX_CORRIDOR_CENTERLINE_POINTS);
        assert_eq!(bounded.first(), samples.first());
        assert_eq!(bounded.last(), samples.last());
        let footprint = stroke_corridor_footprint(&samples, 0.1).expect("bounded corridor");
        assert!(footprint.len() <= MAX_CORRIDOR_CENTERLINE_POINTS * 4 + 16);
    }

    #[test]
    fn bounded_centerline_retains_equal_amplitude_zigzag_turns() {
        let mut samples: Vec<[f32; 2]> = (0..MAX_BRUSH_DABS_PER_STROKE)
            .map(|index| [index as f32, if index % 2 == 0 { 1.0 } else { -1.0 }])
            .collect();
        samples[0][1] = 0.0;
        samples[MAX_BRUSH_DABS_PER_STROKE - 1][1] = 0.0;

        let bounded = bounded_stroke_centerline(samples);
        assert_eq!(bounded.len(), MAX_CORRIDOR_CENTERLINE_POINTS);
        assert!(bounded.iter().filter(|point| point[1] > 0.5).count() > 48);
        assert!(bounded.iter().filter(|point| point[1] < -0.5).count() > 48);
        assert!(
            bounded
                .windows(3)
                .filter(|turn| orient_2d(turn[0], turn[1], turn[2]).abs() > 0.5)
                .count()
                > MAX_CORRIDOR_CENTERLINE_POINTS / 4
        );
    }

    #[test]
    fn bounded_centerline_preserves_localized_turn_extremum() {
        let mut samples: Vec<[f32; 2]> = (0..MAX_BRUSH_DABS_PER_STROKE)
            .map(|index| [index as f32, 0.0])
            .collect();
        samples[MAX_BRUSH_DABS_PER_STROKE / 2][1] = 50.0;
        let bounded = bounded_stroke_centerline(samples);
        assert!(bounded.len() <= MAX_CORRIDOR_CENTERLINE_POINTS);
        assert!(bounded.iter().any(|point| point[1] >= 50.0));
    }

    #[test]
    fn bounded_centerline_preserves_localized_self_crossing_for_rejection() {
        let mut samples = Vec::new();
        for index in 0..1_500 {
            samples.push([-100.0 + index as f32 * (100.0 / 1_500.0), 0.0]);
        }
        let loop_vertices = [
            [0.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [10.0, 0.0],
            [20.0, 0.0],
        ];
        for edge in loop_vertices.windows(2) {
            for index in 0..200 {
                let t = index as f32 / 200.0;
                samples.push([
                    edge[0][0] + (edge[1][0] - edge[0][0]) * t,
                    edge[0][1] + (edge[1][1] - edge[0][1]) * t,
                ]);
            }
        }
        while samples.len() < MAX_BRUSH_DABS_PER_STROKE {
            let t = (samples.len() - 2_300) as f32 / (MAX_BRUSH_DABS_PER_STROKE - 2_300) as f32;
            samples.push([20.0 + 80.0 * t, 0.0]);
        }
        let bounded = bounded_stroke_centerline(samples.clone());
        assert!(bounded.len() <= MAX_CORRIDOR_CENTERLINE_POINTS);
        assert!(!centerline_is_valid(&bounded));
        assert!(stroke_corridor_footprint(&samples, 0.1).is_none());
    }

    #[test]
    fn sculpt_state_mints_monotonic_ids() {
        let mut s = SculptToolState::default();
        assert_eq!(s.mint_region_id(), "r1");
        assert_eq!(s.mint_region_id(), "r2");
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
    fn brush_commit_persists_region_and_creates_endpoint_node() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("petal-1".into()),
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        let mut sculpt = SculptToolState::default();
        let mut earthwork = EarthworkNodeMap::default();
        handle_brush_stroke(
            &sender,
            &mut petal_map,
            &mut sculpt,
            &mut earthwork,
            "petal-1".into(),
            vec![[2.0, 3.0]],
            4.0,
            0.25,
            "raise".into(),
            None,
            Some(8.0),
            "soil".into(),
        );
        match rx.try_recv().expect("terrain write") {
            DbCommand::SetPetalTerrain {
                petal_id,
                terrain: Some(doc),
            } => {
                assert_eq!(petal_id, "petal-1");
                assert_eq!(doc["proposals"][0]["delta"], json!(2.0));
                assert_eq!(doc["proposals"][0]["material"], json!("soil"));
            }
            other => panic!("unexpected first brush command: {other:?}"),
        }
        assert!(matches!(
            rx.try_recv().expect("endpoint node"),
            DbCommand::CreateNode {
                petal_id,
                correlation_id: Some(_),
                ..
            } if petal_id == "petal-1"
        ));
    }

    #[test]
    fn brush_stroke_commits_one_document_and_one_corridor_node() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("petal-1".into()),
            world_scale: 0.001,
            terrain_json: Some(json!({
                "enabled": true,
                "world_scale": 0.001,
                "proposals": []
            })),
            ..Default::default()
        };
        handle_brush_stroke(
            &sender,
            &mut petal_map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "petal-1".into(),
            vec![[0.0, 0.0], [0.001, 0.0], [0.002, 0.0]],
            0.002,
            0.5,
            "raise".into(),
            None,
            Some(0.004),
            "soil".into(),
        );

        let doc = match rx.try_recv().expect("single terrain write first") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected first stroke command: {other:?}"),
        };
        let proposals = doc["proposals"].as_array().expect("proposal array");
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0]["footprint"].as_array().unwrap().len() >= 18);
        assert!((proposals[0]["delta"].as_f64().unwrap() - 0.002).abs() < 1e-7);

        let followups: Vec<_> = rx.try_iter().collect();
        assert_eq!(followups.len(), 1);
        assert!(matches!(followups[0], DbCommand::CreateNode { .. }));
    }

    #[test]
    fn brush_stroke_handler_caps_untrusted_sample_batches() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("p".into()),
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        handle_brush_stroke(
            &sender,
            &mut petal_map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "p".into(),
            (0..MAX_BRUSH_DABS_PER_STROKE + 7)
                .map(|index| [index as f32, 0.0])
                .collect(),
            1.0,
            1.0,
            "raise".into(),
            None,
            Some(1.0),
            "earth".into(),
        );
        let doc = match rx.try_recv().expect("one terrain write") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected first stroke command: {other:?}"),
        };
        assert_eq!(doc["proposals"].as_array().unwrap().len(), 1);
        let max_x = doc["proposals"][0]["footprint"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|point| point[0].as_f64())
            .fold(f64::MIN, f64::max);
        assert!(max_x <= MAX_BRUSH_DABS_PER_STROKE as f64 + 1e-4);
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn brush_level_and_smooth_parameters_are_truthful() {
        let run = |op: &str, strength: f32, target_height: Option<f32>| {
            let (tx, _rx) = crossbeam::channel::unbounded();
            let sender = DbCommandSender(tx);
            let mut petal_map = PetalMapState {
                petal_id: Some("p".into()),
                terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
                ..Default::default()
            };
            handle_brush_stroke(
                &sender,
                &mut petal_map,
                &mut SculptToolState::default(),
                &mut EarthworkNodeMap::default(),
                "p".into(),
                vec![[0.0, 0.0]],
                1.0,
                strength,
                op.into(),
                target_height,
                Some(99.0),
                "earth".into(),
            );
            petal_map.terrain_json.unwrap()["proposals"][0].clone()
        };
        let level = run("level", 0.2, Some(12.0));
        assert_eq!(level["target_height"], json!(12.0));
        assert!((level["delta"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        let smooth = run("smooth", 0.35, None);
        assert!((smooth["delta"].as_f64().unwrap() - 0.35).abs() < 1e-6);
        assert!(smooth.get("target_height").is_none());
    }

    #[test]
    fn brush_without_enabled_map_is_a_noop() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut map = PetalMapState {
            petal_id: Some("p".into()),
            terrain_json: Some(json!({ "enabled": false })),
            ..Default::default()
        };
        handle_brush_stroke(
            &DbCommandSender(tx),
            &mut map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "p".into(),
            vec![[0.0, 0.0]],
            1.0,
            1.0,
            "raise".into(),
            None,
            Some(1.0),
            "earth".into(),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn brush_commit_bind_and_volume_gate_compose_without_repeat_writes() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut map = PetalMapState {
            petal_id: Some("p".into()),
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        let mut earthwork = EarthworkNodeMap::default();
        handle_brush_stroke(
            &sender,
            &mut map,
            &mut SculptToolState::default(),
            &mut earthwork,
            "p".into(),
            vec![[2.0, 3.0]],
            1.0,
            1.0,
            "raise".into(),
            None,
            Some(2.0),
            "earth".into(),
        );
        assert!(matches!(
            rx.try_recv().expect("terrain document"),
            DbCommand::SetPetalTerrain { .. }
        ));
        let correlation_id = match rx.try_recv().expect("correlated endpoint") {
            DbCommand::CreateNode {
                correlation_id: Some(correlation_id),
                ..
            } => correlation_id,
            other => panic!("unexpected endpoint command: {other:?}"),
        };
        let region_id = earthwork_region_id_from_correlation(&correlation_id)
            .expect("earthwork correlation")
            .to_string();
        earthwork.record(&region_id, "node-1");

        let report = fe_renderer::terrain_overlay::EarthworkVolumeReport {
            petal_id: "p".into(),
            region_id,
            cut_m3: 4.0,
            fill_m3: 9.0,
        };
        persist_earthwork_volume_report(&report, &mut earthwork, &sender);
        let writes: Vec<_> = rx.try_iter().collect();
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|command| matches!(
            command,
            DbCommand::SetNodeProperty { node_id, .. } if node_id == "node-1"
        )));

        persist_earthwork_volume_report(&report, &mut earthwork, &sender);
        assert!(rx.try_recv().is_err(), "unchanged report must be gated");
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
    fn mint_unused_region_id_skips_rehydrated_ids() {
        let mut s = SculptToolState::default();
        // A reloaded doc already holds r1/r2 from a previous session.
        let terrain = json!({ "proposals": [ { "id": "r1" }, { "id": "r2" } ] });
        assert_eq!(mint_unused_region_id(&mut s, Some(&terrain)), "r3");
        assert_eq!(mint_unused_region_id(&mut s, None), "r4");
    }

    // --- Finding #11 fix: `handle_shape_region` converts petal-local meters
    // to world units via `PetalMapState.world_scale` before persisting ---

    #[test]
    fn shape_region_converts_footprint_and_deltas_meters_to_world_units_at_nondefault_scale() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("p".into()),
            world_scale: 0.01, // 0.01 world units per real meter
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        handle_shape_region(
            &sender,
            &mut petal_map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "p".into(),
            vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]], // petal-local meters
            "raise".into(),
            None,
            Some(50.0), // meters
            "earth".into(),
        );
        let doc = match rx.try_recv().expect("terrain write") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected command: {other:?}"),
        };
        let footprint = doc["proposals"][0]["footprint"].as_array().unwrap();
        // 100 meters * 0.01 world units/meter = 1.0 world unit.
        assert!((footprint[1][0].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert!((footprint[2][1].as_f64().unwrap() - 1.0).abs() < 1e-6);
        // delta: 50 meters * 0.01 world units/meter = 0.5 world units.
        assert!((doc["proposals"][0]["delta"].as_f64().unwrap() - 0.5).abs() < 1e-6);

        // The endpoint node's centroid must also be in the converted
        // (world-unit) footprint, not the raw meters one.
        let node_position = match rx.try_recv().expect("endpoint node") {
            DbCommand::CreateNode { position, .. } => position,
            other => panic!("unexpected endpoint command: {other:?}"),
        };
        assert!((node_position[0] - (200.0 / 3.0 * 0.01)).abs() < 1e-4);
    }

    #[test]
    fn shape_region_target_height_also_converts_to_world_units() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("p".into()),
            world_scale: 2.0, // 2 world units per real meter
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        handle_shape_region(
            &sender,
            &mut petal_map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "p".into(),
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            "level".into(),
            Some(10.0), // meters
            None,
            "earth".into(),
        );
        let doc = match rx.try_recv().expect("terrain write") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected command: {other:?}"),
        };
        // 10 meters * 2 world units/meter = 20 world units.
        assert!((doc["proposals"][0]["target_height"].as_f64().unwrap() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn shape_region_at_default_scale_is_numerically_unchanged() {
        // world_scale == 1.0 must behave as an identity conversion — a
        // sanity guard against a regression that shifts values even at the
        // most common (unscaled) case.
        let (tx, rx) = crossbeam::channel::unbounded();
        let sender = DbCommandSender(tx);
        let mut petal_map = PetalMapState {
            petal_id: Some("p".into()),
            world_scale: 1.0,
            terrain_json: Some(json!({ "enabled": true, "proposals": [] })),
            ..Default::default()
        };
        handle_shape_region(
            &sender,
            &mut petal_map,
            &mut SculptToolState::default(),
            &mut EarthworkNodeMap::default(),
            "p".into(),
            vec![[0.0, 0.0], [3.0, 0.0], [3.0, 3.0]],
            "raise".into(),
            None,
            Some(4.0),
            "earth".into(),
        );
        let doc = match rx.try_recv().expect("terrain write") {
            DbCommand::SetPetalTerrain {
                terrain: Some(doc), ..
            } => doc,
            other => panic!("unexpected command: {other:?}"),
        };
        assert_eq!(
            doc["proposals"][0]["footprint"],
            json!([[0.0, 0.0], [3.0, 0.0], [3.0, 3.0]])
        );
        assert_eq!(doc["proposals"][0]["delta"], json!(4.0));
    }
}
