//! Handler for per-petal terrain docs feeding the map picker + rehydrating the
//! terrain-proposal mirror (data-loss guard), and for the durable per-petal
//! `render_distance` echo (p2p_asset_streaming S6). See ../AGENTS.md §db-results.

use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::notice::DenialContext;
use crate::plugin::LocalUserRole;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalEditState;

/// `PetalRenderDistanceLoaded`: both the GUI's own `Set`/`GetPetalRenderDistance`
/// echo and a peer-converged value (via the `ReplicatedRowApplied` re-fetch,
/// see `mod.rs`) land here — the in-memory tree is the single source
/// `ResidencyBudget::render_distance_for` reads (fe-ui/src/AGENTS.md §app-settings).
pub(super) fn handle_petal_render_distance_loaded(
    petal_id: &str,
    render_distance: Option<f32>,
    verse_mgr: &mut crate::verse_manager::VerseManager,
) {
    verse_mgr.update_petal_render_distance(petal_id, render_distance);
}

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
    correlation_id: Option<&str>,
) {
    // M-03: a confirmed echo of THIS petal's own in-flight set clears
    // "Applying…" regardless of the active-petal gate below — the user may
    // have navigated away while the write was in flight, and the row must
    // not be stuck "Applying…" forever just because this load is now a
    // no-op for the viewport. A `None`/foreign id (a plain Get, a peer's
    // echo, or someone else's correlation) leaves pending_set untouched.
    if let (Some(id), Some(pending)) = (correlation_id, petal_map.pending_set.as_ref()) {
        if pending.correlation_id == id {
            petal_map.pending_set = None;
        }
    }
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
    petal_map.world_scale = fe_format::scale::world_scale_from_terrain_json(terrain.as_ref());
    // Any other load for the active petal is persisted truth (an earlier,
    // superseded set's echo, a peer's write, a plain Get), so it becomes what
    // a still-pending set rolls back to if denied.
    if let Some(pending) = petal_map.pending_set.as_mut() {
        pending.previous_tileset_ids = petal_map.tileset_ids.clone();
        pending.previous_world_scale = petal_map.world_scale;
    }
    // Hexon-authoritative clamp bounds (scale orchestration track); see fe-ui/src/verse_manager/AGENTS.md.
    petal_map.scale_bounds = terrain
        .as_ref()
        .and_then(|t| t.get("scale_bounds"))
        .and_then(|v| serde_json::from_value::<[f64; 2]>(v.clone()).ok());
    // Keep the raw doc for the GIS Layer Manager's mutate-and-round-trip flow.
    petal_map.confirm_terrain_document(terrain.clone());
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

/// `PetalTerrainSetFailed`: a denied/erroring CORRELATED `SetPetalTerrain`
/// (M-03). Rolls back to the exact snapshot `actions::hexon::set_petal_map`/
/// `set_petal_map_scale` captured before their optimistic update, and shows
/// a role-aware error notice. A correlation that doesn't match this petal's
/// `pending_set` is a foreign/stale echo (another in-flight set, or a race
/// with a newer one) and is ignored — it must never roll back a DIFFERENT
/// in-flight change.
pub(super) fn handle_petal_terrain_set_failed(
    petal_id: &str,
    error: &str,
    correlation_id: &str,
    petal_map: &mut PetalMapState,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    verse_name: Option<&str>,
    now: f64,
) {
    let Some(pending) = petal_map.pending_set.as_ref() else {
        return;
    };
    if pending.correlation_id != correlation_id {
        return;
    }
    // Safe to take now: the id just matched, so this IS the set this petal
    // is waiting on.
    let pending = petal_map
        .pending_set
        .take()
        .expect("pending_set checked Some immediately above");
    if petal_map.petal_id.as_deref() == Some(petal_id) {
        petal_map.tileset_ids = pending.previous_tileset_ids;
        petal_map.world_scale = pending.previous_world_scale;
    }
    ui_mgr.show_db_error(error, &DenialContext::new(local_role, verse_name), now);
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
            "world_scale": 0.5,
            "scale_bounds": [0.001, 0.1],
            "proposals": [
                { "id": "p1", "op": "raise", "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "delta": 2.0 }
            ],
        }));

        handle_petal_terrain_loaded("p1", &terrain, &nav, &mut petal_map, &mut proposals, None);

        assert_eq!(
            petal_map.world_scale, 0.1,
            "an out-of-bounds stored scale lands clamped into scale_bounds"
        );

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
            None,
        );
        assert!(proposals.hydrated);
        assert!(proposals.proposals.is_empty());

        let mut proposals2 = ProposalEditState::default();
        handle_petal_terrain_loaded("p1", &None, &nav, &mut petal_map, &mut proposals2, None);
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

        handle_petal_terrain_loaded("p1", &terrain, &nav, &mut petal_map, &mut proposals, None);

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
            None,
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

    // --- M-03: petal-map truth (correlated set, rollback, foreign echoes) ---

    fn installed_ts(id: &str) -> crate::terrain_map::dto::InstalledTilesetDto {
        crate::terrain_map::dto::InstalledTilesetDto {
            hexon_id: id.to_string(),
            region_name: "Region".into(),
            bounds: [0.0, 0.0, 1.0, 1.0],
            zoom_range: (8, 12),
            tile_count: 1,
            size_bytes: 0,
            seeding_enabled: false,
            installed_at: String::new(),
            publisher_did: None,
        }
    }

    fn db_sender() -> fe_runtime::app::DbCommandSender {
        let (tx, rx) = crossbeam::channel::unbounded();
        // W1-GATE fix: binding-and-dropping the receiver as `_rx` closed the
        // channel before `set_petal_map` ever sent on it, so every send hit
        // the `Err` arm and silently skipped the optimistic update + pending
        // correlation (the real bug every test here exists to catch). Leak
        // it instead — these tests only assert on `petal_map`'s side effects.
        std::mem::forget(rx);
        fe_runtime::app::DbCommandSender(tx)
    }

    #[test]
    fn denied_petal_map_set_rolls_back_and_explains() {
        let mut petal_map = PetalMapState {
            petal_id: Some("p1".into()),
            tileset_ids: vec!["old-ts".into()],
            world_scale: 1.0,
            ..Default::default()
        };
        crate::actions::hexon::set_petal_map(
            &db_sender(),
            &mut petal_map,
            "p1".into(),
            Some(installed_ts("new-ts")),
        );
        assert_eq!(
            petal_map.tileset_ids,
            vec!["new-ts".to_string()],
            "optimistic update applies immediately"
        );
        let correlation_id = petal_map
            .pending_set
            .as_ref()
            .expect("set_petal_map records a pending correlation")
            .correlation_id
            .clone();

        let mut ui_mgr = UiManager::default();
        let local_role = LocalUserRole {
            role: Some(fe_database::RoleLevel::Viewer),
        };
        handle_petal_terrain_set_failed(
            "p1",
            "permission denied: viewer cannot set petal terrain",
            &correlation_id,
            &mut petal_map,
            &mut ui_mgr,
            &local_role,
            Some("Acme Verse"),
            0.0,
        );

        assert_eq!(
            petal_map.tileset_ids,
            vec!["old-ts".to_string()],
            "a denial restores the pre-send tileset"
        );
        assert_eq!(petal_map.world_scale, 1.0);
        assert!(
            petal_map.pending_set.is_none(),
            "the row must leave \"Applying…\" once the denial is handled"
        );
        let notice = ui_mgr
            .latest_unacknowledged_error()
            .expect("a denial surfaces an error notice, never a silently-swallowed failure");
        assert_eq!(
            notice.text,
            "You're a Viewer in Acme Verse — Editor role needed to do that"
        );
        assert_eq!(
            notice.detail.as_deref(),
            Some("permission denied: viewer cannot set petal terrain"),
            "the raw error stays in the history"
        );
    }

    /// Two sets in flight: the second's denial must roll back to the last
    /// PERSISTED state — the original when the first was denied too, the
    /// first's map when its echo confirmed it — never to the first's
    /// optimistic, unconfirmed map.
    #[test]
    fn superseded_set_denial_rolls_back_to_last_confirmed_map() {
        for first_confirmed in [false, true] {
            let mut nav = NavigationManager::default();
            nav.active_petal_id = Some("p1".into());
            let mut petal_map = PetalMapState {
                petal_id: Some("p1".into()),
                tileset_ids: vec!["original".into()],
                world_scale: 1.0,
                ..Default::default()
            };
            let sender = db_sender();
            crate::actions::hexon::set_petal_map(
                &sender,
                &mut petal_map,
                "p1".into(),
                Some(installed_ts("first")),
            );
            let first_id = petal_map.pending_set.as_ref().unwrap().correlation_id.clone();
            crate::actions::hexon::set_petal_map(
                &sender,
                &mut petal_map,
                "p1".into(),
                Some(installed_ts("second")),
            );
            let second_id = petal_map.pending_set.as_ref().unwrap().correlation_id.clone();

            let mut ui_mgr = UiManager::default();
            let local_role = LocalUserRole::default();
            if first_confirmed {
                handle_petal_terrain_loaded(
                    "p1",
                    &Some(json!({ "tileset_hexon_uris": ["first"], "world_scale": 1.0 })),
                    &nav,
                    &mut petal_map,
                    &mut ProposalEditState::default(),
                    Some(first_id.as_str()),
                );
            } else {
                handle_petal_terrain_set_failed(
                    "p1",
                    "permission denied: x",
                    &first_id,
                    &mut petal_map,
                    &mut ui_mgr,
                    &local_role,
                    None,
                    0.0,
                );
            }
            handle_petal_terrain_set_failed(
                "p1",
                "permission denied: x",
                &second_id,
                &mut petal_map,
                &mut ui_mgr,
                &local_role,
                None,
                0.0,
            );

            let expected = if first_confirmed { "first" } else { "original" };
            assert_eq!(
                petal_map.tileset_ids,
                vec![expected.to_string()],
                "first_confirmed={first_confirmed}"
            );
            assert!(petal_map.pending_set.is_none());
        }
    }

    #[test]
    fn confirmed_petal_map_set_clears_applying() {
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut petal_map = PetalMapState {
            petal_id: Some("p1".into()),
            ..Default::default()
        };
        crate::actions::hexon::set_petal_map(
            &db_sender(),
            &mut petal_map,
            "p1".into(),
            Some(installed_ts("ts-1")),
        );
        assert!(petal_map.pending_set.is_some());
        let correlation_id = petal_map.pending_set.as_ref().unwrap().correlation_id.clone();

        let mut proposals = ProposalEditState::default();
        let terrain = Some(json!({ "tileset_hexon_uris": ["ts-1"], "world_scale": 1.0 }));
        handle_petal_terrain_loaded(
            "p1",
            &terrain,
            &nav,
            &mut petal_map,
            &mut proposals,
            Some(correlation_id.as_str()),
        );

        assert!(
            petal_map.pending_set.is_none(),
            "a confirmed echo for OUR correlation clears \"Applying…\""
        );
    }

    #[test]
    fn foreign_correlation_does_not_clear_pending() {
        let mut nav = NavigationManager::default();
        nav.active_petal_id = Some("p1".into());
        let mut petal_map = PetalMapState {
            petal_id: Some("p1".into()),
            ..Default::default()
        };
        crate::actions::hexon::set_petal_map(
            &db_sender(),
            &mut petal_map,
            "p1".into(),
            Some(installed_ts("ts-1")),
        );
        assert!(petal_map.pending_set.is_some());

        let mut proposals = ProposalEditState::default();
        let terrain = Some(json!({ "tileset_hexon_uris": ["ts-1"] }));
        handle_petal_terrain_loaded(
            "p1",
            &terrain,
            &nav,
            &mut petal_map,
            &mut proposals,
            Some("ui-petalmap-999999"),
        );

        assert!(
            petal_map.pending_set.is_some(),
            "a foreign/stale correlation must not clear THIS petal's in-flight set"
        );
    }
}
