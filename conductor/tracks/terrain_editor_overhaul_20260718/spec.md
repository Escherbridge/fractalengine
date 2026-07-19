---
type: Track Spec
title: Unified Spatial Editor + Analytics-First Terrain Proposal Overlays
description: Re-spec of the editor as one typed-selection, object-type-aware left-click operation surface (fixing the path-node/gimbal and path-delete-stamp QA bugs along the way), plus a Cities-Skylines-map-editor-inspired terrain editor whose edits are NON-destructive PROPOSED overlays separate from true terrain, reportable on real scale and geometry
tags: [feature, editor, terrain, selection, gimbal, path-assets, analytics, measurement, terrain_editor_overhaul_20260718, pending]
timestamp: 2026-07-18T00:00:00Z
resource: ./metadata.json
---

# Specification: Unified Spatial Editor + Analytics-First Terrain Proposals

**Track ID:** `terrain_editor_overhaul_20260718`
**Priority:** P0 UX / P0 Analytics (user-directed 2026-07-18)
**Crates:** `fe-ui` (selection read-model, dispatch, gimbal, proposal UI, report
panel, settings consumer), `fe-terrain` (proposal overlay layer + rendering,
measurement math wiring), `fe-renderer` (overlay/ghost material; reads the
read-only `TerrainHeightField`), `fractalengine` (gpx_bridge delete cascade)

## Overview

Verbatim user asks (2026-07-18):

> "The nodes for paths are selectable but only in the tools context menu and is
> not possible to do with the pen or the select tool, also no gimbal is shown on
> path selection which should be possible."

> "Paths deleted that had stamps should also remove the gltf objects that were
> tied to them — this is almost like layering assets on a texture to paths but
> allowing for more integrated operations."

> "We may want to introduce more operations on left click and execute on the
> operations and allow options to be aware of the object types selected. It would
> be best to spec out the terrain editor again taking inspiration from Cities
> Skylines [map editor]. However we should remember that we are an analytical
> solution so the volumes remain as proposed edits only and are separate from the
> true terrain environment with the opportunity to report on the scale, and
> geometry."

This is one editor thrust with a single spine: **know what kind of object is
selected, then make left-click do the right, object-aware thing** — and extend
that same spine to a terrain editor whose outputs are **proposed analytics
overlays**, never destructive terrain writes.

### Ground truth (from the 2026-07-18 exploration sweep)

- **Two selection authorities, codified as a non-merge:** `NodeManager.selected`
  (drives the gimbal; `fe-ui/src/node_manager/mod.rs:44`) vs `PathEditorState`
  (`editing_track_id`/`selected_point`/`selected_segment`; `fe-ui/src/gis/mod.rs:130-207`).
  Whole-track selection was already bridged (`viewport_pick::open_track_on_select`,
  `viewport_pick.rs:92-144`); **vertex/segment selection is not** — it lives only
  in `PathEditorState` and never populates `NodeManager.selected`. The split is
  **codified in `conductor/code_styleguides/ui_ux.md §5` — do NOT merge the two
  stores.**
- **Gimbal reads only `NodeManager.selected`** (`gimbal_interaction.rs:308`) and
  is **double-gated off** during Select/Pen (`gimbal.rs:107-109`,
  `gimbal_interaction.rs:167`). So a selected path vertex/segment shows no gimbal
  for two reasons: it never enters `NodeManager.selected`, and even a whole ribbon
  won't draw a gimbal while a path-editing tool is active.
- **No object-type model:** dispatch branches on the `Tool` enum
  (`Select|Move|Rotate|Scale|Pen`, `panels/toolbar.rs:14-22`) and per-frame
  `ClickPriority` geometry (`node_manager/router.rs:13-26`), never on the *type*
  of the selected object. `NodeSelection` carries only `entity` + `node_id`
  string.
