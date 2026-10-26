---
type: review
track: terrain_proposal_delta_viz_20261025
slice: S7
title: "S7 adversarial review-and-fix: delta viz + volume report (waves 1-3)"
created: 2026-10-26
reviewed_commits: [95ac555, 585784b, 5bf9883]
verdict: pass-after-fixes
---

# S7 review-and-fix: verdict

**PASS after fixes.** Waves 1-3 delivered the P1/P2 surface the spec asks for, and NFR-1 holds structurally. I found no write path to the DB, the `TerrainHeightField` or `ActivePetalTerrain.revision` from any preview, heatmap or report system. But the review found **4 high-severity correctness bugs**. Three are preview/report ≠ commit drifts, which is exactly the FR-3/FR-5 promise. The fourth is a named track test that was red. There were also 6 medium/low defects. All are fixed in this commit with flow tests that go red on the old code.

Gate (all foreground, `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217828`):

| Step | Result |
|---|---|
| `cargo check -j4 -p fe-terrain -p fe-renderer -p fe-ui --tests` | exit 0, 0 warnings |
| `cargo +1.99.0 clippy -j4 -p fe-terrain -p fe-renderer -p fe-ui --all-targets -- -D warnings` | exit 0 |
| same clippy for `-p fe-terrain --features render` | exit 0 |
| `cargo test -j4 -p fe-terrain -p fe-renderer -p fe-ui` | fe-renderer 70+5, fe-terrain 220 lib + 55 integration (1 ignored, pre-existing), fe-ui 1022: **all pass** |
| `cargo test -j4 -p fe-terrain --features render --lib` | **318 passed, 0 failed** |

> **Gate gap found:** the mandated `-p fe-terrain` test run does NOT enable `render`, so every S2/S5 Bevy-App flow test in `terrain_plugin.rs` (`committed_raise_renders_lattice_ghost_not_flat_lid`, `armed_preview_spawns_…`, `volume_request_round_trip_…`, …) **never ran** in the wave gates. The extra `--features render` run is how `overlays_hidden_when_proposal_layer_hidden` was found red. Future gates for this crate must add `-p fe-terrain --features render` (a `--workspace` run unifies it in via the `fractalengine` binary).

## Findings (all fixed)

