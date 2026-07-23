---
type: Track Spec
title: Pen Curve Tool — Illustrator-Style Bezier Anchors + Corner Settings
description: Turn the Pen tool from a click-to-append straight-polyline placer into an Illustrator-style cubic-bezier curve tool. Each anchor gains in/out bezier handles + a Corner/Smooth/Symmetric classification; click places a sharp corner, press-drag pulls out symmetric handles (smooth), Alt-drag breaks symmetry; a per-anchor "corner settings" smoothness slider (0 = sharp .. 1 = round) auto-derives collinear handles. Reuses the existing de Casteljau tessellation; all geometry stays in raw petal-local meters; legacy straight polylines render byte-identically.
tags: [feature, editor, pen, bezier, curves, corner-settings, pen_curve_tool_20260722, pending]
timestamp: 2026-07-22T00:00:00Z
resource: ./metadata.json
---

# Specification: Pen Curve Tool — Illustrator-Style Bezier + Corner Settings

**Track ID:** `pen_curve_tool_20260722`
**Priority:** P1 UX (user-directed 2026-07-22)
**Crates:** `fe-ui` (anchor authoring model, Pen gesture, dispatch handle variants,
corner-settings UI, ops/actions), `fe-terrain` (anchor-aware flattener + pick shape;
render-side twin of the anchor struct). No new crate dependency — see NFR-4.

Cross-links: [`../../product.md`](../../product.md),
[`../../tech-stack.md`](../../tech-stack.md),
[`../../code_styleguides/ui_ux.md`](../../code_styleguides/ui_ux.md),
[`../terrain_editor_overhaul_20260718/spec.md`](../terrain_editor_overhaul_20260718/spec.md)
(this track extends its `HitTarget`/`Operation` dispatch table + `SelectionKind`
read-model), [`../tool_inspector_ux_20260719/spec.md`](../tool_inspector_ux_20260719/spec.md)
(the read-only Pen affordance lives in its inspector; the editable default in
`tool_panel.rs`).

## Overview

Verbatim user directive (2026-07-22, given as the resolution to the Pen arm-grab
code-review finding):

> "it should show curves like in illustrator and allow the user to adjust the
> smoothness of a corner so basically corner settings"

This supersedes the arm-grab micro-finding: rather than carve a Pen exception into
the ratified "grab it wherever it's shown" gimbal rule, the **Pen tool becomes a
true vector-curve editor** — the whole interaction model changes, and the old
arm-grab conflict dissolves into it. It also delivers the long-standing
"cursor editor → PEN TOOL (curves/shapes)" direction.

**Scope-defining finding (2026-07-22 grounded exploration).** The
bezier→polyline→ribbon tessellation **already exists end-to-end**: a complete,
unit-tested curve module (`fe-ui/src/node_manager/curve.rs` — `push_cubic`
de Casteljau at `:157`, `catmull_rom` tension at `:47-77`, plus
`ellipse`/`circle`/`rectangle` shape emitters) is already wired into the ribbon
pipeline. So this track does **not** need new curve math. It needs (a) a per-anchor
data model that carries bezier handles, (b) UI to place and drag those handles
(Pen gesture + viewport handle markers + a corner-settings slider), and (c)
persistence of the handles through the existing `gpx_points` wire format. The
curve *rendering* is a small anchor-aware flatten step feeding the existing mesh.

### Ground truth (grounded exploration sweep)

- **Authoring buffer.** `PathEditorState` (`fe-ui/src/gis/mod.rs:129-207`) —
  `editing_track_id: Option<String>` (`:141`) is Authority B, `selected_point`
  drives per-vertex editing. The per-point row is `PathPointRow`
  (`fe-ui/src/gis/mod.rs:120-124`): `position: [f32;3]` (petal-local METERS) +
  `time_seconds: Option<f64>`. This is the primary insertion point for handles.
- **Render/persist working struct.** `TimestampedRoutePoint`
  (`fe-terrain/src/iot/animation.rs:47-50`) is the render/persist twin; it flows
  through `in_flight_points` (`gpx_bridge.rs:933`) and has a
  `#[cfg(feature="render")]` variant to mirror.
