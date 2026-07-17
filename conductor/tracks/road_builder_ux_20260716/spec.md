---
type: Track Spec
title: Road Builder UX — Cities:Skylines-inspired drag-to-place path input layer
description: Straight/curved/freeform segment placement with chaining, angle snapping, guidelines, ghost preview, snap-to-existing-path, and live metric length readout — built on the existing gpx_points + path_asset stamping model; no procedural road meshes or intersection topology
tags: [feature, road_builder_ux_20260716, pending]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Road Builder UX (input layer)

**Track ID:** `road_builder_ux_20260716`
**Crates:** `fe-ui`, `fractalengine` (gpx_bridge only)
**Reference feel:** Cities:Skylines road placement (user-supplied screenshot, 2026-07-16)

## Overview

A road/path *builder input layer* for the viewport: click-and-drag placement of
path segments in three modes (straight two-click, curved three-click, freeform
drag), with C:S-style continuous chaining, 45°/90° angle snapping, alignment
guidelines off existing paths, endpoint/vertex snapping, a live ghost preview
with valid/invalid coloring, and a live metric length readout on the pending
segment.

Everything the builder places is an ordinary GPX track: points persist through
the existing `PathOp → DbCommand` seam (`CreateTrack`/`AppendPoint`/
`RemovePoint`/`MovePoint`), tracks keep `gpx_type: "track"`, and rendering
stays on the existing ribbon + `path_asset` stamping mechanisms. **This track
adds no procedural road meshes, no intersection/junction topology, and no new
persistence primitives** — those are explicitly deferred to a named follow-on
track (`procedural_roads`, not yet created; see Out of Scope).

## Background

What already exists (do not re-do):

- **Pen tool** (`Tool::Pen`, archived `pen_tool_curves_20260713` /
  `pen_autocreate_track_20260713`): click-to-place polyline points, curve
  resample (`node_manager/curve.rs`: `PenMode`, `resample`, `bezier`,
  `catmull_rom`, shape rings), auto-create-on-first-click via a fe-ui
  correlation id + deferred flush on the `NodeCreated` echo.
- **PathOp queue** (`fe-ui/src/path_ops.rs`, helpers in
  `fe-ui/src/actions/path.rs`): `CreateTrack{correlation_id}` / `AppendPoint` /
  `MovePoint` / `RemovePoint` / `AnnotatePoint` / `DeleteTrack`, drained by
  `fractalengine/src/gpx_bridge.rs` into `DbCommand`s. Curves/shapes already
  re-express as Remove/Append ops (`replace_points`, `append_shape`).
- **Input router** (`node_manager/router.rs`, archived
  `input_router_20260713`): per-frame left-click arbitration via
  `ClickPriority` (`Gimbal > PathMarker > PathPlace > PathSegment > NodePick`),
  egui/viewport gating in `ClickArbiter.available`.
- **path_interaction_20260716 (LANDED)**: ray-vs-polyline ribbon picking
  (`TrackPickShape` — per-track polylines resident in fe-ui, fed by the
  bridge), vertex select/move in Select tool, per-segment selection with
  real-metric length (`sync_path_measurements`, `format_distance_m`), centroid
  gimbal baked into `gpx_points`, 0.5-unit default width.
- **gpx_stamp_persistence_20260716 (LANDED)**: petal-wide stamp
  re-materialization from persisted `path_asset` descriptors; metric stamp
  spacing via `PetalMapState.world_scale`.
- **path_asset stamping** (`fe-sdk/src/path_asset.rs`,
  `verse_manager/path_asset_reconcile.rs` + `materialize_path_assets`):
  hexon-as-path-asset, FixedSpacing|FixedCount, tangent toggle (2026-07-13
  locked decisions). Roads *render* through this + the existing ribbon/wall
  mechanisms — unchanged by this track.

## Parallel-track coordination (2026-07-16 directive, binding)

- Runs in parallel with `mcp_scene_primitives_20260716` (fe-api / fe-runtime /
  fe-database) and `map_scale_authority_20260716` (scale accessor +
  placement). **This track stays in `fe-ui` + `fractalengine` (gpx_bridge)**
  and must not touch fe-runtime message types or fe-database (it needs no new
  `DbCommand`s — a deliberate design constraint).
