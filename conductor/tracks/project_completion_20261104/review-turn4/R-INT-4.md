---
type: Review
---

# R-INT-4: adversarial review of I-LIBS-2, I-API-2, I-UI-2 (security review included)

Diffs reviewed: `git diff 7af0a144 -- <owns>`, i.e. commits `91a59533` (I-LIBS-2), `d87dce6f` (I-API-2) and
`b7e921dd` (I-UI-2) on top of the wave-1 integrations already passed by R-INT-1..3. Each wiring was checked
against turn4-plan.md §4 seams and the upstream modules at HEAD (`fe-hexon/src/snapshot.rs`,
`fe-terrain/src/{section_responder,tiles/entitlement,terrain_plugin}.rs`, `fe-renderer/src/section_profile.rs`,
`fe-api/src/{petal_snapshot,hexon,auth}.rs`, `fe-ui/src/node_manager/{road_interaction,road_overlay}.rs`,
`fe-ui/src/panels/{section_profile_panel,unsaved_edits,tool_options}.rs`, `fe-ui/src/ui_shell/right_sidebar.rs`).
Security inbox: no entry names a file this gate owns (entries 5, 6, 8 and 9 concern the snapshot handler and DB
door. Their API-side remainder is routed below). Leak scan of every added line: clean. No cargo was run.

## Routed edits (applied once, correctly placed)

- I-LIBS-2 `fe-hexon/src/lib.rs:30` `pub mod snapshot;` (HEXON-11). `fe-terrain/src/tiles/mod.rs:4` `pub mod entitlement;` (HEXON-15). `fe-renderer/src/lib.rs:13` `pub mod section_profile;`. `fe-terrain/src/lib.rs:33-34` `mod section_responder;` is gated on `render`, and its only user `terrain_plugin.rs:604` is gated the same way. The messages are registered at `terrain_plugin.rs:600-601`. KEPT (correct).
- I-LIBS-2 UI-09(d): the crate-wide `#![allow(dead_code)]` was replaced by per-module allows on every module. That matches the task's default when unsure (no compiler this phase). KEPT. See F6 for the comment pointer.
- I-API-2 `fe-api/src/lib.rs:36` `pub mod petal_snapshot;`. `fe-api/src/server.rs:282-290` mounts both routes inside the `authenticated` router (auth layer at `:545-547`). The handlers apply the Manager floor (`petal_snapshot.rs:24,109,212`, `RoleLevel::from("manager")`). Import carries `SNAPSHOT_ROUTE_BODY_LIMIT` (256 MiB + 1 MiB, the same budget as `/assets`), and `read_archive_field` streams under the cap. KEPT (correct).
- I-API-2 `fe-api/tests/api_integration.rs:820-990` hexon crates journey (HEXON-06 + HEXON-14 403 `entitlement_required`). The `ApiState` literal matches all 20 fields at HEAD. `HexonBuilder`, `EntryKind`/`LicenseType` (re-exported from fe-format), the `id@version` URI, the multipart `file_name` pickup and the `code` envelope were all checked against source. KEPT (compiles on reading, not run).
- I-UI-2 `fe-ui/src/node_manager/mod.rs:463-469`: Road is chained right after Measure, so ClickPriority order stays normative. `RoadHoverState` init is at `:430`, and `draw_road_overlay` comes after `handle_road_interaction`. `panels/mod.rs:24` `mod unsaved_edits;` (used by `status_bar.rs:410,442`). `panels/mod.rs:44` `pub mod section_profile_panel;`. `plugin.rs:955` init plus PostUpdate pair ordered after the volume pair. `maintenance.rs:65` resets the state (session-reset requirement met). KEPT (correct).

## Findings

