---
type: Review
track: terrain_proposal_delta_viz_20261025
critic: C-UI
round: 4
head: 50c50d29
date: 2026-11-13
---

# C-UI completeness: terrain_proposal_delta_viz_20261025 (round 4)

Checked at HEAD `50c50d29` against spec.md FR-1..FR-11, NFR-1..NFR-7 and plan.md §4, plus round-4 items UI-01..UI-05 and UI-12. Gates read: `review-turn4/R-UI-2.md` (UI-03..UI-06, UI-09) and `polish_lane_20261031/review-turn4/R-UI-1.md` (UI-01, UI-02), plus R-INT-4 (section-profile wiring). No cargo was run.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file owned by this critic or a file of this track's round-4 items; nothing to close.

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 lattice ghost surface (S1, S2) | complete | `fe-terrain/src/delta_viz.rs:39` lattice build; `fe-terrain/src/terrain_plugin.rs:1528` `render_terrain_proposals`. |
| FR-2 cut/fill heatmap drape (S1b, S2) | complete | `terrain_plugin.rs:1772` `heatmap_vertex_colors`; journey `:2776`. |
| FR-3 live pre-commit preview (S3) | complete | `terrain_plugin.rs:2345` `render_proposal_preview`. |
| FR-4 baked regions show their delta | complete | `terrain_plugin.rs:2048` `render_baked_region_heatmaps`. |
| FR-5 one composite truth | complete | `fe-terrain/src/sculpt/mod.rs:291` `composite_proposed_height`. |
| FR-6/FR-7/FR-8 volume selection, report, per-item breakdown (S4-S6) | complete | `fe-terrain/src/volume_report.rs:187` `selection_volume_report`; responder `terrain_plugin.rs:2527`; panel `fe-ui/src/panels/volume_report_panel.rs:515` `nodes_in_footprint`. |
| UI-01 report never hangs (W4-S3, R-01/R-04) | complete | `terrain_plugin.rs:2551-2553` unavailable replies; panel `volume_report_panel.rs:761`, `:773-775`. |
| FR-9 copyable output | complete | `volume_report_panel.rs:891` "Copy as TSV". |
| FR-10 / S9 profile sampler (UI-03) | complete | `fe-terrain/src/profile.rs:31` `sample_profile`, table tests `:130`, `:162`; world-scale range check (R-UI-2 F4). |
| FR-10 / S10a seam + responder (UI-04) | complete | Seam `fe-renderer/src/section_profile.rs` (incl. `world_length`, :28); responder `fe-terrain/src/section_responder.rs:22`, re-answers on terrain change (R-UI-2 F2); journey `section_responder.rs:170`; messages registered in fe-ui `plugin.rs:1048-1049` (R-INT-4 F1). |
| FR-10 / S10b section panel (UI-05), plan `S10` | partial | Panel built: `fe-ui/src/panels/section_profile_panel.rs:71` centroid chord, `:216` `render_section_profile_block`; state threaded to `panels/mod.rs:168,275`. Not mounted: `render_right_sidebar` has no `section_profile` parameter and nothing calls the block (`right_sidebar.rs:784`), so fe-ui does not compile (R-UI-2 F6, R-INT-4 F4/N1). Plan box `plan.md:135` correctly open. |
| NFR-1 never write true terrain | complete (carried) | S7 review `review-s7.md`; profile/section are read-only closures (`section_responder.rs`). |
| NFR-2 residency | complete | `terrain_plugin.rs:1731` `heatmap_vertices_fit`; journey `:3312`. |
| NFR-4 reporting honesty | complete | `volume_report_panel.rs:848` world-units chip; section factor `length / world_length` (R-UI-2 F1). |
| NFR-6 visibility (heatmap sub-toggle, OD-10) | partial | UI-02 toggle `fe-ui/src/panels/terrain_tools_panel.rs:275` honoured by `terrain_plugin.rs:3348`; the toggle is not hidden while the ProposalOverlay layer is off (R-UI-1 F7 DEFERRED). |
| UI-12 Phase 4 spin-out (utility node contract stub) | partial | Track exists `conductor/tracks/utility_node_contract_20261113/{spec,plan,metadata}` (`efa13b1f`); reserve `fe-renderer/src/terrain_overlay.rs:296` `elevation_band`. Not registered in `conductor/tracks.md`; plan box `plan.md:138` not ticked. |
| Retro + archive | missing | `plan.md:131` open. |
| §9 q29 default SHIP P3 now | partial | Shipped in code (UI-03..UI-05), not reachable in-app until the panel is mounted. |
| §9 q30 default chord through the volume centroid at a set angle | complete | `section_profile_panel.rs:25` `angle_deg`, `:71-99` `section_chord`; no new viewport gesture. |

## Excluded by the scoper (not gaps)

- blocked-user: Full sweep (orchestrator, once) (plan.md:129).
- blocked-user: In-app verify F5 (plan.md:130).
- obsolete: P4 assets-in-volume takeoff (FR-11 future; only the UI-12 stub is in scope).
- obsolete: review-s7 residuals (overlapping drapes double-tint, per-frame allocation in `sync_volume_report_request`/`config_earthwork_regions`).

## HANDED-OFF (owner = next round)

- HANDED-OFF (owner of `fe-ui/src/ui_shell/right_sidebar.rs`, orchestrator to route): mount the Section block and restore the fe-ui compile with R-INT-4 NEEDS N1 (parameters `road_state`, `section_profile`; call `render_section_profile_block(ui, ui_mgr, section_profile, volume_report)` after `render_volume_report_body` at `right_sidebar.rs:784`). Then T-UI ticks S10.
- HANDED-OFF (later round, owner of `fe-terrain/src/volume_report.rs`): R-UI-2 F9, share `combined_height` as `pub(crate)` with `section_responder.rs:68` so the two reports cannot drift.
- HANDED-OFF (UI next round, fe-renderer seam owner): NFR-6 layer gate for the heatmap toggle (R-UI-1 F7).
- HANDED-OFF (I-CONDUCTOR next round): register `utility_node_contract_20261113` in `conductor/tracks.md`; correct `tracks.md:216` ("14 done / 0 open") while S10 is open.
- HANDED-OFF (T-UI next round): tick `plan.md:138` (spin-out done, reserve present) and run Retro + archive once S10 is mounted and the user has verified F5.
- HANDED-OFF (I-BATCH next round, low): `fe-renderer/src/section_profile.rs` has no tests, so nothing to register; if a seam test is added, extend the R4 `fe-renderer --lib -- terrain_overlay terrain_height` filter with `section_profile`.

Verdict: PARTIAL - P1, P2 and the P3 maths/responder are complete; the P3 section panel is built but unmounted (fe-ui compile blocked on right_sidebar.rs N1), and the spin-out stub is unregistered.
