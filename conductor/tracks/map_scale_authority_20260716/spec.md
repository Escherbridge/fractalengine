---
type: Track Spec
title: Map-Authoritative Scale Consistency
description: The petal's terrain/map world_scale is the single source of scale truth — model placement and every user-facing number (inspector, rulers, path lengths, stamp spacing, gimbal/transform displays) derive from one canonical accessor, consistently.
tags: [feature, map_scale_authority_20260716, pending]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Overview

Make the petal's map `world_scale` (world units per real meter) the single source of
scale truth for **everything that isn't terrain**: node placement defaults and every
user-facing length/position/size number. One canonical accessor, one formula
(`real_m = world / scale`, `world = real_m * scale`), routed through by placement
handlers and every UI conversion site.

**User directive (2026-07-16, binding):** "the map always sets the scale — this is about
having the UI scale numbers match what the scale is in the petal/terrain." NO per-asset
scale metadata; glTF assets are authored in meters (glTF spec) and the petal's
`world_scale` converts real meters to world units everywhere, one formula.

This track is the model/UI-consistency **complement** to
[hexon_scale_orchestration_20260712](../hexon_scale_orchestration_20260712/spec.md):
that track owns the scale *pipeline* (TilesetMeta fields, backfill, clamp — done) and
measurement *tools* (Phase 5 tape/area/bearing, Phase 6 graticule — remaining). This
track owns scale **correctness of placement + existing displays**. Do not duplicate its
remaining scope.

# Background (verified 2026-07-16)

The terrain side is already consistent:

- `fe-terrain/src/scale.rs` — pure math: `sanitize_world_scale` (finite, > 0, else 1.0),
  `clamp_world_scale_to_bounds`, `scaled_tile_size`, `world_to_real_height`.
- `fe-format/src/manifest.rs` — `TilesetMeta.native_scale` (world units per real meter),
  `ground_sample_distance_m`, `scale_bounds`, `derive_scale_from_bounds` backfill.
- `fe-terrain/src/petal_binding.rs` — `config_for_tileset` sets
  `world_scale = native_scale.unwrap_or(1.0)`; `apply_terrain_assignments` clamps to
  hexon-authoritative bounds; `TerrainConfig::effective_world_scale()` (config.rs:80)
  is the render path's accessor (terrain_plugin, splat render, ruler_plugin all use it).
- `fe-ui/src/dialogs/hexon_manager.rs::render_scale_controls` — presets 1:1 human …
  1:1000 region + log slider bounded by `scale_bounds`; emits `UiAction::PetalSetMapScale`
  → `actions/hexon.rs::set_petal_map_scale` persists via `SetPetalTerrain`.

Everything else is not:

- **Placement ignores scale.** `fe-database/src/handlers/crud.rs::import_gltf_handler`
  (:348) and `create_node_handler` (:183) hardcode `scale: [1.0, 1.0, 1.0]`. The
  `SceneChange::NodeAdded` / `DbResult` echoes in `fe-database/src/lib.rs` (:368, :397)
  hardcode the same. A 10 m-tall glTF house dropped on a 1:100 petal renders 100× too
  big relative to the terrain.
- **Every UI conversion is a local copy.** The 2026-07-16 UX tracks
  (`inspector_units_width`, `gpx_stamp_persistence`, `path_interaction`) went metric,
  but each carries its own sanitize/convert copy: `actions/hexon.rs::clamp_to_bounds`
  (:218), `verse_manager/path_asset_reconcile.rs::sanitize_world_scale` (:250, f32),
  `node_manager/path_segment_interaction.rs::guard_world_scale` (:108), inline sanitize
  in `terrain_map/mod.rs::tileset_to_terrain_json` (:123), inspector conversions in
  node_manager, plus the raw un-sanitized read in
  `verse_manager/db_results/terrain.rs` (:28) that populates `PetalMapState.world_scale`.
  Same ~10-line formula, six-plus homes, drift guaranteed.
- **Terrain-less petals:** no terrain JSON → `PetalMapState.world_scale` defaults 1.0
  (`terrain_map/mod.rs:43`); verified — the canonical accessor must encode this default.
- **y=0 placement:** `fe-ui/src/plugin.rs::update_viewport_cursor_world` (:514-559)
  projects the cursor onto the infinite Y=0 plane, so all placed models land at ground
  zero regardless of terrain elevation (known open follow-up from
  ultrapilot_4tracks_20260712).

# Functional Requirements