- **Wire format.** The `gpx_points` node property is a positional JSON array
  `[x,y,z,t]` per element, **no version field** (encoder `route_points_to_json`
  `gpx_bridge.rs:242-251`; decoders `json_to_route_points` `gpx_bridge.rs:256-275`
  and `decode_gpx_points` `fe-ui/src/gis/query.rs:171-190`; key const
  `GPX_POINTS_KEY` `gpx_bridge.rs:40`). Both decoders already read with
  `.get(i)…unwrap_or`/skip-short, so **appending** slots is backward-compatible
  for free.
- **Points are RAW petal-local METERS.** `Projection::wgs84_to_local` applies **no
  scale**; the ribbon entity Transform is scale-1 (`projection.rs:61-63`;
  `track_mesh` doc `fe-terrain/src/mesh/track.rs:40-48` states width is petal-local
  meters and "the ribbon is NOT world-scaled, so width must not be either"). Grep
  for `world_scale`/`effective_world_scale` across `fe-terrain/src/mesh` returns
  **zero** hits. Handles, samples, and width all share this meter frame — see NFR-1.
- **The Pen click seam.** `handle_path_point_interaction`
  (`fe-ui/src/node_manager/path_point_interaction.rs:215-403`) owns
  `ClickPriority::PathPlace`, the only path drag-state machine (`PathPointDrag`,
  `:70-74`), and raw `mouse_button` access. The router already computes the full
  `PointerPhase::{Press,Hold,Release}` lifecycle (`router.rs:136-144`) but only
  `is_fresh_press()` is consumed — `phase()` is `#[allow(dead_code)]`
  (`router.rs:113`), ready to wire. **Caution (critique):** the "empty-ground press"
  branch (`:360-402`) is the **LIVE append/auto-create path**, not an unused branch;
  click-vs-drag requires restructuring it from *press-time append* to
  *press-capture → hold-track → release-decide*.
- **New-track first anchor is deferred.** When `editing_track_id` is `None`, a Pen
  press does NOT append — it auto-creates a track and stashes only
  `first_point: [f32;3]` in `PendingPenCreate` (`gis::PendingPenCreate`), deferring
  the append until a `NodeCreated` echo returns the correlation id. Any first-anchor
  handles must be threaded through this deferred path — see FR-4 / NFR-3.
- **Dispatch has reserved headroom.** `HitTarget`/`Operation`
  (`fe-ui/src/node_manager/dispatch.rs:17-70`) are the documented growth point for
  "more operations on left click" (`:2-6`, reserved `PlaceNode` seam `:59`). Handle
  editing slots in here.
- **Existing tessellation to reuse.** `node_manager/curve.rs` de Casteljau
  (`push_cubic:157`) consumes a control **polygon**, and lives in `fe-ui` — which
  `fe-terrain` (where meshing/picking run) must not depend on. So the render side
  gets a small **anchor-aware** flattener duplicated in `fe-terrain` (NFR-4).
- **Corner-settings UI home.** `path_editor_card.rs::render_edit_view` is Authority
  B: it keys on `editing_track_id` (`:75`) + `selected_point` (`:378-384`), receives
  `&mut PathEditorState + &mut UiManager`, and never touches `NodeManager.selected`.
  It already uses a deferred-push idiom (`render_style_controls:559-576`). This is
  the split-safe home for the smoothness slider + corner-type toggle.
- **`tool_inspector.rs` is read-only BY CONSTRUCTION** (`tool_inspector_panel(ctx,
  tool: &ToolState, node_mgr: &NodeManager, path_state: &PathEditorState)`,
  `:131-136`; SETTINGS zone is a static `&[&str]` bullet list). It can host a
  **read-only** Pen affordance, but the **editable** Pen default must go in
  `tool_panel.rs` (owns `&mut ToolPanelState`, already renders editable
  `pen_mode`/`pen_samples_per_segment`) — see NFR-6 / FR-6.

### Assumed decisions (recommended defaults — confirm in Open Questions)

- **Handles are RELATIVE offset vectors** from `position`, in the same raw-meter
  frame — so `MovePoint` rides handles for free and smoothness scaling is a pure
  length multiply.
- **Smoothness = auto-derived collinear handle LENGTH** (reusing the `catmull_rom`
  tension math), NOT a true arc/fillet radius. A fillet radius is a later, separate
  knob.
- **Encoding = mixed 4-slot / 12-slot rows** in one array (zero migration), no
  version field, no object wrap.
- **The anchor-aware flattener is duplicated (~15 lines) in
  `fe-terrain/src/mesh/curve.rs`** to respect the `fe-ui ↛ fe-terrain` boundary.

## Functional Requirements

