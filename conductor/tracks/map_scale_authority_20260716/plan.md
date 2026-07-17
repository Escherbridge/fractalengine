---
type: Implementation Plan
title: "Implementation Plan: Map-Authoritative Scale Consistency"
tags: [map_scale_authority_20260716]
resource: ./spec.md
---

# Implementation Plan: Map-Authoritative Scale Consistency

## Overview

Five TDD phases, dependency-ordered: canonical accessor first (everything routes
through it), then the placement default (fe-database), then the UI audit (fe-ui) +
API contract doc, then terrain-height snap-on-place, then the scripted round-trip
proof + the single full workspace sweep.

**Test policy (binding for this track):** per task, red→green runs ONLY the task's own
new/changed test target (e.g. `cargo test -p fe-format scale`); the full workspace
sweep (`cargo fmt --check && cargo clippy -- -D warnings && cargo test`) runs exactly
once, in Phase 5. Docs go in directory `AGENTS.md` sections with one-line `///`
pointers in code.

**Coordination:** do not touch `fe-terrain/src/scale.rs`'s public API shape
(hexon_scale_orchestration Ph5/6 build on it); keep `petal_world_scale` reachable for
mcp_scene_primitives' `place_asset`; road_builder_ux will import `fe_format::scale`
for road lengths.

## Phase 1: Canonical accessor — `fe_format::scale`

Goal: one Bevy-free home for sanitize/clamp/convert + the terrain-JSON accessor;
fe-terrain delegates with zero behavior change.

Tasks:
- [ ] Task: Create `fe-format/src/scale.rs` with `sanitize_world_scale`,
  `clamp_world_scale_to_bounds`, `meters_to_world`, `world_to_meters`, and
  `world_scale_from_terrain_json(Option<&serde_json::Value>) -> f64` (default 1.0 on
  None/null/missing/invalid; sanitize + clamp to `scale_bounds` when present).
  (TDD: port fe-terrain's scale test vectors verbatim + new JSON-accessor cases:
  null, missing key, NaN/∞/0/negative, in-bounds, clamped-out-of-bounds, malformed
  bounds; then implement.)
- [ ] Task: Rewire `fe-terrain/src/scale.rs` to delegate/re-export from
  `fe_format::scale`, keeping its public API identical. (TDD: fe-terrain's existing
  scale + config `effective_world_scale` tests must pass UNCHANGED — that is the
  parity proof; no test edits allowed in this task.)
- [ ] Task: Docs — new `fe-format/src/AGENTS.md` §scale (canonical-home rationale,
  rejected fe-database alternative, the one formula); update `fe-terrain/src/AGENTS.md`
  §scale with the delegation note.
- [ ] Verification: `cargo test -p fe-format -p fe-terrain` green; confirm no public
  API change in fe-terrain scale (callers unmodified). [checkpoint marker]

## Phase 2: Map-authoritative placement (fe-database)

Goal: imported/created nodes default to `scale = [world_scale; 3]` read from the
petal's terrain JSON; echoes carry the true value; existing nodes untouched.

Tasks:
- [ ] Task: Add fe-format dep to fe-database; implement
  `petal_world_scale(db, petal_id) -> f64` in `handlers/` (SELECT terrain FROM petal,
  `world_scale_from_terrain_json`; missing petal/terrain → 1.0), reachable by
  fe-api-facing handler code (mcp_scene_primitives seam). (TDD: handler test with
  in-memory DB — petal with terrain `world_scale: 0.01`, petal with null terrain,
  nonexistent petal; then implement.)
- [ ] Task: `import_gltf_handler` + `create_node_handler` write
  `scale: [ws, ws, ws]` and return the applied scale; update `lib.rs` dispatch so
  `SceneChange::NodeAdded` + `DbResult::NodeCreated`/`GltfImported` echoes carry it
  (kill the hardcoded `[1,1,1]` at lib.rs:368/:397). (TDD: red test asserting the
  created node row scale is `[0.01;3]` on a 1:100 petal and `[1.0;3]` on a
  terrain-less petal, and that the result echo matches the row; then implement.)
- [ ] Task: Update consumers of the widened handler returns: `fe-test-harness/src/peer.rs`
  ImportGltf arm + `scenarios/blob_roundtrip.rs`/`two_peer_blob_exchange.rs`
  expectations; verify the fe-ui spawn path (`verse_manager/db_results/nodes.rs`)
  applies `NodeDto.scale` to the spawned transform. (TDD: adjust/extend scenario
  assertions first where they encode `[1,1,1]`.)
- [ ] Verification: `cargo test -p fe-database -p fractalengine-test-harness` green;
  manual: same glb placed on 1:1 and 1:100 petals shows correct relative size to
  terrain (acceptance 1 — user-gated in-app check noted for Phase 5).
  [checkpoint marker]

## Phase 3: UI units audit + API contract (fe-ui, fe-api docs)

Goal: every displayed length/position/size derives from the accessor; every meters
input converts back through it; local formula copies become thin shims; API unit
contract documented.

