---
type: QA Findings
title: UX QA Review — findings log
tags: [qa, ux, findings, ux_qa_review_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./metadata.json
---

# UX QA Review — Findings

Log issues found during the review here. One entry per finding. When the review
is done, triage these into the follow-on `ux_polish_*` implementation track.

**Format per finding:**

```
### [SEVERITY] Surface — one-line title
- **Surface:** (pen / icons / styling / stamp / paths-gis / analytics)
- **Repro:** how to reproduce
- **Expected:** what should happen
- **Actual:** what happens
- **Suggested fix:** (optional)
- **Severity:** blocker | major | minor | polish
```

Severity guide: **blocker** = can't complete the task · **major** = works but
painful/confusing · **minor** = small friction · **polish** = cosmetic/nice-to-have.

---

## Findings

### 2026-07-16 — user live-testing batch #1 (fixed same-day, tracks noted)

All eight findings below came from the user's hands-on session on 2026-07-16 and
were triaged straight into four implementation tracks (fixed that session,
in-app verification still user-gated):

### [MAJOR] stamp — GPX stamping does not persist across scene changes
- **Track:** `gpx_stamp_persistence_20260716` — petal-wide `PathAssetCache` +
  `materialize_path_assets`, per-track applied gate cleared on petal change.

### [MAJOR] pen — individual path vertices not selectable/moveable (want Illustrator pen tool)
- **Track:** `path_interaction_20260716` FR-2 — vertex select + highlight +
  drag now works in Select and Pen tools while a track is open for editing.

### [MAJOR] paths-gis — ribbon "too selectable": km-scale AABB swallows clicks near the path
- **Track:** `path_interaction_20260716` FR-1 — ray-vs-polyline narrow phase
  (`TrackPickShape`) replaces the AABB hit for track lines.

### [MAJOR] paths-gis — no gimbal on a selected path; want move/rotate/scale like a grouped asset
- **Track:** `path_interaction_20260716` FR-4 — centroid-anchored ribbon +
  gimbal commit bakes the delta into all `gpx_points` (timestamps kept).

### [MINOR] styling — default path ribbon far too wide (2.0 wu cyan)
- **Track:** `path_interaction_20260716` FR-5 — default width 0.5, slider down
  to 0.1. NOTE: per-track color/thickness/visibility controls already existed
  in the Paths-tab edit view (track_styling_20260713) — discoverability issue.

### [MAJOR] paths-gis — segments need selection + real-metric length on select; stamp spacing in meters
- **Tracks:** `path_interaction_20260716` FR-3 (segment select + m/km readout +
  total length) and `gpx_stamp_persistence_20260716` FR-3 (spacing in meters
  via `world_scale`).

### [MAJOR] inspector — side panel blows out to huge width on long property values; want copyable non-editable field
- **Track:** `inspector_units_width_20260716` FR-1 — width-capped copyable
  value boxes (elided display, full-value copy button), panel stays 260px.

### [MAJOR] inspector — asset size/position/rotation inputs should use real measurements
- **Track:** `inspector_units_width_20260716` FR-2 — Position (m), Rotation (°),
  Size (m) from node AABB with scale back-computation.

### [MAJOR] camera — clips after asset placement; zooms to default area then jumps; clips zooming on duck glb
- **Track:** `camera_focus_clip_20260716` — NodeCreated echoes position, focus
  resolves live entity transform, near 0.01 + min_distance 0.05.

---

## 2026-07-17 — audit-sourced findings (not yet fixed)

Sourced from the 2026-07-17 board-hygiene audit, not user testing. All await
the user-owned UX track — logged here so they survive until it is scoped.

### [MAJOR] paths-gis — no undo system anywhere
- **Actual:** no undo/redo exists in any editing surface; a one-shot Paths
  undo has been proposed (gimbal commit on an N-point track emits N MovePoint
  ops in one shot — cheapest first target).
- **Status:** audit-sourced 2026-07-17, awaiting the user-owned UX track.

### [MINOR] input — modifier-convention unification residual
- **Actual:** modifier-key conventions (Shift/Ctrl/Alt semantics) still vary
  across tools; unification residual from the input_router work.
- **Status:** audit-sourced 2026-07-17, awaiting the user-owned UX track.

### [POLISH] onboarding — verse/fractal/petal jargon strategy for first-run UI
- **Actual:** first-run UI exposes verse/fractal/petal hierarchy jargon with
  no plain-language framing.
- **Status:** naming RESOLVED 2026-07-17 — D-72 ratified: vocabulary stays
  ("for sure"). Residual scope for the UX track is plain-language *framing*
  (explainers/tooltips) only; renaming is off the table.

### [MAJOR] paths-gis — scale-authority split pending map_scale_authority
- **Actual:** two disagreeing scale authorities remain (terrain
  `effective_world_scale()` vs fe-ui `PetalMapState.world_scale`); UI numbers
  can disagree with the ruler until `map_scale_authority_20260716` lands.
- **Status:** audit-sourced 2026-07-17, awaiting the user-owned UX track
  (fix owned by map_scale_authority_20260716).

---

## 2026-07-24 — user live-testing batch #2 (in-app)

Findings from the user's live in-app test on 2026-07-24 (run on the pen_curve
Phases 3-6 build, CI green @ `f5d9673`). All three are owned by
`ui_shell_architecture_20260724`.