- **FR-1 — Per-anchor bezier model.** Extend `PathPointRow`
  (`fe-ui/src/gis/mod.rs:120-124`) and its render twin `TimestampedRoutePoint`
  (`fe-terrain/src/iot/animation.rs:47-50`) with `handle_in: Option<[f32;3]>`,
  `handle_out: Option<[f32;3]>` (relative meter offsets; `None` = no handle),
  `corner: CornerKind` (`{ Corner, Smooth, Symmetric }`, `#[default] Corner`), and
  `smoothness: f32` (0..1). `CornerKind` is defined in `fe-ui`; `fe-terrain` uses an
  identical local enum (no cross-crate import). *Priority: P1.* *Acceptance:* a
  cubic segment i→i+1 uses control points `[P_i, P_i+out_i, P_{i+1}+in_{i+1},
  P_{i+1}]`; both-handles-`None` = a straight line; the model round-trips through a
  unit test; every existing `PathPointRow` constructor/test is updated.

- **FR-2 — Backward-compatible wire format.** Extend the `gpx_points` encoding to
  `[x,y,z,t, inx,iny,inz, outx,outy,outz, corner_code, smoothness]`, emitting the
  **compact 4-slot** form for handle-less corner anchors and the **12-slot** form
  only when an anchor carries handles (mixed lengths in one array are fine). No
  version field, no object wrap, no `.hexon` archive bump (`gpx_points` is a DB node
  property, not an archive entry). *Priority: P1.* *Acceptance:* a legacy 4-slot row
  decodes to `Corner`/no-handle/`smoothness=0`; a 12-slot bezier row round-trips; a
  mixed-length array decodes correctly; the encoder never shortens an existing row
  nor emits an intermediate length (partial-row normalization — see Open Q); the
  round-trip test in `fe-database/tests/gpx_path_persistence_test.rs:169-200` is
  extended with a legacy-decode case and a bezier round-trip case.

- **FR-3 — Curve rendering + pick (reuse existing tessellation).** Add an
  anchor-aware `flatten_route(points, samples_per_seg) -> Vec<[f32;3]>` in a new
  `fe-terrain/src/mesh/curve.rs` (small de Casteljau duplicate, per NFR-4): for each
  segment build the cubic and push `samples_per_seg` samples; both-handles-`None`
  emits the single straight endpoint (zero added points). Feed the flattened dense
  polyline to BOTH `render_gpx_tracks` (before centroid-recenter + `track_mesh`,
  `fe-terrain/src/terrain_plugin.rs:718-722`) and `TrackPickShape`/`track_pick_shape`
  (`path_segment_interaction.rs:26-33`, `gpx_bridge.rs:524-555`) so clicks hit the
  visible curve. `track_mesh` (`mesh/track.rs:48`) stays polyline-based and
  unchanged; width stays in the meter frame; the entity Transform stays scale-1.
  *Priority: P1.* *Acceptance:* an all-corner track flattens to the identical
  polyline it renders today (byte-identical mesh); a single symmetric-handle segment
  produces a visibly curved ribbon; `flatten_route` is unit-tested for straight
  passthrough, one cubic, and **no `world_scale` leakage**. (The RDP simplify gate,
  `SIMPLIFY_THRESHOLD=10_000`, is not a practical concern for authored curves —
  critique downgraded it; still, only subdivide handle-carrying segments.)

- **FR-4 — Pen click-vs-drag gesture.** Restructure the Pen press path from
  press-time append to a deferred **release-time decision**: wire `phase()`
  (`router.rs:113`) into `handle_path_point_interaction`, add a `PenHandleDrag`
  resource (mirroring `PathPointDrag`), and on **Release** decide by drag distance
  vs a threshold (~0.15 m petal-meters): below ⇒ **corner** anchor (today's
  `append_point`, unchanged); above, no Alt ⇒ **symmetric smooth** anchor
  (`handle_out = drag_vec`, `handle_in = -drag_vec`); above + Alt ⇒ **corner** with
  an independent out-handle only. **Must also thread first-anchor handles through
  `PendingPenCreate` + the deferred `NodeCreated` echo** (critique must-fix) so a
  press-drag that *starts* a new track keeps its handles. *Priority: P1.*
  *Acceptance:* pure unit tests on the threshold/symmetry/Alt decision (mirroring
  `path_gimbal_drag.rs:277-351`); a click still yields a sharp corner; a press-drag
  yields a symmetric smooth anchor **including as the first anchor of a new track**;
  the existing Select-yield and `PathPlace`-vs-`NodePick` claim ordering still hold.