| # | Sev | Finding | Fix | Test (red before) |
|---|---|---|---|---|
| 1 | HIGH | **FR-5 composite drift (Smooth).** S2 made the bake and heatmap resolve every Smooth mean at one *union-of-all-regions* grid step. Adding an unrelated far-away region therefore re-shaped an existing Smooth region's baked relief, and the bake no longer matched the volume persisted on the region node (own-step mean). `volume_report` had a *third* copy of the composite walk, with means at the selection's step. | `sculpt::prepare_composite(regions, base)` now samples each Smooth mean at the region's OWN `region_grid_step`. `EARTHWORK_LATTICE`/`lattice_grid_step`/`region_grid_step` move into `sculpt`. `composite_grid_step` is deleted. `volume_report` now calls `prepare_composite` + `composite_proposed_height` (its private copy and the `region_effective_delta` duplicate are deleted). | `sculpt::composite_smooth_mean_matches_standalone_and_ignores_unrelated_regions`, `volume_report::selection_total_and_items_share_the_bake_composite` |
| 2 | HIGH | **Shape preview in the wrong units.** `terrain_preview::sculpt_shape_draft` sent the Circle/Rect footprint in METERS (`SculptToolState`), but the seam, fe-terrain and the `SculptShapeRegion` commit are WORLD units. On any scaled petal the ghost was mis-sized and mis-placed and its cut/fill ≠ the commit. It also used the raw radius (not `sanitized_radius`) and `sculpt.strength`, while the committed shape region carries no strength (it bakes at 1.0). The Volume report's drawn-selection fallback had the same meters bug, so it integrated, and listed nodes inside, the wrong area. | New `terrain_preview::shape_commit_footprint_world` = the Apply button's `tool_options::shape_apply_footprint` → `meters_to_world` (exactly what `handle_shape_region` does). Shape params map through `region_record_preview_params` (strength 1). `resolve_volume_selection` takes `world_scale` and uses the same fn. | `sculpt_shape_preview_matches_what_apply_commits_on_a_scaled_petal`, `drawn_shape_selection_is_the_world_footprint_apply_commits` |
| 3 | HIGH | **Brush preview ≠ commit.** `handle_brush_stroke` persists Raise/Lower as `|delta| × strength` at full strength. The preview passed raw `delta` with `strength` (ignored by Raise), so at strength 0.5 it showed **2×** the committed volume. | Extracted `actions::terrain_proposal::brush_region_fields` (now the ONE mapping, used by the commit handler) and fed it to the preview via `region_record_preview_params`. | assertion added to `brush_stroke_in_progress_publishes_corridor_preview` |
| 4 | HIGH | **Named track test red:** `overlays_hidden_when_proposal_layer_hidden` panicked: the overlay entities had no `Visibility`. Behind it sat a real NFR-6 bug: `sync_layer_visibility` only runs on a LayerStack *change*, so any ghost/drape/**live preview** spawned while the ProposalOverlay layer was already hidden appeared anyway. The GeoJSON spawner already did this right. | `layer_initial_visibility` is inserted at all four new spawn sites (committed ghost + drape, baked drape, preview ghost + drape). The test now covers drapes and a rebuild-while-hidden. | `overlays_hidden_when_proposal_layer_hidden` (extended), `preview_spawned_under_hidden_layer_starts_hidden` |
| 5 | MED | **ProposalOverlayBudget bypass + no per-petal heatmap cap (NFR-2).** `render_baked_region_heatmaps` spawned one drape (≤16k verts) per region with no budget at all, and nothing capped total heatmap vertices. | Baked drapes honour `proposal_overlay_capacity(budget.allowance, built)` with the same truncate/regrow gate. New `MAX_HEATMAP_VERTICES_PER_PETAL` (1,048,576) is shared by committed + baked drapes via `HeatmapVertexUsage`. Hitting the cap warns once and skips the drape. It deliberately never sets `truncated`, because that flag re-arms a rebuild on any headroom (a per-frame loop). | `baked_heatmaps_stop_at_residency_budget` |
| 6 | MED | **FR-4 heatmap missing after startup.** Baked heatmaps rebuilt only on a revision change. The petal config usually lands before its chunks, so the first build sampled an empty field and the drape never appeared. | A re-ground gate mirrors the committed ghosts: rebuild when the field changed and grounded footprint vertices grew. | `baked_heatmap_appears_once_covering_terrain_arrives` |
| 7 | MED | **Stale volume report.** fe-ui re-requested only on a selection/petal change. After a commit, delete or API write under an unchanged selection, the old figure stayed up indefinitely. | `sync_volume_report_request` also re-requests on `DbResult::PetalTerrainLoaded` for the active petal and withdraws the old figure (Pending). | `selected_region_report_shows_true_cut_fill_not_area_times_delta`, rewritten as a real App flow (see 9) |
| 8 | MED | **NFR-4 + duplicate overlay in the live readout.** The readout hard-coded `m³` even on unscaled petals (`ProposalPreviewReport` had no `scaled`). The "readout-only" selected-proposal source (d) actually rendered a second identical ghost + drape over the committed one (z-fight / double tint). | `ProposalPreviewReport.scaled` and `preview_readout_text` (`wu³` + "no map scale"). New `PreviewSource::SelectedProposal`: fe-terrain reports but spawns nothing. | `preview_readout_never_labels_world_units_as_meters`, `selected_proposal_preview_reports_without_duplicate_meshes` |
| 9 | LOW | **Tests that could not fail.** `selected_region_report_shows_true_cut_fill_not_area_times_delta` set `result = 12.5` and asserted `12.5 ≠ 200` (a tautology). `unscaled_petal_report_shows_world_units_chip` restated two string constants. `lattice_integrated_volume_equals_cut_fill_volume` only compared the lattice's `cut_fill` to the fn that produced it, not the spec's "heatmap's integrated dh". | Rewritten: (a) a real fe-ui App round trip (selection → request; matching reply shown; stale-correlation reply ignored; terrain reload re-requests); (b) the TSV egress for an unscaled result carries `wu` on every row and no `m`; (c) integrates the drape's own per-vertex dh over its triangles and matches `cut_fill_volume` to 1e-4. | the tests themselves |
| 10 | LOW | **Per-frame allocation (NFR-3) + robustness.** fe-ui re-inserted `ProposalPreview` every frame, so the resource always read as changed and fe-terrain cloned a snapshot every frame. `cut_fill_rgba` produced channels > 1 when the neutral band ≥ ramp max, and NaN for a NaN dh. `effective_step`'s 8-pass loop had no hard cap guarantee. Also the S3 `private_interfaces` warning, plus S4/S6 doc-list, dead-field and field-reassign lints that failed `clippy -D warnings`. | Compare-before-insert in fe-ui and `PreviewDebounce::poll_unchanged` (an idle frame allocates nothing). Clamped ramp with a finite guard. A final hard step bound. `publish_proposal_preview` → `pub(crate)`. The dead `area × delta` `volume` field is removed from `proposal_report_panel::compute_report` (R-4: the approximation is gone, not just unrendered). | ramp table extended |

## Checked and sound (no change)

- **NFR-1.** Every new sampler is `&impl Fn`, and every system takes the field as `Option<Res<_>>`. No `DbCommand`/`SyncCommand` sender is reachable from fe-terrain. The preview never touches `revision`. Byte-identity tests exist for the lattice.
- **NFR-5.** fe-ui has no fe-terrain dependency (checked `fe-ui/Cargo.toml`). All traffic goes through fe-renderer seam types.
- **Petal switch.** The preview clears on the switch frame (fe-ui baseline plus fe-terrain petal-scope gate). Wrong- or stale-petal volume requests are dropped. `VolumeReportState` clears.
- **Stale preview after disarm.** Closing TerrainTools, committing (count-grow suppression) or ending a stroke removes the resource, and fe-terrain despawns on the Clear edge.
- **Report vs ground truth.** Totals and per-item go through `cut_fill_volume`. The S4 analytic tests and S5's 200 m³ round trip hold.

## Residual (consciously not fixed here; owner: S8 docs / follow-up)

- Palette proposals and regions are composited proposals-then-regions, not interleaved in true record order. That only matters for overlapping non-additive palette+region pairs. It is documented in `fe-terrain/src/AGENTS.md` §volume-report.
- Overlapping baked regions draw overlapping drapes at the same height (both show the composite dh), so they double-tint. This is cosmetic.
- `HeatmapOverlayToggle` and the OD-8 "ghost toggle in Options" have no UI yet. The toggle defaults to on. Wire it in the Options surface (`ui_semantics_unification`).
- `sync_volume_report_request` and the bake's `config_earthwork_regions` still allocate every frame. Both predate this review's scope or are pre-existing. Gate them on real input change when the UI stops `ResMut`-touching `PetalMapState` every frame.
- The palette preview sits at the origin, matching the commit's "append at origin" convention, not the cursor. Revisit with gimbal-on-proposal.
- In-app F5 verification stays user-gated.