- **Path-delete stamp cascade — back-reference already exists:** every stamp
  entity carries `PathAssetInstance { source_track_id }` (`verse_manager/spawn.rs:103-106`).
  The materializer is the only despawner and is gated on `PathAssetCache`
  (`path_asset_materialize.rs`). `DbResult::NodeDeleted` (fe-ui arm,
  `db_results/mod.rs:151` → `handle_node_deleted`, `nodes.rs:106`) **never
  invalidates the cache**, so the deleted track's entry survives as a zombie:
  stamps orphan in-session AND **resurrect on petal re-entry**. Both
  `PathAssetCache::invalidate` and `PathAssetApplied::invalidate` already exist.
- **Terrain is load-and-display only** — zero edit tooling (literal
  `"(placeholder — future terrain tools)"`, `panels/tool_panel.rs:601-613`).
  Building blocks that DO exist: the display-overlay `LayerStack`/`MapLayer`
  pattern (`fe-terrain/src/layers/`), the read-only bilinear `TerrainHeightField`
  sampler (`fe-renderer/src/terrain_height.rs`), and **tested-but-unwired**
  geometry math `polygon_area_m2` / `world_to_real_distance` / `bearing_deg`
  (`fe-terrain/src/ruler.rs`, zero live callers).

## Functional Requirements

- **FR-1 — Typed selection read-model (facade, not a storage merge).** A
  `SelectionKind` read-model — `{ Empty, Node(Entity), PathTrack(track_id),
  PathVertex{track_id, idx}, PathSegment{track_id, idx}, Stamp(Entity),
  TerrainProposal(id) }` — computed each frame as a **projection over the existing
  authorities** (`NodeManager.selected` + `PathEditorState.*`), so any system can
  ask "what kind of thing is selected?" without unifying storage (respects
  ui_ux.md §5). Stamps and terrain proposals gain first-class selection
  representation (they have none today).
- **FR-2 — Object-type-aware left-click operation dispatch.** An `Operation`
  table keyed on `(active Tool, SelectionKind, hit-target type)` so left-click
  executes the context-appropriate action — e.g. a vertex vs a segment vs a stamp
  vs a terrain cell each do the right thing — with headroom to grow the op set
  (the "more operations on left click" ask). Preserve the first-claim-wins
  `ClickArbiter` and the `ViewportRect`/egui gating; this extends the router
  (`node_manager/router.rs`), it does not replace it. Coordinate the terrain-cell
  op seam with `road_builder_ux` so both share one dispatch path.
- **FR-3 — Gimbal on path (and any typed) selection** *(QA bug)*. Derive the
  gimbal target from the FR-1 read-model so a selected vertex/segment/track yields
  a transform to draw, and **ungate the gimbal for path selections in the editing
  tools**. Vertex gimbal drags the vertex; segment gimbal drags the segment; track
  gimbal drags the ribbon (the whole-path bake already exists). No storage merge —
  the gimbal reads the read-model, the read-model reads both authorities.
- **FR-4 — Path-delete cascades to stamped glTF** *(QA bug)*. Route
  `DbResult::NodeDeleted` (fe-ui arm) → `PathAssetCache::invalidate(node_id)` +
  `PathAssetApplied::invalidate(node_id)`. Evicting the zombie entry lets the
  materializer's existing orphan-cleanup despawn every `PathAssetInstance` whose
  `source_track_id` matches — killing both the in-session orphan and the
  petal-re-entry resurrection. This realizes the "layering assets on paths /
  integrated operations" vision: a path's lifecycle owns its stamped assets. No
  new data model needed (the back-reference exists).