### [MAJOR] paths-gis — cannot select/manipulate existing path points from the viewport
- **Surface:** paths-gis / pen
- **Repro:** open a petal with an existing track; attempt to click-select or
  drag an existing path point in the viewport.
- **Expected:** viewport pick selects the point and allows manipulation
  (select/drag), entering the path edit flow.
- **Actual:** existing path points cannot be selected or manipulated from the
  viewport at all. Suspected selection-routing gap between viewport picks and
  the Authority B (`PathEditorState`) edit mode — app-wide, not pen-specific
  (this was also terrain_editor_overhaul's failed residual acceptance).
- **Status:** transferred to `ui_shell_architecture_20260724`.
- **Severity:** major

### [BLOCKER] terrain — terrain tools crash the app (gardener_ui_system panic, exit 101)
- **Surface:** terrain tools
- **Repro:** use the terrain tools in-app.
- **Expected:** terrain tool UI runs without panicking.
- **Actual:** panic in `fe_ui::plugin::gardener_ui_system`, followed by a
  bevy_egui `run_egui_context_pass_loop_system` panic ("pass output has not
  been prepared") and `Main::run_main` abort, exit code 101.
- **Status:** owned by `ui_shell_architecture_20260724`.
- **Severity:** blocker

### [MINOR] styling — tool-descriptions sidebar always open wastes real estate
- **Surface:** styling / tool inspector
- **Actual:** the always-open per-tool descriptions sidebar
  (`tool_inspector.rs` left SidePanel) consumes viewport real estate for
  static descriptive text.
- **Suggested fix:** replace with a tooltip model (user verdict 2026-07-24);
  `tool_inspector_ux_20260719` superseded accordingly.
- **Status:** owned by `ui_shell_architecture_20260724`.
- **Severity:** minor

---

## Candidate UX-track scope (fill after review)

Once findings are logged, summarize the themes here → this becomes the proposed
scope for the user-owned `ux_polish_*` track.

---

## Outstanding decisions feeding the UX track (2026-07-15)

UX/product-surface entries mirrored from the session-end decision register. Authoritative copy (full context, defaults, ratification checklist): [`../outstanding_decisions_20260715/spec.md`](../outstanding_decisions_20260715/spec.md). Resolve there; these lines are pointers, not a second log.

- D-28 (register): copy-for-BI card implemented in the GIS panel's Export tab (`gis_panel.rs` → `egress_card.rs`); API base URL is an editable field defaulting to localhost:8765 — ratify placement.
- D-29 (register): the hands-on GUI review + ux_polish FR-2 scoping are user-owned — schedule the pass.
- D-30 (register): may an agent pre-seed this findings.md with static-analysis candidates, or is the log purely user-authored?
- D-31 (register): where does the analytics-egress affordance live, and which spatial selections (petal / track / bbox) are reportable? Feeds analytics_egress Phase 4 / checklist §F.
- D-32 (register): inspector scope under repositioning — is FR-3 petal/fractal/verse inspection still wanted; cut first FR-4 delivery to a read/write role list?
- D-33 (register): inspector tabs folded to shipped {Properties, ApiAccess, Query} + new Access tab (wave default; active_tab persists across selections) — still open: multi-selection behavior, panel resizability.
- D-34 (register): confirm BIM FR-5 wall-binding is superseded by GPX rip-walls; amend spec.
- D-35 (register): is sign-first-then-promote petal-wide primitive materialization (async property fetch on cold cache) acceptable v1 behavior?
- D-36 (register): path-asset picker end state — shipped asset-node/blob:// picker vs hexon-ref semantics (spec OQ1, decide during/after FR-6); FR-1a still gated on lifting the ImportGltf quarantine.
- D-37 (register): sidebar-toggle button removal + pre-LogPlugin logging style — proceeding on plan defaults; veto now if wanted.
- D-38 (register): click-to-place surface semantics — Y=0 plane vs terrain-surface snap (interacts with node_placement_z_axis follow-up).
- D-64 (register): "Get shareable link" ships as a copyable curl command, not an in-app call — follow-up is a fe-ui→fe-api client seam wiring the button to POST /api/v1/query/share.
- D-65 (register): a node with both asset_path and a primitive descriptor renders the GLTF (asset wins) — flag if primitives should win.

(The single-point-node click-target concern is already on this track's checklist §B.)