Tasks:
- [ ] Task: Add fe-format dep to fe-ui; route the scale *read* through the accessor:
  `db_results/terrain.rs` populate of `PetalMapState.world_scale` (raw read →
  `world_scale_from_terrain_json`), `terrain_map/mod.rs::tileset_to_terrain_json`
  inline sanitize, `actions/hexon.rs::clamp_to_bounds`. Resolve OQ-3
  (consolidate `UiManager.world_scale` mirror onto PetalMapState or document the
  single write-path). (TDD: existing tests keep passing; add a test that a terrain
  JSON with out-of-bounds `world_scale` lands clamped in `PetalMapState`.)
- [ ] Task: Replace conversion copies in landed-track surfaces with accessor
  delegation: `path_segment_interaction.rs::guard_world_scale`,
  `path_asset_reconcile.rs::sanitize_world_scale` (thin f32 shim over
  `fe_format::scale`), inspector position/size conversions (node_manager
  `sync_inspector_units`). (TDD: existing unit tests stay green unchanged — parity
  proof; shims get one delegation test each.)
- [ ] Task: Audit + fix remaining display/input surfaces per the spec FR-3 table:
  gimbal drag deltas / `transform_broadcast` / `viewport_labels` readouts, waypoint
  move (`path_point_interaction`), `gis_panel` readouts, GltfImport dialog position
  display → meters (OQ-1: meters only). Record the audit result (surface → verdict →
  fix commit) in `fe-ui/src/AGENTS.md` §units. (TDD: pure reducer tests per converted
  surface where one exists; display-only egui sites covered by the audit table +
  Phase 5 manual verification.)
- [ ] Task: Document the API unit contract — `fe-api/AGENTS.md` §units: world units on
  the wire, meters are a UI-edge concern, formula + `fe_format::scale` pointer,
  caller-supplied y for placement endpoints; one-line `///` pointers on transform DTOs
  in `fe-api/src/types.rs`.
- [ ] Verification: `cargo test -p fe-ui` green; grep proves fe-ui has no remaining
  standalone sanitize/guard implementations (only delegating shims); no fe-ui →
  fe-terrain edge introduced. [checkpoint marker]

## Phase 4: Terrain-height snap-on-place

Goal: in-app placement lands models on the terrain surface at (x,z); terrain-less
petals keep today's y=0 behavior.

Tasks:
- [ ] Task: Resolve OQ-2 and implement the ground-height sampler in fe-terrain —
  either `elevation_at_world_xz` (inverse projection → elevation tile decode →
  bilinear sample → `(ele − origin_ele) × world_scale`) or a `TerrainChunk`-mesh
  raycast helper. (TDD: pure math first — sampling/interpolation or ray-mesh helper
  unit tests against a synthetic heightfield; then the terrain-facing wrapper.)
- [ ] Task: Ground-height seam: extend `ViewportCursorWorld` (or add a sibling
  resource) in fe-ui; bridge system in the main binary (pattern:
  `fractalengine/src/gpx_bridge.rs`) samples at placement time (context-menu open),
  not per-frame; fallback y=0 when no terrain/no sample. (TDD: bridge system test in
  the main binary asserting resource population with terrain active and fallback
  without.)
- [ ] Task: Wire placement: `viewport.rs` context menu / `GltfImport.position` use the
  snapped y; leave a wired seam note for the `CreateEmptyNode` TODO
  (context_menu.rs:51); document the flow in `fe-ui/src/AGENTS.md` +
  `fractalengine/src/AGENTS.md`. (TDD: dialog-position test mirroring the existing
  `active_dialog_context_menu_*` tests with a snapped y.)
- [ ] Verification: place on visibly elevated terrain → model sits on the surface;
  terrain-less petal unchanged (in-app check user-gated, listed in Phase 5 manual
  plan). `cargo test -p fe-terrain -p fe-ui -p fractalengine` targeted green.
  [checkpoint marker]

## Phase 5: Round-trip proof + full sweep

Goal: scripted end-to-end proof of the unit contract, the acceptance checklist, and
the track's single full workspace sweep.

Tasks:
- [ ] Task: Scripted round-trip integration test (FR-6): `SetPetalTerrain` with
  `world_scale = 0.01` → `ImportGltf` → read the node transform back through the
  API/handler read surface → `world_to_meters` matches expected meters; repeat for
  1.0 and terrain-less petal. Home: fe-test-harness scenario (preferred — exercises
  the real DbCommand loop) or fe-database integration test. (TDD: this test IS the
  red; wire until green.)
- [ ] Task: Manual verification plan (user-gated): (1) same glb on 1:1 vs 1:100 petal
  — correct relative size to terrain; (2) known-dimension asset (e.g. 2 m cube) —
  inspector Size reads authored meters on both scales; (3) snap-on-place on elevated
  terrain; (4) stamp spacing + path lengths + ruler HUD agree on one petal.
- [ ] Task: Full workspace sweep — `cargo fmt --check`,
  `cargo clippy -- -D warnings`, `cargo test` (workspace) — the ONE sweep for the
  track; fix fallout; confirm >80% coverage on new modules
  (`cargo tarpaulin` per workflow.md).
- [ ] Verification: sweep green; acceptance criteria 1-3 evidenced (tests + manual
  plan executed); AGENTS.md docs in place (fe-format §scale, fe-terrain §scale,
  fe-database placement, fe-api §units, fe-ui §units). [checkpoint marker]