- **FR-5 — Viewport handle editing.** Add `HitTarget::PathHandle { idx, side }` +
  `Operation::MoveHandle { idx, side, position }` to `dispatch.rs` (`:17-70`),
  resolved in `resolve_operation`/`resolve_gimbal` (`:134-176`). Spawn handle-marker
  entities analogous to `PathPointMarker` (`path_point_interaction.rs:19`), picked
  via the same along-ray + `PICK_RADIUS=0.7` test, and **claiming ahead of** the
  vertex-body-yield and gimbal (respecting `MARKER_BODY_RADIUS`,
  `path_gimbal_drag.rs:33,218-229`; priority handle-marker > vertex-marker >
  gimbal-arm). Dragging one handle of a Smooth/Symmetric anchor moves the other
  collinearly (Symmetric keeps equal length; Smooth frees length); Alt breaks to
  Corner. *Priority: P1.* *Acceptance:* the "grab wherever shown" rule for lone
  vertices/segments is **untouched**; collinear enforcement + Alt-break transitions
  are unit-tested; a handle drag commits via `SetAnchorHandles`/`SetAnchorCorner`.

- **FR-6 — Corner settings UI (the user's headline ask).** In
  `path_editor_card.rs::render_edit_view`, add a "Selected vertex" sub-card gated on
  `selected_point.is_some()` (below `:378-384`), Authority B and split-safe by
  construction: (1) a Corner / Smooth / Symmetric segmented toggle
  (`UiAction::PathSetAnchorCorner`), and (2) a **smoothness slider 0.0..=1.0** — the
  primary "corner settings" knob — that AUTO-DERIVES collinear symmetric handles
  from the neighbor tangent (`normalize(P_{i+1} − P_{i-1})`, endpoints duplicate
  their neighbor per `curve.rs:66-69`; handle length `= smoothness · k ·
  min(gap_prev, gap_next)`, `k≈1/3`). smoothness 0 ⇒ zero-length handles ⇒ sharp;
  1 ⇒ round. Use the `render_style_controls` deferred-push idiom (`:559-576`):
  live-edit the local buffer for instant viewport echo, persist on drag-release.
  A **read-only** per-anchor affordance goes in `tool_inspector.rs`; the **editable**
  Pen tool-level default (`pen_new_anchor_kind`) goes in `tool_panel.rs`
  (NFR-6 must-fix). *Priority: P1.* *Acceptance:* the slider + toggle mutate
  `PathEditorState` + queue a `UiAction` and **never** read/write
  `NodeManager.selected`; smoothness 0/1 produce sharp/round anchors; every number
  carries its unit (`m`, `ui_ux.md §2`); `tool_inspector`'s signature is unchanged.

- **FR-7 — Ops / actions / bridge persistence.** Add `PathOp` variants
  (`fe-ui/src/path_ops.rs:11-55`) `AppendSmoothPoint`, `SetAnchorHandles`,
  `SetAnchorCorner`, with matching `UiAction` variants + handlers beside
  `PathAppendPoint` (`actions/mod.rs:593`) and `append_point` (`actions/path.rs:94`).
  `AppendPoint`/`MovePoint` stay unchanged (`MovePoint` already moves relative
  handles for free). Extend bridge `advance_path_edits` (`gpx_bridge.rs:1246`) +
  `persist_and_render_points` (`gpx_bridge.rs:1700`) to read-modify-write the new
  slots. *Priority: P1.* *Acceptance:* a bridge round-trip (append smooth, set
  handles, reload) preserves handles; `MovePoint` on an anchor moves its handles
  with it (no explicit handle rewrite).

## Non-Functional Requirements

- **NFR-1 — Raw-meter frame; no `world_scale`.** Handles, samples, and ribbon width
  all stay in petal-local meters; nothing multiplies geometry or width by
  `world_scale`/`effective_world_scale` (the terrain surface scales; the ribbon
  deliberately does not — `projection.rs:61-63`, `mesh/track.rs:40-48`). This is the
  #1 regression guard (a prior session's width×world_scale attempt collapsed GPX
  ribbons to hairlines).
- **NFR-2 — Two-authority selection split preserved (SACRED, `ui_ux.md §5`).** All
  corner-settings mutation targets `PathEditorState` (Authority B) + queued
  `UiAction`s; nothing reads or writes `NodeManager.selected`. `SelectionKind` stays
  a read-only facade.
