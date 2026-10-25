---
type: spec
title: Terrain proposal delta visualization + selected-volume reporting
track: terrain_proposal_delta_viz_20261025
tags: [feature, terrain, earthwork, analytics, bim, fe-terrain, fe-renderer, fe-ui, terrain_proposal_delta_viz_20261025]
created: 2026-10-25
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: in-app finding F5 (2026-10-25)
depends_on: [sculpt_earthwork_regions_20260725, terrain_editor_overhaul_20260718 (archived = satisfied)]
---

# Specification: Terrain proposal delta visualization + selected-volume reporting

**Track ID:** `terrain_proposal_delta_viz_20261025` · **Type:** feature · **Priority:** P0 (Spatial Builder Program follow-on)
**Crates:** `fe-terrain` (pure delta/volume math + render systems), `fe-renderer` (color ramp, materials, cross-crate seam types), `fe-ui` (preview producer + volume report panel)
**Citations computed at:** `951adb2`. Symbol names are stable. Line numbers are not, so re-grep before editing.

## 1. User vision (2026-10-25, in-app F5)

> Terrain controls post and save proposals as nodes, but "there is no visible
> transformation to show the delta we are proposing — which is the whole point."

The ask has two parts.

1. **See the delta.** A proposal should visibly transform the scene against the current terrain. That means showing what is cut, what is filled, and by how much, live while editing and before commit.
2. **Report on a selected volume and everything in it.** The report should cover geometry and scale, excavation estimates (cut/fill/net in real units), and, later, the other assets in that volume (pipes, electrical wiring) to guide the work.

## 2. Industry anchor

Civil design tools converge on one pattern. This track adopts it.

| Practice | Civil 3D / InfraWorks / Trimble Business Center | This track |
|---|---|---|
| **Existing-vs-proposed surface pair** | Volume surfaces: Civil 3D TIN volume surface, TBC surface-to-surface volume. The *difference* surface is the analysis object. | Read-only `TerrainHeightField` (pristine, never baked) vs the composite proposed surface. NFR-1 already guarantees the pair exists. |
| **Color-graded cut/fill map draped on terrain** | Red = cut, blue = fill, graded by depth bands, with a neutral band near zero. Civil 3D elevation-range / cut-fill analysis; TBC cut/fill map. | P1: a per-vertex diverging heatmap drape, plus a legend. |
| **Ghost / proposed surface preview** | Proposed grading surface shown translucent over existing ground. Game builders (Cities: Skylines, Planet Coaster terraform) show a live ghost before commit. | P1: a lattice-sampled translucent proposed surface, live from the armed tool. |
| **Volume report** | Cut / fill / net m³, per-surface and per-region tables, mass-haul diagram. | P2: selected-region panel (area, cut, fill, net, per-item breakdown, contained nodes). Mass-haul is deferred (§9). |
| **Cross-sections** | Sample lines → section views showing existing vs proposed profile. | P3 (deferable). |
| **Utility / clash awareness** | Pipe networks / pressure networks in Civil 3D; clash detection (Navisworks). | P4 (future, minimal commitment). |

## 3. Ground truth: what exists (inventory @ `951adb2`)

The cut/fill **math is done and tested**. The visible delta is missing because of *how* the ghost is built and *when* it rebuilds. It is not missing because there is no ghost. The four root causes are R-1 to R-4 below.

### 3.1 Root causes of "no visible transformation"

- **R-1: The ghost is a flat lid, not a surface.** `proposal_overlay_mesh_data` (`fe-terrain/src/terrain_proposal.rs:258`) earcut-triangulates **only the footprint perimeter vertices** (`positions` = one vertex per footprint point, :273-289). The interior has no vertices, so the ghost is a planar polygon spanning the footprint. Over relief it slices through the terrain: part buried, part floating. A Raise of 2 m over a hill looks the same as a tinted decal. The delta is not representable in that mesh.
- **R-2: No pre-commit path at all.** `render_terrain_proposals` (`fe-terrain/src/terrain_plugin.rs:1167`) rebuilds only on an `ActivePetalTerrain.revision` change (`proposals_need_rebuild`, :1195). That change happens only after `TerrainProposalAdd` → `SetPetalTerrain` → DB → `PetalTerrainLoaded` → bridge. Nothing renders while the user is choosing op/radius/delta, drawing a shape, or dragging a brush.
- **R-3: Baked regions erase the comparison.** Earthwork regions are skipped by the ghost path (:1228) and baked straight into chunk meshes (`bake_earthwork_regions`, :1432). The new relief appears, but nothing shows *what changed* relative to before. The pristine reference survives only in `PristineChunkHeights` and in the never-baked `TerrainHeightField`.
- **R-4: The report shows an approximation while the true figure is computed elsewhere.** The right-sidebar report (`fe-ui/src/panels/proposal_report_panel.rs`) uses `area × |delta|` (`compute_report`, :321; `cut_fill_totals`, :123). Its own doc comment (:114-122) admits the true value lives on the region node's `cut_volume_m3`/`fill_volume_m3` and is not read there. The true palette-proposal figure (`proposal_cut_fill`, `terrain_proposal.rs:396`) is never called outside tests.

