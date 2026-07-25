//! Terrain-proposal action handling (terrain_editor_overhaul FR-5): add/delete
//! proposed-overlay records and persist the whole set ADDITIVELY under the petal
//! terrain config's `proposals` key — without clobbering the tileset/layer
//! config. Mirrors `actions::gis::set_layer`'s "mutate one field of the stored
//! terrain JSON, then round-trip via SetPetalTerrain" idiom. See
//! `fe-ui/src/AGENTS.md` §terrain-proposal-editor.

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
        assert!(out.get("tile_source_url").and_then(|v| v.as_str()).is_some());
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
}