- **FR-5 — Analytics-first proposed-overlay terrain editor (non-destructive).**
  A `TerrainProposal` record set + a dedicated **proposal overlay layer** (parallel
  to `MapLayer::{satellite,terrain,gpx_track,geojson_overlay}`) holding edit
  operations — raise / lower / flatten / ramp / slope / pad / cut / fill volumes —
  each rendered as **distinct ghosted/tinted geometry atop** the read-only
  `TerrainHeightField`. **The true heightfield and loaded tileset are NEVER
  written** (NFR-1). Cities-Skylines map editor is the interaction reference (tool
  palette, brush modes), reframed so every "brush" emits a proposal record + a
  report, not a destructive terrain write. Proposals persist **additively** via
  the `SetPetalTerrain` round-trip in a proposals block that does not touch the
  tileset config (a hexon proposal-layer is the later option — see open Q).
- **FR-6 — Measurement + scale/geometry reporting.** Wire the tested-but-unwired
  `polygon_area_m2` / `world_to_real_distance` / `bearing_deg` into (a) interactive
  measure tools (tape / area / bearing pick) and (b) a **report panel** on any
  selected proposal or geometry: real-unit extent (m), footprint area (m²),
  cut/fill volume (m³), slope (%), bearing (°) — the analytics payoff of "report
  on scale and geometry." Uses `world_scale`/`effective_world_scale` for the
  metric frame and shows the honest "no map scale" state when unscaled
  (RulerPlugin precedent). Coordinate scope with `hexon_scale_orchestration`
  Phase 5 (that track owns the measure TOOLS; this owns the report PANEL + the
  proposal geometry they report on).

## Non-Functional Requirements

- **NFR-1 — True terrain is immutable (the analytics contract).** No editor
  operation may write `TerrainHeightField` or the loaded tileset. Proposals are a
  separate overlay/record set; ground truth stays ground truth so reports compare
  proposed-vs-true honestly.
- **NFR-2 — Proposed geometry counts against budgets.** Proposal overlay meshes
  spawn through the same residency/mesh-budget gating (`spawn_allowance`,
  `mesh_instance_watchdog`) — no bypass. Over-consumption prevention applies to
  proposals too.
- **NFR-3 — Respect the codified selection split (ui_ux.md §5).** FR-1 is a
  read-model facade; it must not merge `NodeManager.selected` and `PathEditorState`
  storage. Highlighting stays distinguishable per authority.
- **NFR-4 — Reporting is metrically honest.** When no map scale is set, report in
  world units with the explicit "no map scale" chip; never fabricate meters.
- **NFR-5 — Deletions are cancel-safe and idempotent.** FR-4's cascade must be a
  no-op when a track has no stamps and must not double-despawn under repeated
  `NodeDeleted` for the same id.

## Out of scope

- P2P streaming / residency ledger / application settings persistence — owned by
  `p2p_asset_streaming_20260718` (D-73…D-78). This track **consumes** the budget +
  `AppSettings` those produce; it does not build them.
- Road/path segment placement input (straight/curve/freeform) — owned by
  `road_builder_ux_20260716`. FR-2 shares the left-click dispatch seam; do not
  duplicate the placement layer.
- Destructive terrain export / real-DEM authoring / hexon terrain baking —
  foundry-adjacent, not analytics core (an edited DEM would violate NFR-1).
- Camera hardening (ground-clamp / scale-aware distances / easing) — landed in
  `ux_interaction_hardening_20260718` FR-5.
- Scale correctness plumbing (world_scale accessor) — `map_scale_authority_20260716`.

## Open questions

- Does FR-6 subsume `hexon_scale_orchestration` Phase 5 (measurement tools), or
  split (that track = tools, this = report panel + proposal geometry)? Same
  subsumption pattern as p2p FR-4 vs runtime_instance_guardrails FR-6.
- Proposal persistence for v1: petal-config proposals block now vs a hexon
  proposal-layer later — which ships first? (Petal-config is the lower-risk v1.)
- Do terrain proposals need RBAC (Editor+ to propose, mirroring node writes via
  `fe-policy`)? Likely yes.
- Proposal geometry LOD: do far-away proposals evict like terrain chunks, or stay
  resident because they're user intent? (Lean: evict via the same ledger, but
  keep the record.)