### 3.2 Reusable machinery (entry points)

**Pure math (fe-terrain, Bevy-free, always compiled, NFR-1 read-only base `&impl Fn(f32,f32)->Option<f32>`)**

| Entry point | Location | Reuse |
|---|---|---|
| `TerrainProposal` / `TerrainOp` (+ `is_earthwork_region`) | `fe-terrain/src/terrain_proposal.rs:53` / `:13` / `:75` | Record model; region discriminator |
| `apply_proposal_over_base` | `terrain_proposal.rs:227` | Per-point proposed height for every palette op. This is the **proposed-surface sampler** for P1 lattices |
| `proposal_overlay_mesh_data` | `terrain_proposal.rs:258` | **Replaced** by the lattice ghost (R-1) |
| `point_in_polygon` (pub(crate)), `footprint_bounds` (pub(crate)) | `terrain_proposal.rs:155`, `:125` | Footprint gating / bounds |
| `proposal_report` → `ProposalReport` | `terrain_proposal.rs:339` | Extent / area / slope / bearing (keep); its `volume_m3` is representative-only |
| `proposal_cut_fill` | `terrain_proposal.rs:396` | **True** separated cut/fill for any palette op (unused in prod today) |
| `Footprint { Brush, Circle, Rect, Polygon }` (+ `contains`, `to_polygon`, `bounds`) | `fe-terrain/src/sculpt/mod.rs:70` | Selection footprints for P2 |
| `SculptOp`, `proposed_height`, `region_mean_height` | `sculpt/mod.rs:22`, `:173`, `:153` | Region-op proposed surface (Smooth needs the mean) |
| `cut_fill_volume` → `CutFill { cut_m3, fill_m3, scaled }` + `net_m3` | `sculpt/mod.rs:252`, `:231` | **The** volume integrator (prism/grid, O(step) tolerance, real units via `/scale`, `/scale²`) |
| `for_each_cell` (private) | `sculpt/mod.rs:306` | Lattice walk. Promote to `pub(crate)` for the delta lattice |
| `EarthworkRegion` + `cut_fill`, `parse_regions` | `sculpt/mod.rs:338`, `:382`, `:403` | Region record + per-region volume |
| `earthwork_surface_delta` (private, sequential record-order composite) | `terrain_plugin.rs:1381` | **Composite** proposed surface. Lift into `sculpt` so viz, bake and report share one composite (one truth) |
| `EARTHWORK_LATTICE` (128) / `region_grid_step` | `terrain_plugin.rs:1283` / `:1311` | Lattice resolution policy |
| `TerrainConfig::effective_world_scale` / `scale::sanitize_world_scale` | `fe-terrain/src/config.rs:106` / `fe-terrain/src/scale.rs:4` | Map-scale authority (N-1, NFR-4: never fabricate meters) |
| `viridis`, `ColorMode`, `compute_vertex_colors` | `fe-terrain/src/layers/style.rs:139`, `:10`, `:54` | Vertex-color ramp precedent (GPX tracks) |

**Render systems (fe-terrain, `render` feature)**

| Entry point | Location | Reuse |
|---|---|---|
| `render_terrain_proposals` + `ProposalOverlayEntity` / `ProposalRenderState` | `terrain_plugin.rs:1167` / `:170` / `:178` | Sole spawner/despawner of ghosts. Extended (not forked) to lattice ghost + heatmap |
| `ProposalOverlayBudget` (NFR-2 residency) + `MAX_PROPOSAL_OVERLAYS` | `terrain_plugin.rs:194` | Budget for new meshes (bridge: `fractalengine/src/terrain_bridge.rs:39` `sync_proposal_overlay_budget`) |
| `bake_earthwork_regions` (+ `EarthworkVolumeReport` writer) | `terrain_plugin.rs:1432` (writes at :1547) | Region bake. Its height field stays PRE-BAKE, which is exactly the "existing" surface the heatmap needs |
| `MapLayer::ProposalOverlay` + `sync_layer_visibility` | `layers/stack.rs`; `terrain_plugin.rs:1566` | Visibility toggle for the new overlays |