- F1 FIXED `fe-ui/src/plugin.rs:1048-1049`: fe-ui wrote/read `SectionProfileRequest`/`SectionProfileResult` without registering them. Only `TerrainPlugin` did, so any app built without it (fe-ui test apps, a host without terrain) panics on the missing `Messages<T>` resource. Both are now registered beside the volume pair (idempotent double registration, same pattern).
- F2 FIXED `fe-ui/src/node_manager/mod.rs:508` → `fe-ui/src/plugin.rs:1108`: `draw_road_readout` takes `EguiContexts` and shows an `egui::Area`, but it was registered in `Update`. Under bevy_egui multi-pass, egui drawing belongs in `EguiPrimaryContextPass`, and every other viewport label is drawn there after `gardener_ui_system`, which also writes the same-frame `ViewportRect` the readout clips to. Moved there. The gizmo overlay stays in Update.
- F3 FIXED (my half) + HANDED-OFF `fe-ui/src/plugin.rs:1248,1347`, `fe-ui/src/panels/mod.rs:166,273,859,913`: UI-05's Section block (`render_section_profile_block`) had no caller anywhere, so the panel could never render. `SectionProfileState` now flows through `MiscUiParams` → `gardener_console` → `render_right_sidebar`. The right_sidebar half is NEEDS N1 below.
- F4 HANDED-OFF (owner of `fe-ui/src/ui_shell/right_sidebar.rs`, last owner R-NET-6. Orchestrator to route): fe-ui does not compile at HEAD. `panels/mod.rs` passes `road_state` (I-UI-2), and `tool_options::render` (CITY-13) takes `road: &mut RoadBuilderState`, but `render_right_sidebar`/`render_options_section` have neither. Exact edit: NEEDS N1.
- F5 DEFERRED (trigger: GUI door queue injectable per App, same as R-NEW-1 N13) `fe-ui/src/node_manager/mod.rs:859`: NEW-02 test leg (a) (handed to I-UI-2 by R-NEW-1 N12) is still unwritten. Waiting and `is_pending` are observable, but proving delivery means pumping the process-global `GUI_DOOR` past its 50 ms backoff. `pump_scene_door_at` is private to `actions/node.rs`, so the only route is a sleep (a timing assertion, banned by steer item 6), and a parked write would hold parallel tests' FIFO for about 7 s.
- F6 FIXED + HANDED-OFF (I-DOC-CORE) `fe-hexon/src/lib.rs:1`: the comment pointed at `fe-hexon/src/AGENTS.md §lint-scope`, which does not exist. The line now names its reason (UI-09, until a compile sweep). I-DOC-CORE must add the section: NEEDS N4.
- F7 HANDED-OFF (owner of `fe-api/src/petal_snapshot.rs`, SEC-INBOX 9 API side, first routed by R-INT-1): still open at HEAD. `import_petal_snapshot` (`petal_snapshot.rs:283`) never calls `scene_gate::admit(Door { claims: &claims, kind: SceneCallerKind::Rest }, &snapshot_petal_id, ..)`, so a bound scene hook never judges REST-imported nodes. The door classification R-INT-1 added only works if the caller passes through it.
- F8 HANDED-OFF (owner of `fe-runtime/src/app.rs`, first routed by R-INT-1): still open at HEAD (`grep PetalSnapshot fe-runtime/src/app.rs` is empty). `correlation_of_command`/`reply_kind_of_command` do not know the snapshot commands, so a correlated `RequestFailed` finds no waiter and the route returns 504 instead of the failure. A success could also be delivered to another uncorrelated waiter.
- F9 FIXED `fe-api/tests/api_integration.rs:992-1013` (test coverage of the wiring itself): the only snapshot test (`export_share_test.rs:1784`) calls the handlers directly, so the route mounts and the auth layer had no coverage. The hexon journey now asserts tokenless export = 401, an editor's export = 404 with the handler's `NOT_FOUND_OR_DENIED` body, and an editor's import = the same. An unmounted route answers an empty 404, and a route outside auth fails the `Extension<ApiClaims>` extractor.
- F10 DEFERRED (trigger: memory pressure or an OOM report on a host accepting snapshot imports) `fe-api/src/petal_snapshot.rs:207`: each import buffers up to 256 MiB, then parses it twice (authorize pass + restore pass) with no per-host concurrency limit. A Manager can hold several such buffers at once. A semaphore like `TerrainCaptureLimiter` would bound it.
- F11 HANDED-OFF (owner of `fe-api/src/petal_snapshot.rs` / `fe_hexon::snapshot`): `restore_snapshot_archive` writes blobs to the store before the DB can refuse the import (409 existing petal or identity collision). A refused import therefore leaves content-addressed blobs behind. Bounded by the body cap and harmless to integrity, but unreferenced. Write blobs after `PetalSnapshotImported`, or sweep on refusal.
- F12 HANDED-OFF (owner of `fe-terrain/Cargo.toml`): `tiles/entitlement.rs:4` uses `fe_format::entitlement`, which exists only under fe-format's `entitlement` feature. fe-terrain gets that feature only by unification through fe-hexon. Declare it directly: NEEDS N3.
- F13 KEPT (cosmetic) `fe-renderer/src/lib.rs:13`, `fe-terrain/src/lib.rs:21`: new module lines are not in alphabetical order. No behaviour impact.