- **NFR-3 — Backward compat byte-identical.** Legacy 4-slot polylines decode to
  all-corner/no-handle and flatten/mesh/RDP/centroid identically to today; every new
  op/`UiAction`/field/slot is additive; the first-anchor deferred path keeps handles
  (no silent sharp-corner regression). No migration pass.
- **NFR-4 — No `fe-ui → fe-terrain` dependency.** The anchor-aware flattener is
  duplicated (~15 lines) in `fe-terrain/src/mesh/curve.rs`; the fe-terrain anchor
  struct uses a local corner enum. Precedent: the existing local mirror enums in
  `tool_panel.rs`.
- **NFR-5 — Pure, testable helpers.** `flatten_route`, the smoothness→handle
  derivation, the click-vs-drag threshold decision, the collinear/Alt-break handle
  math, and the decode/encode slot mapping are all pure and unit-tested; egui paint
  stays thin (validated in-app).
- **NFR-6 — `tool_inspector.rs` stays read-only.** The editable Pen default lives in
  `tool_panel.rs` (which owns `&mut ToolPanelState`); `tool_inspector_panel`'s
  signature does not change (correcting the design's original false "no `&mut`
  change" claim).

## Out of scope

- **New curve math** — the de Casteljau flattener + `catmull_rom` + shape emitters
  already exist in `node_manager/curve.rs`; this track reuses them.
- **Road placement input modes** (straight / curve / freeform) —
  `road_builder_ux_20260716`.
- **Scale-authority plumbing** — `map_scale_authority_20260716`; this track consumes
  the existing meter frame, it does not unify the `world_scale` accessor.
- **`.hexon` archive / `fe-format` version bump** — the editable path lives only as a
  DB node property (`gpx_points`), not the bundled archive.
- **A true arc/fillet corner radius** — the smoothness knob scales handle length;
  a real fillet (inserts arc geometry) is a deferred follow-up (Open Q).
- **Deprecating the existing post-hoc `smooth_current`/PenMode bake**
  (`actions/path.rs:295`) — it can remain as a bulk "smooth entire track" action;
  its fate is an Open Q, not a task here.

## Open questions (ratify before build)

1. **Handle storage** — RELATIVE offset from anchor (assumed; `MovePoint` free,
   easy smoothness scaling) vs ABSOLUTE petal-meter positions (matches Illustrator's
   mental model).
2. **Smoothness semantics** — scale auto-derived collinear handle LENGTH (assumed;
   reuses `catmull_rom` math, no arc geometry) vs a true corner-radius/fillet that
   inserts an arc. Ship handle-length first?
3. **Flattener location** — duplicate ~15 lines in `fe-terrain/src/mesh/curve.rs`
   (assumed) vs extract a shared no-dep geometry crate vs flatten in the
   `fractalengine` bridge before spawn.
4. **Encoding** — mixed 4/12-slot rows (assumed, zero-migration) vs always-12-slot
   (uniform, but bloats every legacy track on next write).
5. **Smoothness slider readback after manual handle drag** — a hand-dragged
   asymmetric/rotated handle has no single scalar smoothness. Define the slider as
   write-derives / read-approximate, or grey it out once handles are manually
   asymmetric (else the slider "lies"). *Lean:* slider overwrites manual handles;
   document that.
6. **Partial-row normalization** — decode-side guard: if `corner_code` is
   Smooth/Symmetric but a handle is `None`, treat as Corner (prevents silent state
   drift from an intermediate-length row).
7. **Handle-marker vs gimbal/vertex-body priority** — confirm the exact
   `ClickPriority` ordering and pick radius for overlapping handles vs the
   `MARKER_BODY_RADIUS=0.7` vertex-body yield.
8. **Alt-drag-while-placing semantics** — does releasing an Alt-drag set only the
   out-handle (in=`None`, as designed), or also retro-set the *previous* anchor's
   out-handle (full Illustrator behavior)?
9. **Fate of the existing `smooth_current`/PenMode bake** — keep as a bulk
   "smooth entire track" action, or deprecate now that per-anchor live handles
   supersede it?
10. **Subdivision** — fixed `samples_per_seg` (assumed, deterministic; reuse
    `ToolPanelState.pen_samples_per_segment`) vs adaptive (flatness-based). Fixed
    first?