**Cross-crate seam (fe-renderer, the "fe-renderer-as-shared-seam" idiom: fe-ui must NOT depend on fe-terrain)**

| Entry point | Location | Reuse |
|---|---|---|
| `TerrainHeightField` / `height_at` | `fe-renderer/src/terrain_height.rs:59` / `:91` | Pristine base sampler |
| `op_tint`, `ghost_material`, `GHOST_ALPHA` | `fe-renderer/src/terrain_overlay.rs:21`, `:42`, `:12` | Ghost styling |
| `brush_ring` / `brush_overlay_positions` | `terrain_overlay.rs:74` / `:90` | Brush cursor precedent |
| `EarthworkVolumeReport` (Message, fe-terrain → fe-ui) | `terrain_overlay.rs:107` | **Template** for the new preview / volume-report seam types |

**fe-ui (producer + report)**

| Entry point | Location | Reuse |
|---|---|---|
| `ProposalEditState` / `ProposalRecord` | `fe-ui/src/terrain_proposal_state.rs:46` / `:31` | Committed palette set + `selected` |
| `render_controls_and_emit` (palette armed op/radius/delta → `TerrainProposalAdd`) | `fe-ui/src/panels/terrain_tools_panel.rs:139` (push at :232) | Armed-tool state = the **pre-commit draft** for preview |
| `SculptToolState` (+ `region_draft`) | `fe-ui/src/actions/terrain_proposal.rs:303` (:317) | Shape-mode draft footprint |
| `ActiveStroke` / `BrushGesture` / `handle_brush_interaction` | `fe-ui/src/node_manager/brush_interaction.rs:57` / `:64` / `:135` | Live brush stroke samples |
| `stroke_corridor_footprint` | `actions/terrain_proposal.rs:964` | Stroke → corridor polygon (the same footprint the commit will persist) |
| `EarthworkNodeMap`, `earthwork_node_properties`, `persist_earthwork_volumes` | `actions/terrain_proposal.rs:427`, `:504`, `:572` | Region ↔ node binding; true volumes on node properties (`cut_volume_m3`/`fill_volume_m3`, :419-420) |
| `render_report_body` / `cut_fill_totals` / `compute_report` | `fe-ui/src/panels/proposal_report_panel.rs:171` / `:123` / `:321` | Report host. Totals switch to true figures (R-4) |
| `render_proposal_report_section` | `fe-ui/src/ui_shell/right_sidebar.rs:545` (dispatch :330) | Section chrome host for P2 |
| `pick_ground_proposal` / `pick_ground_region` | `fe-ui/src/node_manager/context_pick.rs:192` / `:202` | Footprint-hit selection of proposals/regions |
| `PetalMapState` (mirrors `world_scale`) | `fe-ui/src/terrain_map/mod.rs:31` | Panel unit conversion |
| `VerseManager::find_petal` / `all_nodes`, `NodeEntry.position` | `fe-ui/src/verse_manager/mod.rs:124` / `:115` / `:81` | In-memory "everything inside the volume" enumeration (no DB round-trip, AGENTS anti-pattern #3) |

**Query / API (P2 egress parity, P4)**

| Entry point | Location |
|---|---|
| `nodes_in_bbox`, `nodes_within_radius` | `fe-query/src/gis.rs:70`, `:109` |
| `NodeKind`, `stamps_on_path_sql`, `earthwork_volume_sql` | `fe-query/src/spatial_nodes.rs:28`, `:86`, `:105` |
| `GET /api/v1/petals/{p}/earthwork/summary` | `fe-api/src/endpoint.rs:454`, route `fe-api/src/server.rs:162` |

Directory docs to read before editing: `fe-terrain/src/AGENTS.md` §terrain-proposals, §sculpt (incl. "Earthwork bake"); `fe-renderer/src/AGENTS.md` §terrain-overlay; `fe-ui/src/AGENTS.md` §terrain-proposal-editor; `fe-ui/src/actions/AGENTS.md` §sculpt; `fe-ui/src/panels/AGENTS.md` §terrain-tools.

## 4. Functional requirements

### P1: Visible delta (the core of F5)

- **FR-1: Lattice ghost surface.** A proposal renders as a translucent *proposed surface* sampled on a lattice inside the footprint (heights from `apply_proposal_over_base` / `sculpt::proposed_height`), not a perimeter lid. *Acceptance:* for a Raise over non-flat relief, ghost interior vertices exist and their Y = base + delta at each lattice point (within float epsilon). Vertex count is bounded by a documented cap. Fixes R-1.
- **FR-2: Cut/fill heatmap drape.** A per-vertex color-graded overlay draped on `max(existing, proposed) + lift` shows `dh = proposed − existing`: red family = cut (dh < 0), blue family = fill (dh > 0), intensity graded by `|dh|` in **real meters** up to a ramp max, transparent inside a ±neutral band. A legend (ramp + units + "no map scale" chip when unscaled) shows in the report section. *Acceptance:* color sign matches the cut/fill sign at every vertex; unscaled petals label world units (NFR-4).
- **FR-3: Live pre-commit preview.** While a terrain tool is armed, the current draft renders FR-1 + FR-2 and a live cut/fill/net readout, before any `TerrainProposalAdd` / `SculptShapeRegion` / `SculptBrushStroke` is sent. The draft can be the palette op + radius + delta at the cursor/placement, the shape-mode footprint (`region_draft` / circle / rect), or the in-progress brush corridor. Disarm, commit or petal switch clears it. *Acceptance:* the preview appears with zero DB traffic. Its cut/fill equals the committed figure for the same inputs (one integrator). Fixes R-2.
- **FR-4: Baked regions show their delta.** Committed earthwork regions (baked) get the FR-2 heatmap against the pristine `TerrainHeightField`. Committed palette proposals get FR-1 + FR-2. *Acceptance:* after commit, the region still visibly reads as "this much cut/fill here". Toggling the overlay layer hides it without un-baking. Fixes R-3.
- **FR-5: One composite truth.** Viz, bake and report use one composite proposed-surface function (the current private `earthwork_surface_delta`, lifted into `sculpt`). *Acceptance:* a test asserts that the heatmap's integrated dh equals `cut_fill_volume` for the same lattice, and that the bake uses the same composite (no drift).

### P2: Selected-volume report

- **FR-6: Volume selection.** The user selects a volume as an existing earthwork region (node or footprint pick), a palette proposal, or a drawn selection footprint (circle / rect / polygon, reusing sculpt shapes). The default volume is 2.5D: the footprint prism with unbounded vertical extent (OD-5).
- **FR-7: Volume report.** For the selection: footprint area, **true** cut m³, fill m³, net m³ (composite surface clipped to the selection), lattice step + stated tolerance, and units per map-scale authority. *Acceptance:* matches `cut_fill_volume` on analytic cases. Unscaled petals report world units with the chip. The `area × |delta|` approximation is removed from totals (R-4).
- **FR-8: Per-item breakdown.** The report lists every proposal/region overlapping the selection (standalone cut/fill clipped to the selection, overlap area, op, material, linked node). It also lists every scene node whose position lies inside the footprint (name, kind when known: earthwork / stamp / path / asset). Overlaps are flagged where the composite total differs from the per-item sum.
- **FR-9: Copyable output.** One "Copy as CSV/TSV" action for the table (BI egress spirit, roadmap). A REST/MCP selection-report endpoint is **not** in scope (OD-7).

### P3: Cross-sections (deferable)

- **FR-10:** Draw a section line across the terrain and show a profile of existing vs proposed elevation along it (cut/fill shaded between). Profile sampling is pure fe-terrain; the plot uses the egui painter (no `egui_plot` dependency exists in the workspace). Ships only if P1-P2 land with budget remaining.

### P4: Assets-in-volume (future, minimal commitment)

- **FR-11 (future):** Enumerate utility assets (pipes, conduits, cables) inside a selected volume, with a simple quantity takeoff: clipped length inside the footprint per utility type, plus count for point assets. Flag utilities intersecting the cut prism (excavation clash warning). **Precondition not met today:** there is no utility node classification. P4 needs a `node_kind: utility` + `utility_type` + depth/invert/diameter property contract first, and a true 3D volume (OD-5). This track only reserves the seam: the P2 per-item table is kind-extensible. It commits no code.

## 5. Non-functional requirements

- **NFR-1 (sacred, inherited):** never write the true terrain. Every new fn takes the base as read-only `&impl Fn`. `TerrainHeightField` stays pre-bake. A byte-identity test covers the new delta lattice path.
- **NFR-2 (residency):** new overlay meshes are bounded by `ProposalOverlayBudget` plus a per-lattice vertex cap and a per-petal total heatmap vertex cap (documented constant). Truncation warns and never panics.
- **NFR-3 (frame budget):** live preview recompute is throttled (OD-3). A coarse lattice while the input is changing and the full lattice on idle/release. No allocation in per-frame no-change paths (AGENTS anti-pattern #7).
- **NFR-4 (reporting honesty, inherited):** meters only when `world_scale` is finite > 0; otherwise world units + chip. Volumes state their lattice step/tolerance.
- **NFR-5 (layering):** fe-ui gains no fe-terrain dependency. All fe-ui ↔ fe-terrain traffic goes through fe-renderer seam types (Resource for the preview draft, Messages for reports) or terrain JSON.
- **NFR-6 (visibility):** all new overlays bind to the `ProposalOverlay` map layer (or a sub-toggle, OD-10) and honor `sync_layer_visibility` and hierarchy visibility.
- **NFR-7 (ui_ux):** legend and report follow `conductor/code_styleguides` ui_ux checklist (calm, never-blank empty states, units always shown).

## 6. Open decisions (user, ratify before build)

Recommended defaults are marked **(rec)**. Unratified decisions proceed on the recommendation, logged as an assumption in plan §0.

| ID | Decision | Options | Gates |
|---|---|---|---|
| **OD-1** | Heatmap color ramp | **(rec)** civil convention, diverging red (cut) / blue (fill) graded by depth with a transparent neutral band · colorblind-safe diverging orange/purple · user-switchable (both, default civil) | FR-2 |
| **OD-2** | Ramp max (depth at full saturation) | **(rec)** auto = max \|dh\| of the shown set, rounded up to a nice meter value, with a user override · fixed (e.g. 5 m) · fixed bands (0.5/1/2/5 m, Civil 3D-style) | FR-2 |
| **OD-3** | Live-update throttle | **(rec)** recompute on change, debounced ~100 ms; coarse lattice (64) while changing, full (128) on idle/release · every frame at coarse · only on release | FR-3, NFR-3 |
| **OD-4** | Where the P2 report lives | **(rec)** extend the existing right-sidebar **Proposal report** section (rename to "Volume report"; no new `RightSidebarSection` variant) · Inspector tab for the selected region node · its own new sidebar section | FR-7/8 |
| **OD-5** | Volume definition | **(rec)** 2.5D footprint prism now; reserve an optional elevation band for P4 · true 3D box now | FR-6, FR-11 |
| **OD-6** | Overlapping proposals in totals | **(rec)** totals = composite surface (record-order, matches the bake); per-item = standalone vs existing; overlap flagged · totals = sum of per-item | FR-7/8 |
| **OD-7** | Selection report egress | **(rec)** in-app + Copy-as-CSV only this track; REST/MCP `volume-report` endpoint as a follow-up · add the endpoint in P2 | FR-9 |
| **OD-8** | Ghost + heatmap default visibility | **(rec)** both on, ghost toggle in Options · heatmap only, ghost opt-in · ghost only, heatmap opt-in | FR-1/2 |
| **OD-9** | Pre-commit model | **(rec)** preview the *armed tool's* draft (no new state machine; the existing Add/Apply/release stays the commit) · introduce explicit draft proposals with Commit/Discard | FR-3 |
| **OD-10** | Overlay layer | **(rec)** reuse `MapLayer::ProposalOverlay` with a heatmap sub-toggle · new `CutFillHeatmap` MapLayer (touches `layers/stack.rs` + config names) | NFR-6 |
| **OD-11** | Palette proposals as nodes | The user said proposals are "saved as nodes". Today only sculpt **regions** create nodes; palette `TerrainProposalAdd` does not. **(rec)** out of scope here (P2 reads both kinds regardless) · make palette proposals nodes in this track | scope |
| **OD-12** | Ship P3 in this track? | **(rec)** P3 only if P1+P2 land green with budget; else spin out a track · commit P3 now | P3 |

## 7. Dependencies

- **depends_on:** `sculpt_earthwork_regions_20260725` (regions, bake, volumes; status implemented_pending_inapp_verify, code present). `terrain_editor_overhaul_20260718` (archived = satisfied).
- **related:** `map_scale_authority_20260716` (units), `hierarchy_visibility_groups_20260808` (overlay visibility), `ui_semantics_unification_20260808` (Options surface for toggles), `endpoint_api_surface_20260725` (future egress for OD-7).
- **blocks:** none.

## 8. Out of scope

- Writing true terrain / destructive edits (NFR-1).
- Survey-grade volumes (TIN/prismoidal), and per-layer strata/material volumes (sculpt Q-3/Q-4 deferrals stand).
- Mass-haul diagram / balance-line optimization (natural P3+ follow-up).
- Utility network modelling itself (P4 precondition), clash detection beyond a flag.
- REST/MCP selection-report endpoint (OD-7 rec).

## 9. Deferred (explicit)

- **Mass-haul** needs a haul alignment plus station-ordered cut/fill. Revisit when cross-sections (P3) land.
- **P4 utilities** need a utility node property contract; open a dedicated track when the first real utility import exists.
- **Egress endpoint** for selection reports (OD-7): `endpoint_api_surface` follow-up.