### FR-1 — One canonical scale accessor (P0)
A single pure, Bevy-free module `fe-format/src/scale.rs` becomes the canonical home of
world-scale math:
- `sanitize_world_scale(f64) -> f64` and `clamp_world_scale_to_bounds(f64, Option<[f64;2]>) -> f64`
  (semantics identical to today's `fe-terrain/src/scale.rs`).
- `world_scale_from_terrain_json(Option<&serde_json::Value>) -> f64` — reads
  `world_scale` + `scale_bounds` keys; sanitize + clamp; **returns 1.0** for
  `None`/null/missing/invalid (the terrain-less-petal default).
- `meters_to_world(m, scale) -> f64` and `world_to_meters(w, scale) -> f64` — THE formula.
- `fe-terrain/src/scale.rs` keeps its public API by delegating/re-exporting from
  `fe_format::scale` — existing fe-terrain call sites (terrain_plugin, ruler_plugin,
  splat, petal_binding, `effective_world_scale`) and the in-flight
  hexon_scale_orchestration Phase 5/6 seams are untouched.
- **Acceptance:** fe-terrain's existing scale tests pass unchanged against the
  delegated implementation (proof of parity); the JSON accessor covers
  null / missing key / non-finite / ≤0 / out-of-bounds-clamped cases.

### FR-2 — Model placement honors the map scale (P0)
- `import_gltf_handler` and `create_node_handler` read the petal's terrain JSON from
  the DB (`SELECT terrain FROM petal WHERE petal_id = $petal_id`), derive
  `ws = world_scale_from_terrain_json(...)`, and write node
  `scale: [ws, ws, ws]` (uniform) by default. Terrain-less petal → `[1,1,1]`
  (unchanged behavior).
- Handlers return the applied scale; the `SceneChange::NodeAdded` and
  `DbResult::NodeCreated`/`GltfImported` echoes (`fe-database/src/lib.rs:368/:397`)
  carry the real value instead of hardcoded `[1,1,1]`, so the spawned entity and the
  UI tree match the row.
- Per-node override preserved: only the *initial placement default* changes; inspector
  Size edits and `UpdateNodeTransform` behave exactly as today.
- **Existing nodes untouched — no migration.**
- The scale-lookup helper (e.g. `petal_world_scale(db, petal_id)`) is exposed within
  fe-database handlers so `mcp_scene_primitives_20260716`'s `place_asset` seam can
  consume the identical placement math.
- **Acceptance:** placing the same glb on a `world_scale = 1.0` petal and a
  `world_scale = 0.01` (1:100) petal yields node rows with scale `[1,1,1]` and
  `[0.01,0.01,0.01]` respectively — correct relative size to terrain.

### FR-3 — UI numbers audit: every surface derives via the accessor (P0)
Every displayed length/position/size is real meters computed by
`fe_format::scale::world_to_meters`; every input field that accepts meters converts
back via `meters_to_world`. Audit inventory (each surface fixed or verified-consistent):

| Surface | Site | Action |
|---|---|---|
| Inspector position (m) / rotation (deg) / size (m) | node_manager (landed inspector_units_width) | re-route conversion through accessor |
| Gimbal drag deltas / transform broadcast readouts | node_manager/gimbal_interaction.rs, transform_broadcast.rs, viewport_labels.rs | audit; convert displays to meters via accessor |
| Ruler HUD scale bar | fe-terrain/ruler_plugin.rs | verify only — already `effective_world_scale()` (same formula post-FR-1) |
| Tape / area / bearing tools | hexon_scale_orchestration Phase 5 | NOT ours — coordinate: they inherit the formula via the fe-terrain delegation |
| Waypoint move / path point readouts | node_manager/path_point_interaction.rs | audit; convert via accessor |
| Path segment lengths (landed path_interaction) | path_segment_interaction.rs (`guard_world_scale`) | replace local guard with accessor |
| Stamp spacing (landed gpx_stamp_persistence) | path_asset_reconcile.rs / path_asset_materialize.rs | replace local f32 `sanitize_world_scale` with thin shim over accessor |
| Node AABB size display | inspector (landed) | re-route through accessor |
| GltfImport dialog position display | dialogs/gltf_import.rs:92-101 (raw world units) | display meters |
| `PetalMapState.world_scale` populate | verse_manager/db_results/terrain.rs:28 (raw read) | route through `world_scale_from_terrain_json` |
| Scale controls clamp | actions/hexon.rs::clamp_to_bounds:218 | replace with accessor clamp |
| terrain JSON emit sanitize | terrain_map/mod.rs:123 | replace inline sanitize with accessor |

- `PetalMapState.world_scale` remains the UI's cached live value; the *math* is written
  once in `fe_format::scale`. The duplicate `UiManager.world_scale` mirror
  (`plugin.rs:107`) is consolidated onto or documented against `PetalMapState` (OQ-3).
- **Acceptance:** the inspector Size of a known-dimension test asset reads its authored
  meters on any map scale; a grep for local sanitize/guard copies in fe-ui finds only
  thin shims that delegate to `fe_format::scale`.

### FR-4 — API unit contract documented (P0)
REST/WS/MCP transform payloads stay **world units on the wire** (positions, node scale
factors); meters are a UI-edge concern converted via the accessor. Documented in
`fe-api/AGENTS.md` §units (with the formula and a pointer to `fe_format::scale`) and
one-line doc pointers on the transform DTOs in `fe-api/src/types.rs`.
- Rationale: the wire mirrors the DB (source of truth); replication/sync stay
  scale-agnostic; no double conversion when a petal's scale changes after nodes exist;
  BI-egress consumers get raw values and can join the petal's `world_scale` themselves.
- **Acceptance:** AGENTS.md section exists; a scripted client can read a petal's
  `world_scale` and convert any node transform to meters with the documented formula.

### FR-5 — Terrain-height snap-on-place (P1, in scope)
Placement default gets the other half of "models match the map": when the active petal
has terrain, the placement y snaps to the terrain height at (x,z) instead of 0.
- **Scope decision — IN (justification):** a correctly-scaled house floating above or
  buried under a 1:100 mountain still contradicts the directive; placement *defaults*
  are this track's charter, while hexon_scale owns measurement *tools*. Kept as its own
  phase so it is independently de-scopable if elevation-sampling plumbing balloons.
- Implementation seam: fe-ui gains a ground-height resource (extension of
  `ViewportCursorWorld`); a bridge system in the main binary (precedent:
  `fractalengine/src/gpx_bridge.rs` — fe-ui cannot see fe-terrain per C6) populates it
  by sampling terrain elevation at the cursor (x,z) at placement time (context-menu
  open), NOT per-frame. Fallback: y=0 plane when no terrain / no sample (today's
  behavior). Sampling method (elevation-tile decode + bilinear vs. raycast against
  spawned `TerrainChunk` meshes) is OQ-2, resolved at the phase's red stage — both live
  behind the same resource seam.
- API/MCP callers keep explicit, caller-supplied y (documented in FR-4's contract).
- **Acceptance:** right-click-placing a model on visibly elevated terrain lands it on
  the surface; on a terrain-less petal behavior is unchanged (y=0).

### FR-6 — Scripted round-trip proof (P0)
An integration test scripts the full loop: set the petal's terrain `world_scale` →
place a node (`ImportGltf`) → read the transform back via the API/handler surface →
convert with the documented formula → matches expected meters.
- **Acceptance:** the round-trip test passes for `world_scale ∈ {1.0, 0.01}` and for a
  terrain-less petal (implicit 1.0).

# Non-Functional Requirements

- **NFR-1 — One formula.** After this track, exactly one implementation of
  sanitize/clamp/convert exists (`fe_format::scale`); everything else is a re-export or
  a thin, tested shim (f32 adapters allowed).
- **NFR-2 — Layering.** fe-ui gains NO fe-terrain dependency (C6 of
  hexon_scale_orchestration stands). New edges are limited to fe-database → fe-format
  and fe-ui → fe-format (both Bevy-free, serde-level).
- **NFR-3 — Back-compat.** Existing node rows untouched (no migration); terrain-less
  petals behave identically (1.0); no `.hexon`/terrain-JSON shape change.
- **NFR-4 — Purity/testability.** All new math and the JSON accessor are Bevy-free and
  unit-tested; >80% coverage on new code.
- **NFR-5 — Quality gates (workflow.md).** `cargo fmt --check`,
  `cargo clippy -- -D warnings`, `///` one-liner doc comments with "why" in directory
  `AGENTS.md`, no `unwrap()`/`expect()` in production paths, TDD red→green per task.
  Test execution policy: per task, run only the task's own (new/changed) test target
  inline; the **full workspace sweep runs exactly once, at the end of the track**.

# User Stories

**US-1 — Operator places a model on a scaled map.**
As an operator, I want a glTF model authored in meters to appear at the correct size
relative to my petal's terrain, so my digital twin is proportionally true.
- Given a petal at 1:100 (`world_scale = 0.01`), When I import a 10 m-tall glb, Then it
  spawns with node scale `[0.01,0.01,0.01]` and stands in correct proportion to the map.

**US-2 — Operator trusts the numbers.**
As an operator, I want every length/position/size shown anywhere in the UI to be real
meters derived from the map's scale, so the inspector, path lengths, stamp spacing, and
rulers never disagree with each other.
- Given a known 2 m-cube asset, When I inspect it on any map scale, Then Size reads 2 m.

**US-3 — Integrator reads the API.**
As a script/BI author, I want a documented unit contract, so I can convert wire values
to meters deterministically.
- Given the petal's `world_scale`, When I read a node transform via REST/MCP, Then
  `value / world_scale` yields meters per the documented contract.

**US-4 — Operator places on terrain.**
As an operator, I want placed models to land on the ground surface, not at y=0.
- Given elevated terrain, When I right-click → Add GLTF Model, Then the model sits on
  the terrain surface at that point.

# Technical Considerations / Code Seams

- **Accessor home (key decision):** `fe-format/src/scale.rs`. fe-format is Bevy-free,
  already owns scale *metadata* (`native_scale`, `scale_bounds`,
  `derive_scale_from_bounds`), and sits right in the dependency graph: fe-api and
  fe-terrain already depend on it; fe-database and fe-ui add a light edge. Alternative
  considered — canonical home in fe-database (fe-ui/fe-api already depend on it) —
  rejected: fe-terrain cannot depend on fe-database (drags SurrealDB into the render
  path), which would leave two formula copies, i.e. the disease this track cures.
- **Placement:** `crud.rs::import_gltf_handler`/`create_node_handler` +
  `lib.rs` dispatch echoes (:357-413); `fe-test-harness/src/peer.rs:222` and
  `scenarios/blob_roundtrip.rs` exercise ImportGltf. Spawn side consumes
  `NodeDto.scale` (`fe-ui/src/verse_manager/db_results/nodes.rs`).
- **Scale flows to UI:** terrain JSON → `DbResult::PetalTerrainLoaded` →
  `db_results/terrain.rs` → `PetalMapState.world_scale` → mirrored to
  `CameraScaleSettings` (fe-renderer) and `UiManager.world_scale`.
- **Cursor/placement position:** `plugin.rs::update_viewport_cursor_world` (Y=0 plane)
  → `viewport.rs:147` context menu `world_pos` → `dialogs/context_menu.rs:40` →
  `GltfImport.position`. Note context_menu.rs:51 `TODO: CreateEmptyNode at cursor` —
  wire the same snap seam if it lands.
- **Parallel-track coordination:**
  - `hexon_scale_orchestration_20260712` (P0, in-flight): owns measurement tools
    (Ph5/6). We keep `fe-terrain/src/scale.rs`'s public API stable (delegation, not
    move-and-break) so its remaining phases are unaffected.
  - `mcp_scene_primitives_20260716` (fe-api/fe-database, parallel): its `place_asset`
    consumes FR-2's `petal_world_scale` helper — keep it callable from fe-api-facing
    handler code.
  - `road_builder_ux_20260716` (fe-ui, parallel): road lengths display through the same
    `fe_format::scale` accessor.
- **Docs convention:** terse one-line `///`; rationale in `fe-format/src/AGENTS.md`
  (§scale — new), `fe-terrain/src/AGENTS.md` (§scale — delegation note),
  `fe-database/src/AGENTS.md` (placement default), `fe-api/AGENTS.md` (§units),
  `fe-ui/src/AGENTS.md` (accessor routing rule).

# Out of Scope / Non-Goals

- Per-asset scale metadata (directive: glTF assets are authored in meters, period).
- CRS reprojection (recorded, not reprojected — unchanged from hexon_scale).
- Measurement tools (tape/area/bearing, graticule) — owned by
  hexon_scale_orchestration_20260712 Phases 5-6.
- Auto-detecting misauthored assets (wrong-unit glbs are the author's problem).
- Migration/rescaling of existing node rows.
- Camera behavior changes (CameraScaleSettings sync stays as-is).

# Open Questions

- **OQ-1:** GltfImport dialog position readout — display meters only, or meters with a
  world-units tooltip? Default: meters only (consistent with inspector).
- **OQ-2:** Snap-on-place sampling method — elevation-tile decode + bilinear sample
  (pure-testable, needs projection inverse) vs. raycast against spawned `TerrainChunk`
  meshes in the bridge (simple, needs render world). Resolve at Phase 4 red stage; both
  sit behind the same ground-height resource seam.
- **OQ-3:** Consolidate `UiManager.world_scale` (plugin.rs:107) onto
  `PetalMapState.world_scale` vs. keep the mirror with a documented single write-path.
  Default: consolidate reads onto PetalMapState; decide in Phase 3.

# Audit evidence (2026-07-17)

Board-hygiene audit confirms this track's premise with two live, disagreeing
scale authorities: `fe-terrain/src/ruler_plugin.rs:72` reads
`config.effective_world_scale()` (hexon-bounded terrain accessor) while
`fe-ui/src/node_manager/path_segment_interaction.rs:186` reads
`guard_world_scale(petal_map.world_scale)` (raw `PetalMapState` cache) — the
ruler HUD and path-length readouts derive the same number from different
sources. Unification must route BOTH through the canonical
`fe_format::scale` accessor (FR-1/FR-3), not just the fe-ui copies.
