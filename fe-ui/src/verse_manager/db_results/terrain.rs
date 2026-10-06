//! Handler for per-petal terrain docs feeding the map picker + rehydrating the
//! terrain-proposal mirror (data-loss guard). See ../AGENTS.md §db-results.

use crate::navigation_manager::NavigationManager;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalEditState;

/// `PetalTerrainLoaded`: only the active petal's terrain drives the map
/// picker state AND rehydrates `ProposalEditState` from the doc's
/// `proposals` array — the data-loss guard fix (`ui_semantics_unification_
/// 20260808` finding #1): before this runs, the mirror is empty/stale and
/// `actions::terrain_proposal::embed_proposals` must not trust it wholesale.
/// Only palette-shaped entries (no `material` key) are absorbed into the
/// mirror; material-tagged Brush/shape regions are left for
/// `earthwork_regions_from_terrain`/`embed_proposals`'s region pass-through
/// to handle — see the filter below.
pub(super) fn handle_petal_terrain_loaded(
    petal_id: &str,
    terrain: &Option<serde_json::Value>,
    nav: &NavigationManager,
    petal_map: &mut PetalMapState,
    proposal_state: &mut ProposalEditState,
) {
    if nav.active_petal_id.as_deref() != Some(petal_id) {
        return;
    }
    petal_map.petal_id = Some(petal_id.to_string());
    petal_map.tileset_ids = terrain
        .as_ref()
        .and_then(|t| t.get("tileset_hexon_uris"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Restore the stored world scale (drives the settings slider + camera).
    petal_map.world_scale = terrain
        .as_ref()
        .and_then(|t| t.get("world_scale"))
        .and_then(|v| v.as_f64())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    // Hexon-authoritative clamp bounds (scale orchestration track); see fe-ui/src/verse_manager/AGENTS.md.
    petal_map.scale_bounds = terrain
        .as_ref()
        .and_then(|t| t.get("scale_bounds"))
        .and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok());
    // Keep the raw doc for the GIS Layer Manager's mutate-and-round-trip flow.
    petal_map.terrain_json = terrain.clone();
    petal_map.loaded = true;
    // Rehydrate the palette mirror from whatever `proposals` the doc actually
    // holds — an absent/malformed array rehydrates to empty (still marks
    // `hydrated`, see `ProposalEditState::replace_all`), never panics.
    //
    // Finding #1 fix (`ui_semantics_unification_20260808`): the doc's
    // `proposals` array holds TWO record shapes — palette `ProposalRecord`s
    // and Brush/shape-tool earthwork regions (tagged by a `material` key;
    // `actions::terrain_proposal::region_json`'s shape). Region entries must
    // be filtered out BEFORE `ProposalOp` parsing — parsing them used to
    // either (a) accidentally succeed for `raise`/`lower` (a valid
    // `ProposalOp` tag), absorbing the region into the palette mirror and
    // stripping its `material` tag, or (b) silently fail and drop the
    // record entirely for `level`/`smooth` (not valid `ProposalOp` variants).
    // The `material` key is the reliable structural marker (mirrors
    // `proposal_report_panel::earthwork_regions_from_terrain`'s discriminator)
    // — skipping it here means region entries never enter the mirror at all,
    // so `embed_proposals`'s hydrated shape-preserving merge can pass them
    // through untouched.
    let proposals = terrain
        .as_ref()
        .and_then(|t| t.get("proposals"))
        .and_then(|v| v.as_array())
        .map(|items| {
            let palette_only: Vec<serde_json::Value> = items
                .iter()
                .filter(|item| item.get("material").is_none())
                .cloned()
                .collect();
            crate::terrain_proposal_state::from_json(&serde_json::Value::Array(palette_only))
        })
        .unwrap_or_default();
    proposal_state.replace_all(proposals);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;
    use crate::terrain_proposal_state::ProposalOp;
    use serde_json::json;

    #[test]
    fn rehydrates_proposal_mirror_on_load_for_active_petal() {
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut petal_map = PetalMapState::default();
        let mut proposals = ProposalEditState::default();
        let terrain = Some(json!({
            "enabled": true,
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 2.0 }
            ],
        }));

        handle_petal_terrain_loaded("p1", &terrain, &nav, &mut petal_map, &mut proposals);

        assert!(proposals.hydrated, "load marks the mirror trustworthy");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(proposals.proposals[0].id, "p1");
        assert_eq!(proposals.proposals[0].op, ProposalOp::Raise);
    }

    #[test]
    fn rehydrates_to_empty_and_still_marks_hydrated_for_a_proposal_less_doc() {
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut petal_map = PetalMapState::default();
        let mut proposals = ProposalEditState::default();

        // No `proposals` key at all, and a `None` doc — both are legitimate
        // "nothing persisted yet" states, not malformed ones.
        handle_petal_terrain_loaded(
            "p1",
            &Some(json!({ "enabled": true })),
            &nav,
            &mut petal_map,
            &mut proposals,
        );
        assert!(proposals.hydrated);
        assert!(proposals.proposals.is_empty());

        let mut proposals2 = ProposalEditState::default();
        handle_petal_terrain_loaded("p1", &None, &nav, &mut petal_map, &mut proposals2);
        assert!(proposals2.hydrated, "even a None doc marks hydrated-empty");
        assert!(proposals2.proposals.is_empty());
    }

    #[test]
    fn rehydration_skips_material_tagged_region_entries_including_level_and_smooth() {
        // Finding #1 fix: raise/lower regions used to be silently absorbed
        // into the palette mirror (stripping `material`); level/smooth
        // regions used to be silently dropped entirely (not a valid
        // `ProposalOp`). Neither may happen post-fix — region entries never
        // enter the mirror at all, regardless of their `op`.
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut petal_map = PetalMapState::default();
        let mut proposals = ProposalEditState::default();
        let terrain = Some(json!({
            "enabled": true,
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 2.0 },
                { "id": "r1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 3.0, "material": "earth" },
                { "id": "r2", "op": "lower", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 1.0, "material": "gravel" },
                { "id": "r3", "op": "level", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "target_height": 5.0, "material": "earth" },
                { "id": "r4", "op": "smooth", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 0.5, "material": "earth" },
            ],
        }));

        handle_petal_terrain_loaded("p1", &terrain, &nav, &mut petal_map, &mut proposals);

        assert!(proposals.hydrated);
        assert_eq!(
            proposals.proposals.len(),
            1,
            "only the palette entry ('p1') enters the mirror"
        );
        assert_eq!(proposals.proposals[0].id, "p1");
    }

    #[test]
    fn inactive_petal_load_never_touches_the_proposal_mirror() {
        // nav has no active petal — mirrors the existing petal_map gate test.
        let nav = NavigationManager::default();
        let mut petal_map = PetalMapState::default();
        let mut proposals = ProposalEditState::default();
        proposals.push_new(ProposalOp::Raise, vec![], None, Some(1.0));
        proposals.hydrated = true; // simulate an already-hydrated mirror for the CURRENT petal

        let terrain = Some(json!({ "proposals": [] }));
        handle_petal_terrain_loaded(
            "some-other-petal",
            &terrain,
            &nav,
            &mut petal_map,
            &mut proposals,
        );

        assert!(
            proposals.hydrated,
            "a stale/inactive-petal load must not downgrade or touch the mirror at all"
        );
        assert_eq!(
            proposals.proposals.len(),
            1,
            "current petal's local proposals survive an unrelated petal's load"
        );
    }
}
