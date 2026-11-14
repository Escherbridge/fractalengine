---
type: Review
gate: R-UI-2
upstream: [UI-03, UI-04, UI-05, UI-06, UI-09]
base: 7af0a144
---

# R-UI-2 adversarial review (round 4)

Security inbox: no entry names a file in this gate's owns (checked 2026-11-13). Leak scan of every added line in the owned files: clean.

- F1 FIXED - fe-ui/src/panels/section_profile_panel.rs:179 - the display factor was `length / last surviving station distance`, so a hole at the end of the line inflated every metre figure in the plot and TSV; the result now carries `world_length` (fe-renderer/src/section_profile.rs:28, set by the responder from the request) and the factor is `length / world_length`.
- F2 FIXED - fe-terrain/src/section_responder.rs:21 - the responder answered once and forgot, so a section requested before tiles streamed in stayed "unavailable" (and one taken before a proposal edit stayed stale) until the angle or footprint changed; it now retains the latest active-petal request and re-answers it under the same id when the terrain revision or height field changes, like `answer_volume_report_requests`. Journey `section_request_round_trip_answers_fill_and_unavailable` extended (map returns -> one refresh with the fill, quiet frame sends nothing).
- F3 FIXED - fe-ui/src/panels/section_profile_panel.rs:250 - a section `NoMap` reply means "no terrain under this line" (the volume sync clears the selection when the petal has no map), but the panel said "No map on this petal" and showed a second Open Maps button; it now says "No terrain under this section yet." and the whole block hides while the volume body already shows its Maps prompt.
- F4 FIXED - fe-terrain/src/profile.rs:45 - `scaled` accepted any finite positive scale, bypassing the global world-scale range (steer 21:55); it now uses `fe_format::scale::is_world_scale_in_range`, so a 1e300 scale never claims metres.
- F5 FIXED - fe-ui/src/node_manager/router.rs:118 - UI-09 removed the `allow(dead_code)` from `is_owner`, whose only callers are tests (router, measure, viewport_pick, road_interaction), so non-test builds would warn; now `#[cfg(test)]`.
- F6 HANDED-OFF (owner of fe-ui/src/ui_shell/right_sidebar.rs: I-UI-2 follow-up / orchestrator) - fe-ui/src/ui_shell/right_sidebar.rs:344 - `render_section_profile_block` is never called and `panels/mod.rs:273` already passes `section_profile` to `render_right_sidebar`, which has no such parameter (fe-ui does not compile); add `section_profile: &mut crate::panels::section_profile_panel::SectionProfileState` after `volume_report`, thread it to the ProposalReport section, and call `section_profile_panel::render_section_profile_block(ui, ui_mgr, section_profile, volume_report)` right after `render_volume_report_body(..)` (right_sidebar.rs:784).
- F7 HANDED-OFF (I-BATCH-1) - conductor/test-batches.md:154 - `panels::section_profile_panel` (UI-05 journey) is not in the R4 fe-ui filter and `panels::hierarchy_info` (UI-06 journey) is not in the R3 filter; both tests are unreachable by any batch until added.
- F8 HANDED-OFF (I-DOC-UI) - fe-ui/src/AGENTS.md:490 and fe-ui/src/gis/query.rs:15 - both still point at the deleted `actions::node_props::TRACK_NAME_KEY`; repoint to `gis::query::TRACK_NAME_KEY` / `fractalengine/src/gpx_bridge.rs` as the contract holders.
- F9 HANDED-OFF (later round, owner of fe-terrain/src/volume_report.rs) - fe-terrain/src/section_responder.rs:68 - the proposed-surface closure duplicates the private `volume_report::combined_height`; make that fn `pub(crate)` and call it so the two reports cannot drift.
- F10 KEPT (spec) - fe-terrain/src/profile.rs:72 - a skipped station is bridged by the next trapezoid, so area is integrated across a hole; the UI-03 spec's own "hole skipped, still Some" case expects this and no height is invented at a station.
- F11 KEPT (crate-internal) - fe-terrain/src/lib.rs:34 - `section_responder` is a private module although §4 lists it as a seam; its only consumer is `TerrainPlugin` in the same crate.
- F12 KEPT (matches the volume responder) - fe-terrain/src/section_responder.rs:29 - a height field that changes every frame while tiles stream re-answers the retained request each such frame (bounded at 512 stations); no work and no allocation when nothing changed and no request arrived.
- F13 KEPT (low value, not tautological) - fe-ui/src/panels/hierarchy_info.rs:323 - the render step only proves the card does not open a dialog by itself; routing, header/count strings and the Viewer denial text are asserted directly above it.
- F14 KEPT - UI-06 routing, override clearing (navigation change, node selection), Settings gate (`can_edit`, same as tree_entry options) and sidebar re-click branches (verse/fractal/petal) checked against fe-ui/src/panels/sidebar.rs:276, :409, :571; no defect found.
- F15 KEPT - UI-09 removals (TRACK_NAME_KEY, `cursor()`, fe-sim `verse_id` field) have no remaining code callers (field `cursor` is still read at router.rs:249); fe-hexon per-module allows are I-LIBS-2's file.

Verdict: PASS-WITH-FIXES (F6 blocks the fe-ui compile and the panel mount until right_sidebar.rs is threaded).