## NEEDS (exact edits outside these owns)

- N1 `fe-ui/src/ui_shell/right_sidebar.rs`, five changes:
  1. `render_right_sidebar` (:328): add `road_state: &mut crate::node_manager::road_chain::RoadBuilderState,` after `measure_state`.
  2. Same function (:344): add `section_profile: &mut crate::panels::section_profile_panel::SectionProfileState,` after `volume_report: &VolumeReportState,`. That is the order `panels/mod.rs:263-276` now passes.
  3. `render_options_section` (:606): add `road_state: &mut crate::node_manager::road_chain::RoadBuilderState,` after `measure_world_scale: f64,`. Pass `road_state` at the call (:420, after the `measure_world_scale(..)` argument), and insert `road_state,` after `measure_world_scale,` in its `tool_options::render(..)` call (:675).
  4. `render_proposal_report_section` (:760): add a `section_profile: &mut SectionProfileState` parameter after `volume_report`, and pass `section_profile` at its call (:450).
  5. After `volume_report_panel::render_volume_report_body(..)` (:784), call `crate::panels::section_profile_panel::render_section_profile_block(ui, ui_mgr, section_profile, volume_report);`.
- N2 `fe-api/src/petal_snapshot.rs`: F7 (scene_gate admit before `db_round_trip`, map `SceneDenied` to `http_status()`, stage the `Admission` after `PetalSnapshotImported`) and F11. `fe-runtime/src/app.rs`: F8 (add `ExportPetalSnapshot`/`ImportPetalSnapshot` to `correlation_of_command`, the three snapshot results to `correlation_of_result`, and a `ReplyKind::PetalSnapshot` to both kind maps).
- N3 `fe-terrain/Cargo.toml:31`: `fe-format = { path = "../fe-format", features = ["entitlement"] }`.
- N4 `fe-hexon/src/AGENTS.md` (I-DOC-CORE): add `## §lint-scope`. It records that the crate-wide `dead_code` allow became per-module allows (UI-09), that each stays until a compile sweep lists real leftovers, and that the sweep deletes the allow on a module with none.
- N5 `fe-ui/src/AGENTS.md` (I-DOC-UI) §road-chain / viewport labels: the road readout draws in `EguiPrimaryContextPass` after `gardener_ui_system` (egui pass + same-frame `ViewportRect`), and the gizmo overlay stays in Update.
- N6 batch: the new snapshot legs live in `hexon_crates_publish_install_search_entries_asset_journey`, which `--test api_integration` already reaches (`conductor/test-batches.md:313`). No new filter is needed.

## Security review (fresh pass, prompted to refute)

- Auth middleware: both snapshot routes sit inside `authenticated` (no public or debug mount). Tokenless gets 401, and a role below Manager gets an existence-safe 404 before any body read or DB send (pinned by F9).
- Body limit: the import per-route limit bounds the multipart stream. The handler cap refuses with 413 without buffering the rest. Export has no request body.
- Replication admission, relay admission/signing, licence tokens: none of these slices wires them. The hub additions are module roots, routes and UI plumbing.
- Viewer gating: I-UI-2 did not touch `inspector.rs`, and R-INT-3's F1 Save gating is intact.
- Frame safety: no blocking call added. The section-profile systems only write messages, and the readout move removes an egui call made outside the pass.
- Open security items routed, not closed here: SEC-INBOX 9 API side (F7), reply mis-routing (F8), blob residue (F11).

Verdict: PASS-WITH-FIXES (5 fixed, 6 handed off, 2 deferred with triggers, 2 kept). fe-ui needs NEEDS N1 before it compiles.
