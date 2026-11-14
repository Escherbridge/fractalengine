---
type: Review
---
# R5-CITY review (CITY5-01 .. CITY5-05)

R5C-1 HANDED-OFF (I5-RUNTIME) - fe-runtime/src/messages.rs:1629 - `DbResult::NodeCreated` has no `scale` field at HEAD; fe-ui destructures and passes it (db_results/mod.rs:740, nodes.rs:29), so fe-ui does not compile until `/// Scale the row was written with ([1.0; 3] when none applied).` + `scale: [f32; 3],` is added as the LAST field.
R5C-2 HANDED-OFF (I5-DB) - fe-database/src/lib.rs:925 and :1758 - constructors lack the field; add `scale: applied_scale` (:925) and `scale: copy.scale` (:1758); node_lifecycle_test journey (~:572) should set petal world_scale 0.01 then assert the echo `scale == [0.01; 3]`.
R5C-3 HANDED-OFF (I5-RUNTIME) - fe-runtime/src/app.rs:1139 and fe-test-harness/src/peer.rs:319 - constructors need `scale: [1.0; 3]`; fe-api/tests/mcp_integration.rs:430 already has it; ws.rs/rest.rs/tools.rs/gpx_bridge.rs:1067,1371/egress_strings.rs use `..` and need nothing.
R5C-4 KEPT (scoped, no production reader of `kind`; module-wide allow is gone) - fe-ui/src/node_manager/road_snap.rs:186 - field-level `#[allow(dead_code)]` on `Guide.kind` remains.
R5C-5 KEPT (DB stays the range authority via is_admissible_transform) - fe-ui/src/verse_manager/db_results/nodes.rs:14 - UI sanitiser only guards finite and > 0, not the global scale range.
R5C-6 KEPT (verified) - fe-ui/src/verse_manager/db_results/mod.rs:1006 + gis/mod.rs:529 - RequestFailed clears only a pending create whose correlation id matches; foreign id untouched (test request_failed_clears_the_matching_road_create); next frame sync_session_track (road_interaction.rs:139-149) clears the road log so the tool is released.
R5C-7 KEPT (verified) - road_interaction.rs:316-320, road_chain.rs:308, road_overlay.rs:108 - world_scale is written before any reduce call; commit (on_drag_end) and ghost both convert freeform_spacing_m via meters_to_world; NaN/0 falls back to 1:1; tests assert ghost and commit counts agree at 1:100.
R5C-8 KEPT (verified) - road_overlay.rs:192,248 + road_chain.rs:445 - ghost uses a Local buffer cleared per frame and expand_segment_into/bezier_into append; no per-frame allocation on curved/straight/freeform paths.
R5C-9 KEPT (verified) - nodes.rs:42, verse_manager/mod.rs:105 - scale lands only in `transform`; world_transform overrides translation only, so scale is applied once.
R5C-10 KEPT (verified) - fractalengine/src/gpx_bridge.rs:165-205 - extracted submit_authored_track_properties; test asserts path_kind designed + gpx_type track, never "recorded", import draft stays recorded; reachable by batch filter `gpx_bridge::`; removing the PATH_KIND_KEY entry turns it red.
Batches: db_results tests via `verse_manager::db_results::tests::`; road modules via the node_manager::road_* filters (test-batches.md:57, :194); no new filter needed.
SEC-INBOX: no entries name R5-CITY files.

## Completeness
CITY5-01 COMPLETE fe-ui/src/verse_manager/db_results/mod.rs:1006 (trailing-points flush ordered test at :2085-2130)
CITY5-02 COMPLETE fe-ui/src/node_manager/road_chain.rs:308 (ghost: road_overlay.rs:108)
CITY5-03 COMPLETE fe-ui/src/node_manager/curve.rs:121 (road_snap.rs module allow removed; callers road_interaction.rs:14)
CITY5-04 STILL OPEN - fe-ui half done (nodes.rs:29, mod.rs:740) but variant field and constructors missing - fe-runtime/src/messages.rs:1629, fe-database/src/lib.rs:925
CITY5-05 COMPLETE fractalengine/src/gpx_bridge.rs:4147
INT5-07 STILL OPEN - NodeCreated scale widening not landed across fe-runtime/fe-database/app.rs/peer.rs - fe-runtime/src/messages.rs:1629

## Verdict
PASS-WITH-FIXES - the logic and tests are sound; the tree does not compile until I5-RUNTIME and I5-DB add the `scale` field and its constructors.