- This track **consumes — does not define —** the `world_scale` accessor for
  metric readouts. Until `map_scale_authority` lands, use the existing
  `PetalMapState.world_scale` mirror (sanitized ≤0/non-finite → 1.0, as
  `node_manager` already does for segment measurements) behind a **single seam
  helper** so the accessor swap is a one-function change. Feature-detect;
  never hard-depend. Degrade to world-unit display when no sane metric scale
  is resolvable.

## Key design decisions

- **D-1 — Curved mode = three-click quadratic Bezier** (not a circular arc).
  Clicks: ① start (or chain anchor), ② control/apex point, ③ end. This is
  exactly C:S's curved-road gesture, and it maps 1:1 onto the *existing*
  `curve::bezier` machinery: its documented 3-remainder case promotes a
  quadratic `[P0, P1, P2]` to a cubic and samples it — zero new curve
  primitives, already unit-tested. A three-click arc would need new
  circle-fitting math and cannot reuse `curve.rs`. Tangent-continuous chained
  curves fall out of the angle-snap machinery (snapping the control direction
  to 0° relative to the previous segment's end tangent).
- **D-2 — Segments become plain `gpx_points`.** Straight = 2 control points;
  curved = the sampled Bezier polyline; freeform = decimated drag samples. All
  committed via existing ops (`AppendPoint`, batched reuse of the
  `UiAction::PathAppendShape { points }` carrier for multi-point commits). No
  curve parameters are persisted — the sampled polyline is the artifact, same
  contract as the pen tool's smooth/shape features.
- **D-3 — Snapped endpoints share coordinates, not identity.** Snap-to-endpoint
  copies the target `[f32; 3]` verbatim into the new point. There is **no
  topology graph, no shared-node merging, no junction record** — two tracks
  that "connect" merely have coordinate-equal endpoints. This limitation is
  deliberate and documented as the seed input for the `procedural_roads`
  follow-on (which will need to *infer or record* junctions).
- **D-4 — Provenance property `path_kind`.** Designed paths are distinguished
  from recorded GPS traces via a flat node property `path_kind` with values
  `"designed"` | `"recorded"` (flat key, sibling of `gpx_type` — it is data
  provenance, not style). Authored creations (road builder, pen, manual "New
  Path") write `"designed"`; GPX file import writes `"recorded"`; **absence ⇒
  `"recorded"`** so legacy tracks and out-of-scope import paths (fe-api HTTP
  import) are correct with zero migration. `gpx_type: "track"` is kept
  unchanged for renderer compat.
- **D-5 — New tool, existing router.** A new `Tool::Road` toolbar tool (hotkey
  `B`), a new viewport system (`road_builder_interaction.rs`) claiming a new
  `ClickPriority::RoadPlace` variant ranked alongside `PathPlace` (mutually
  exclusive by tool gating; a distinct variant keeps ownership attribution
  clear per `node_manager/AGENTS.md` §input-router "adding a consumer").
- **D-6 — Ghost + guidelines render via Bevy `Gizmos`** (immediate mode): no
  entity lifecycle, no mesh building (avoids the `VertexAttributeValues`-
  private gotcha entirely), colored valid/invalid per frame. The metric
  readout renders as a cursor-tethered egui overlay + mirrored in the Tools
  panel Road section.
- **D-7 — Snap candidates come from `TrackPickShape`** (the per-track polyline
  resource path_interaction already keeps resident in fe-ui) — no fe-terrain
  dependency, no DB round-trip per frame.
- **D-8 — Placement plane.** Points place on the same Y=0 ground plane the pen
  tool uses (known "y=0 placement" follow-up from ultrapilot notes applies
  equally here; terrain conformance is out of scope).

## Functional Requirements

### FR-1 — Placement modes (Priority: Must)

Three placement modes, selectable via Tools-panel Road section and shortcuts:

- **Straight (two-click):** click ① sets the start (or reuses the chain
  anchor), click ② commits a 2-point segment.
- **Curved (three-click quadratic Bezier, per D-1):** clicks ① start /
  ② control / ③ end; on click ③ the segment commits as the polyline sampled by
  `curve::bezier([start, control, end], samples_per_segment)` (default 12,
  panel-tunable). Intermediate state shows the ghost curve live.
- **Freeform (drag):** press-drag-release; cursor samples are decimated to a
  minimum arc spacing (default 1.0 m via world_scale, fallback 1.0 world
  unit) and committed as a polyline on release.

**Acceptance criteria**
- AC-1.1 (scripted): straight mode, two clicks on an empty petal → op queue
  contains `CreateTrack{correlation_id: Some(road-…)}` then exactly 2
  `AppendPoint`s at the click positions (bridge round-trip mocked via the
  deferred-flush seam, same as the pen auto-create tests).
- AC-1.2 (scripted): curved mode with `samples_per_segment = 8` → the commit
  expands through `curve::bezier` to `8 + 1` points, minus the shared anchor
  when chaining (i.e. 8 appended points on a chained segment); first and last
  points equal start/end.
- AC-1.3 (scripted): freeform decimation never emits two consecutive points
  closer than the min spacing, and always keeps first + last samples.
- AC-1.4 (scripted): commits reuse existing ops only — the queue contains no
  op variant other than `CreateTrack`/`AppendPoint`/`RemovePoint`.

### FR-2 — Continuous chaining + escape / undo-last-segment (Priority: Must)

- After a segment commits, the chain anchor becomes that segment's end point;
  the next segment starts there C:S-style **without appending a duplicate
  start point**.
- `Esc` cancels the pending (uncommitted) segment first; a second `Esc` ends
  the chain (clears the anchor); with no road-builder state it falls through
  to the existing deselect behavior.
- `Backspace` undoes the **last committed segment** of the current chain: the
  builder keeps a per-chain log of appended point counts and emits the
  matching `RemovePoint` ops (high→low index order, mirroring
  `replace_points`), restoring the previous anchor.

**Acceptance criteria**
- AC-2.1 (scripted): after committing segment A (2 points), the next straight
  commit appends exactly 1 new point and its position chains from A's end.
- AC-2.2 (scripted): undo-last-segment after a chained curved commit removes
  exactly that segment's appended points (high→low `RemovePoint` indices) and
  the restored anchor equals the prior segment's end.
- AC-2.3 (scripted): Esc state transitions — pending-segment → cleared;
  cleared → chain ended (pure state-machine test).

### FR-3 — Angle snapping (Priority: Must)

- Snap the pending endpoint's direction to multiples of a configurable
  increment (45° default, 90° selectable) **relative to the previous
  segment's end tangent** when chaining, or to the world X/Z axes for the
  first segment of a chain.
- Toggleable (panel toggle + shortcut); holding `Alt` temporarily suspends all
  snapping (Priority: Should).
- Snapping preserves the cursor's distance from the anchor (quantizes
  direction only).
- The ghost preview shows a snap indicator when a snap is active (angle value
  + highlighted guide ray).
- In curved mode the same machinery applies to the ①→② (control) direction —
  which yields tangent-continuous chained curves at the 0°-relative snap.

**Acceptance criteria**
- AC-3.1 (unit): a second click at 43° relative to the previous segment with
  45° snapping quantizes to exactly 45°; at 91° with 90° increments → 90°;
  distance from anchor preserved to within f32 epsilon.
- AC-3.2 (unit): first-segment snapping quantizes against world axes.
- AC-3.3 (unit): snap disabled → position passes through unchanged.

### FR-4 — Guidelines (alignment guides) (Priority: Must)

- While placing, generate guide lines from existing path endpoints/vertices
  within a snap radius (default 3 m via world_scale; panel-tunable):
  **extension lines** (along the segment direction through its endpoint) and
  **perpendiculars** at endpoints.
- The pending point snaps onto a guide when the cursor is within the snap
  radius of the guide line; guide-line **intersections** snap with higher
  priority than a single guide.
- Active guides render as gizmo lines; inactive candidates are not drawn.
- Candidate source: `TrackPickShape` polylines (D-7), including the currently
  edited track's already-committed segments.

**Acceptance criteria**
- AC-4.1 (unit): a cursor within radius of an endpoint's extension line
  projects onto that line exactly (point-on-line within epsilon).
- AC-4.2 (unit): cursor near the intersection of two guides snaps to the
  intersection point, outranking either single guide.
- AC-4.3 (unit): candidates beyond the snap radius produce no snap.
- AC-4.4 (unit): documented snap resolution order is total and deterministic:
  vertex/endpoint > guide intersection > guide line > angle snap > raw cursor.

### FR-5 — Snap-to-existing-path (Priority: Must)

- The pending point snaps to existing tracks' **endpoints** (higher priority)
  and **interior vertices** (lower) within the snap radius, across all tracks
  in the active petal.
- A snapped commit copies the target coordinate **exactly** (bitwise-equal
  `[f32; 3]`, per D-3) so networks connect visually.
- **Documented limitation (feeds `procedural_roads`):** a snapped endpoint
  only shares coordinates. No topology graph, no node merging, no junction
  entity is created; moving one track later does NOT move the other. This
  must be stated in `node_manager/AGENTS.md` §road-builder and in the
  follow-on deferral list below.

**Acceptance criteria**
- AC-5.1 (scripted): committing a segment whose end snapped to another
  track's endpoint appends a point bitwise-equal to the target coordinate.
- AC-5.2 (unit): endpoint candidates outrank interior-vertex candidates at
  equal distance.

### FR-6 — Ghost preview with validity coloring (Priority: Must)

- The pending segment renders every frame as a gizmo polyline from the anchor
  to the (snapped) cursor: straight = 2-point line; curved = live-sampled
  Bezier; freeform = the decimated sample trail so far.
- **Valid** (committable) ghosts use the accent color; **invalid** ghosts
  render red. Invalid states: no active petal, zero-length segment (snapped
  end == anchor), non-finite input. (No topological validity — grades,
  collisions, and junction legality belong to `procedural_roads`.)
- Snap indicators render with the ghost: highlighted target vertex marker for
  endpoint snaps, guide lines for guideline snaps, angle tick + degree label
  for angle snaps.

**Acceptance criteria**
- AC-6.1 (unit): ghost validity classifier covers each invalid state.
- AC-6.2 (manual, user-gated): ghost/guideline/indicator visuals verified
  in-app per the plan's verification checklist.

### FR-7 — Live metric length readout (Priority: Must)

- While a segment is pending, its length displays live near the cursor
  (egui overlay) and in the Tools-panel Road section: straight = anchor→cursor
  distance; curved/freeform = **arc length of the sampled polyline**, not the
  chord.
- Formatted m/km via the existing `format_distance_m` twin; converted through
  the world_scale seam helper (see coordination section). If
  `map_scale_authority` has not landed, the existing sanitized
  `PetalMapState.world_scale` mirror is used; if no sane scale resolves, the
  readout degrades to world units with a "wu" suffix (feature-detect, never
  hard-depend).
- The chain's running total length is also shown in the panel (Should).

**Acceptance criteria**
- AC-7.1 (unit): pending-length helper returns chord length for straight and
  summed polyline arc length for curved/freeform samples.
- AC-7.2 (unit): scale seam — sane world_scale → meters; degenerate scale →
  world-unit fallback string.

### FR-8 — Toolbar + Tools-panel UX and shortcuts (Priority: Must)

- New `Tool::Road` in the top toolbar (`panels/toolbar.rs`) with hotkey `B`
  (`shortcuts.rs`, standard egui `wants_keyboard_input` gating).
- A compact C:S-style **Road section in the Tools panel**
  (`panels/tool_panel.rs`): mode buttons (Straight/Curved/Freeform), snap
  toggles (angle snap + 45°/90° increment, snap-to-paths, guidelines), snap
  radius, curve samples, freeform min spacing, pending/total length readout.
- While `Tool::Road` is active (and egui not capturing): `1`/`2`/`3` select
  Straight/Curved/Freeform; `A` toggles angle snap; `Esc`/`Backspace` per
  FR-2. Shortcut handling follows the existing `shortcuts.rs` conventions
  (input_router: shortcuts never fire while egui wants keyboard).
- Router integration per D-5: `ClickPriority::RoadPlace` claim; `NodePick`
  yields while placing; gimbal handler early-returns for `Tool::Road` exactly
  as it does for `Tool::Pen`.

**Acceptance criteria**
- AC-8.1 (unit): mode/toggle state transitions from shortcut + button inputs
  (pure state functions).
- AC-8.2 (scripted): with `Tool::Road` active a viewport click claims
  `RoadPlace` and node-pick does not select (arbiter-level test, mirroring
  `router.rs` tests).

### FR-9 — `path_kind` provenance property (Priority: Must)

Per D-4 (user saw and approved the concern, 2026-07-16):

- `gpx_bridge` writes `path_kind: "designed"` on every **authored**
  `CreateTrack` (road builder, pen auto-create, manual "New Path" button) and
  `path_kind: "recorded"` on every **GPX-import** track draft.
- Absence ⇒ `"recorded"`: existing tracks and other import paths (e.g.
  fe-api's HTTP GPX import, out of scope here) default correctly with no
  migration and no backfill.
- `gpx_type: "track"` unchanged (renderer compat).
- **Analytics implication documented** (in `fractalengine/src/AGENTS.md` §gpx
  and the track retro): BI/fe-query consumers can filter designed
  infrastructure vs recorded traces, e.g.
  `WHERE properties.path_kind == 'designed'` (and
  `!= 'designed'`/absent for recorded) — this is the property the roadmap's
  analytics-egress queries will key on for as-designed vs as-traveled
  comparisons.

**Acceptance criteria**
- AC-9.1 (unit): the authored-create property set includes
  `("path_kind", "designed")`.
- AC-9.2 (unit): the GPX-import track draft's property list includes
  `("path_kind", "recorded")`.
- AC-9.3 (scripted): straight-mode two-click flow end-to-end at the queue +
  bridge-mapping level yields a 2-point track whose creation properties carry
  `path_kind: "designed"` (composition of AC-1.1 + AC-9.1).

### FR-10 — Persistence through the existing seam only (Priority: Must)

- All new points/segments persist via existing `PathOp` variants drained by
  the existing gpx_bridge arms. **No new `PathOp` variant, no new
  `DbCommand`, no schema change, no new node property except `path_kind`.**
- First-commit auto-create reuses the pen tool's correlation-id +
  deferred-flush seam (`road-track:N` ids, `NodeCreated` echo matching,
  `DbResult::Error` cleanup so the tool can't go dead), extended to stash a
  multi-point first segment instead of a single point.
- Track naming: auto-created tracks get a distinguishable default name
  (e.g. "Road N"); rename stays the Paths tab's job.

**Acceptance criteria**
- AC-10.1 (scripted): a full three-segment chained session produces a queue
  containing only existing op variants.
- AC-10.2 (scripted): a failed create (`DbResult::Error`) clears the pending
  road state (tool recovers, mirrors pen HIGH-1/HIGH-2 hardening).

## Non-Functional Requirements

- **NFR-1 — Purity/testability.** All snap math, guideline generation,
  decimation, segment expansion, length computation, chain bookkeeping, and
  state-machine transitions are pure functions/structs over `[f32; 3]` — no
  egui, no Bevy ECS types — mirroring `curve.rs`. Target >80% coverage on the
  new pure modules (workflow.md).
- **NFR-2 — Per-frame budget.** Snap candidate search is O(total resident
  track vertices) worst case with squared-distance early rejection against
  the snap radius; guideline candidates are generated only from
  within-radius endpoints (bounded fan-out). No per-frame allocation storms:
  reuse scratch buffers in the state resource. Ghost/guides are immediate-mode
  gizmos (no entity churn).
- **NFR-3 — Input safety.** All pointer handling flows through the
  `ClickArbiter` (egui/viewport gating respected); keyboard shortcuts gated on
  `wants_keyboard_input` like `shortcuts.rs`.
- **NFR-4 — Repo conventions.** Terse one-line doc comments; the "why" lives
  in `fe-ui/src/node_manager/AGENTS.md` (new §road-builder) and
  `fractalengine/src/AGENTS.md` §gpx (path_kind). No `unwrap()`/`expect()` in
  production paths; `clippy -D warnings` + `fmt` clean; single full workspace
  sweep at the end of the track (per 2026-07-16 directive), not per-task
  sweeps.
- **NFR-5 — Parallel-track hygiene.** No edits to fe-api, fe-runtime,
  fe-database, fe-terrain, or fe-sdk (avoids collision with
  `mcp_scene_primitives_20260716` and `map_scale_authority_20260716`).

## User Stories

- **US-1:** As a petal designer, I want to lay out a road network by clicking
  segment endpoints with angle snapping and guidelines, so my designed
  infrastructure is tidy without manual vertex nudging.
  - Given the Road tool in straight mode with 45° snap on, when I click a
    start and then click roughly 43° off the previous segment, then the ghost
    snaps to 45°, shows "45°" and the metric length, and the second click
    commits the snapped 2-point segment.
- **US-2:** As a petal designer, I want each new segment to continue from the
  last one and connect exactly to existing paths, so my network looks
  continuous.
  - Given a committed segment, when I move the cursor near another track's
    endpoint, then the ghost end snaps to that exact coordinate and the commit
    reuses it verbatim.
- **US-3:** As an analytics consumer, I want designed paths distinguishable
  from recorded GPS traces, so BI queries can compare as-designed
  infrastructure against as-traveled data.
  - Given a road-builder track and a GPX-imported track, when I query node
    properties, then the former carries `path_kind: "designed"` and the
    latter `"recorded"`.

## Technical Considerations

- **New files (fe-ui):** `node_manager/road_snap.rs` (pure snap/guideline/
  decimation/length/chain math + `RoadMode`/state-machine types),
  `node_manager/road_builder_interaction.rs` (viewport system: arbiter claim,
  state machine driving, commit via `UiAction`s), Tools-panel Road section in
  `panels/tool_panel.rs`, `Tool::Road` in `panels/toolbar.rs` + `shortcuts.rs`,
  `ClickPriority::RoadPlace` in `node_manager/router.rs`, deferred-flush
  extension in `verse_manager/db_results` (road pending-create buffer beside
  the pen's).
- **Touched (fractalengine):** `gpx_bridge.rs` — `PATH_KIND_KEY` constant,
  `"designed"` on the authored `CreateTrack` arm, `"recorded"` in the import
  draft mapping; `src/AGENTS.md` §gpx note.
- **Action reuse:** multi-point commits ride the existing
  `UiAction::PathAppendShape { points }` (already "append pre-generated
  points to the edited track"); single points ride `PathAppendPoint`; undo
  rides `PathRemovePoint`. Expect **zero new `UiAction` variants** unless the
  deferred first-commit stash forces a carrier (planner may add one fe-ui-
  internal action if needed — still no new PathOp).
- **Known cost:** undoing a curved/freeform segment emits one `RemovePoint`
  per sampled point (bounded by samples-per-segment / decimation; same
  one-shot cost class as the landed gimbal bake).
- **Gotchas from memory:** ghost via Gizmos avoids `VertexAttributeValues`
  privacy; if any test-harness enum matching is touched, remember the
  exhaustive-match gotcha; RUST_MIN_STACK=64MB + -j4 for surrealdb-core
  recompiles during the end sweep.

## Out of Scope (explicit non-goals)

Deferred to the named follow-on track **`procedural_roads`** (not yet
created) — user-ratified HYBRID decision, 2026-07-16:

1. Continuous ribbon **road meshes** (procedural geometry along the path).
2. Auto-generated **intersections / junction topology** (including any shared
   node graph, node merging on snap, or junction entities — see D-3/FR-5).
3. **Road upgrade tool** (change a placed road's type/width in place).
4. **Zoning** (C:S-style adjacent-parcel zoning).

Also out of scope for this track (not necessarily in `procedural_roads`):

5. Terrain conformance / elevation grading, bridges, tunnels (Y=0 plane per
   D-8; "y=0 placement" follow-up applies).
6. fe-api HTTP GPX import `path_kind` parity (absence ⇒ recorded already
   yields correct semantics; noted for a future fe-api touch).
7. Ramer-Douglas-Peucker or curvature-aware freeform simplification
   (min-spacing decimation only; RDP is a candidate follow-up).
8. Persisting curve control parameters (sampled polyline is the artifact).
9. Grid snapping, parallel-road offset placement, and cost/budget UX.
10. Changes to stamping/ribbon rendering (consumed as-is).

## Open Questions

- OQ-1: Hotkey `B` for the Road tool — confirm it doesn't collide with
  planned bindings elsewhere (S/G/R/X/P taken; B free today).
- OQ-2: Snap radius semantics — spec'd world-space (metersized via
  world_scale). Screen-space radius feels better across zoom levels; revisit
  after in-app verification if the user finds far-zoom snapping too grabby.
- OQ-3: Default track name for auto-created roads ("Road N" vs "New Path" to
  match pen). Cosmetic; decide at implementation.
- OQ-4: Should the running chain total render at the cursor too, or panel
  only (FR-7 Should)? Decide during manual verification.
