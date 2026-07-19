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
/// A `None`/non-object base yields a proposals-only document. Pure so the
/// additive-merge contract (NFR-1: never clobber tileset config) is testable.
pub(crate) fn embed_proposals(
    base: Option<&serde_json::Value>,
    records: &[ProposalRecord],
) -> serde_json::Value {
    let mut doc = match base {
        Some(v @ serde_json::Value::Object(_)) => v.clone(),
        _ => serde_json::json!({}),
    };
    doc["proposals"] = crate::terrain_proposal_state::to_json(records);
    doc
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
    fn embed_none_base_yields_proposals_only_doc() {
        let out = embed_proposals(None, &[record("p1")]);
        assert!(out.is_object());
        assert_eq!(out["proposals"][0]["id"], json!("p1"));
    }

    #[test]
    fn embed_empty_records_writes_empty_array() {
        let out = embed_proposals(Some(&json!({ "enabled": true })), &[]);
        assert_eq!(out["proposals"], json!([]));
        assert_eq!(out["enabled"], json!(true));
    }
}
