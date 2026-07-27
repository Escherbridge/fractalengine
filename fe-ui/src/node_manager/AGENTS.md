# fe-ui/src/node_manager — single source of truth for selection

## §brush-tool-and-context-hardening

`brush_interaction.rs` claims before all scene interactions while Brush is
active, converts meter controls through the active petal's sanitized
`world_scale`, samples at half the converted radius, and emits one bounded
`SculptBrushStroke` on release. Sampling is distance-based.
Escape/right-click/tool/petal change cancels. Bare B activates Brush and opens
its contextual controls;
Ctrl/Cmd+B remains the sidebar toggle.

Right-click classification only fills `ContextTarget`; it does not mutate
`NodeManager` or select/promote stamps. Viewport pick stays in the main chain;
classification is separately constrained after it and before track opening.

- `mod.rs` — `NodeManager`/`NodeSelection`/`AxisDrag` types +
  `NodeManagerPlugin` (registers the `.chain()`-ed system order inside
  `UiSet::Selection`) + unit tests for the selection state machine.
- `shortcuts.rs` — keyboard tool-switch shortcuts + Escape-to-deselect.
- `sidebar_sync.rs` — resolves `NodeManager.pending_sidebar_select` (a
  node_id set by a sidebar click) into an ECS `Entity` + `select()` call.
- `gimbal_interaction.rs` — hover detection, axis pick → drag → commit, and
  the gizmo draw system. The largest submodule; keep pure geometry helpers
  private to this file, EXCEPT the axis-pick trio (`pick_axis`, `axis_vec`,
  `axis_screen_dir`) which is `pub(super)` so the FR-3 path gimbal drag reuses
  the exact same math (see §dispatch).
- `dispatch.rs` — FR-2 object-aware left-click operation table: the pure
  `resolve_operation(tool, kind, hit)` truth table + `Operation`/`HitTarget`
  enums (see §dispatch).
- `path_gimbal_drag.rs` — FR-3 drag: gimbal-drag a selected path vertex/segment
  (no entity) → `UiAction::PathMovePoint` on release (see §dispatch).
- `viewport_pick.rs` — 3-D viewport click → nearest-node select/deselect via a
  precise ray/AABB raycast (see §glb-mesh-picking).
- `path_point_interaction.rs` — path-point viewport editor (see §path-points).
- `path_segment_interaction.rs` — `TrackPickShape` (precise ribbon pick
  geometry), the shared ray-vs-segment math, ribbon-SEGMENT selection, and
  live metric measurement (see §track-picking + §path-segments).
- `billboard.rs` — `billboard_face_camera`, the per-frame face-camera system
  for `Billboard`-tagged icon quads (see §data-icons + `fe-ui/src/AGENTS.md`
  §data-icons).
- `router.rs` — per-frame left-click arbitration + the pure `hit_target_rank`
  claim-priority table (see §input-router + §pointer-manager).
- `pointer/mod.rs` — cross-authority pointer bridge (FR-3): `open_track_on_select`
  + petal-tracking, re-homed verbatim from `viewport_pick.rs`. Coordinates
  `NodeManager.selected` with `PathEditorState` WITHOUT merging them (see
  §pointer-manager + §track-picking).
- `selection.rs` — typed selection read-model (FR-1): a per-frame projection
  over `NodeManager.selected` + `PathEditorState.*` into `SelectionKind`; feeds
  the gimbal (FR-3) and future object-aware dispatch (FR-2). See
  §selection-read-model.
- `inspector_sync.rs` — `NodeManager` → `InspectorFormState` display sync
  (transform strings + per-node URL/property load on selection change; also
  clears the Annotation card's title/body/color buffers here since the
  underlying property load is async — see root `AGENTS.md` §gis-query-ui).
- `transform_broadcast.rs` — commits a finished gimbal drag to
  `DbCommand::UpdateNodeTransform` + P2P sync, and applies inbound API
  transforms back onto the ECS.

System functions are `pub(super)` (visible to `mod.rs`'s plugin
registration only) — this module's public surface is just `NodeManager`
itself; nothing outside `fe-ui` should call the per-frame systems directly.

## §pen-tool — pen (phase 1 polyline + phase 2 curves/shapes + bezier anchors)

`Tool::Pen` (`panels/toolbar.rs`, hotkey `P` via `shortcuts.rs`) is the
click-to-place tool for drawing a track's polyline. Phase 1 places straight-
segment control points; phase 2 (`curve.rs` + the Tools-panel Pen section)
resamples them into curves and generates shape rings;
`pen_curve_tool_20260722` (the bezier-anchors subsection below) makes the
Pen an Illustrator-style per-anchor cubic-bezier curve tool.

- **Gating, not a separate system.** Pen behavior is folded into the
  existing `handle_path_point_interaction` (no new system/registration).
  An empty press on terrain begins the pen gesture and claims the router's
  `PathPlace` priority — on the PRESS frame, since the arbiter resets its
  owner each frame and only the press is contested — but only when
  `tool.active_tool == Tool::Pen`; a marker pick claims `PathMarker` (Pen
  mode, Select-while-editing per FR-2, or while a drag is in flight) — the
  system's `!pen_active && !markers_editable && …no drag in flight…` guard
  returns before `pick_marker` runs in Move/Rotate/Scale.
  Because `PathPlace` outranks
  `NodePick`, a Pen-mode empty press wins the frame and node-pick yields;
  in Select mode path-point declines to claim `PathPlace`, so `NodePick`
  gets the click. This structurally reproduces the ab9c53c fix (node
  selection wins in Select mode) — see §input-router.
- **Two append actions.** A below-threshold Corner click still emits the
  legacy `UiAction::PathAppendPoint` unchanged; an anchor that carries
  bezier fields emits `PathAppendSmoothPoint` (FR-7 — bezier subsection
  below). The pen decides *which* at Release time, not press time.
- **Auto-create on the first click** (`pen_autocreate_track_20260713` +
  HIGH-1/HIGH-2 correlation-id hardening). A Pen empty-click with
  `PathEditorState.editing_track_id == None` no longer no-ops: it claims
  `PathPlace` (so `NodePick` still yields), then — because the new track's
  `node_id` doesn't exist until the DB round-trips — generates a fe-ui-side
  correlation id (`gis::next_pen_correlation_id`), stashes
  `PathEditorState.pending_pen_create` (that id + the gesture's FULL
  first-anchor payload: the Y=0 world position PLUS
  `handle_in`/`handle_out`/`corner`/`smoothness` — the FR-4 must-fix, so a
  press-drag that STARTS a track keeps its curve through the deferred echo)
  and queues `UiAction::PathCreateTrack { petal_id, "New Path", correlation_id:
  Some(id) }`. The auto-create decision itself now happens at Release (the
  gesture end), but the no-track gates are ALSO enforced at Press so a doomed
  gesture never starts. The petal is the active one, sourced from
  `NavigationManager.active_petal_id` exactly like `viewport_pick`; no active
  petal → keep the no-op (one-line log hint). A second press before the create
  resolves is suppressed by the `has_pending_pen_create()` guard. The deferred
  append lands in `verse_manager::db_results` on the track's
  `DbResult::NodeCreated`, matched by the **echoed correlation id**
  (`take_pending_pen_create_if(cid)`) — NOT the old `!has_asset && active-petal`
  content heuristic, which a concurrent GPX-import/dialog create could hijack. On
  a match it calls `start_editing(new_id)` and pushes the stashed anchor as the
  track's first append — `PathAppendSmoothPoint` when it carries handles, else
  the legacy `PathAppendPoint`. The full id threading + the `DbResult::Error`
  cleanup (a failed create clears the pending state so the pen can't go dead) are
  documented in `fe-ui/src/AGENTS.md` §path-editor. Subsequent clicks see
  `editing_track_id == Some` and append through the normal branch — no further
  special-casing.
- **UI entry point.** `panels/path_editor_card.rs`'s edit view no longer has
  an "Append from cursor" button (removed — it placed points anywhere the
  3-D cursor happened to be, independent of tool mode, which made accidental
  placement easy). It now shows a one-line hint pointing at the Pen tool.
- **Preview.** Placed points render via the existing `sync_path_point_markers`
  spawn/despawn (yellow `Sphere(0.35)`); the connecting polyline renders via
  `fe_terrain`'s `render_gpx_tracks` off the same `gpx_points`. No separate
  pen-preview mesh in phase 1.

### phase 2 — curves + shapes (`curve.rs`)

`curve.rs` is pure `[f32;3]` math (XZ plane, Y carried through), no Bevy
systems. `PenMode` (Polyline/CatmullRom/Bezier) + `resample()` turn placed
control points into a denser sampled polyline; `catmull_rom`'s `tension` is
the "sharp ↔ smooth" sensitivity (1.0 ≈ straight, 0.0 ≈ round). `ellipse`/
`circle`/`rectangle` generate closed point rings. The Tools-panel Pen section
(`panels/tool_panel.rs`) exposes a mode radio, tension slider, and shape
buttons; its buttons queue `UiAction::PathSmoothCurrent` / `PathAppendShape`
into `ToolPanelState.pending_actions`, drained in `process_ui_actions` (the
panel has no `ui_mgr` handle). Both actions re-express their result as the
existing `RemovePoint`/`AppendPoint` `PathOp`s, so no gpx-bridge change is
needed — smooth = resample-then-replace, shape = append the generated ring.
This post-hoc whole-track bake STAYS (ratified Q9) alongside the per-anchor
bezier model below; deprecation deferred.

`flatten_anchor_path` (`stamped_asset_nodes_20260725` T2): flattens
`BezierAnchor = (position, handle_in, handle_out)` slices into the dense
polyline the path-asset stamp materializer samples. It is a byte-exact MIRROR
of fe-terrain `mesh::curve::flatten_route` (`FLATTEN_SAMPLES_PER_SEGMENT` = 16
must equal fe-terrain's `SAMPLES_PER_SEGMENT`; both-handles-`None` segments
pass through, else cubic `[P, P+out, Q+in, Q]`) so stamps land on the same
curve the renderer draws — duplicated because fe-ui must not depend on
fe-terrain. The tuple signature (not `PathPointRow`) keeps `curve.rs` free of
a gis-module dependency.

### bezier anchors — Illustrator-style curves (`pen_curve_tool_20260722`, phases 1/3-6)

All decision logic is pure + unit-tested; geometry stays raw petal-local
meters (no `world_scale`, NFR-1); every mutation targets `PathEditorState` +
queued `UiAction`s (Authority B — `NodeManager.selected` is never touched,
NFR-2, `ui_ux.md §5`).

- **Anchor model (FR-1).** `PathPointRow` (`gis/mod.rs`) carries
  `handle_in`/`handle_out: Option<[f32;3]>` (RELATIVE meter offsets from
  `position`, so `MovePoint` rides them for free), `corner: CornerKind`
  (`Corner`/`Smooth`/`Symmetric` — a fe-ui-LOCAL enum, deliberately never
  shared with fe-terrain's twin, NFR-4; `to_code`/`from_code` maps it to the
  wire's `corner_code` float) and `smoothness: f32` (0..1). The ops seam is
  `PathOp::{AppendSmoothPoint, SetAnchorHandles, SetAnchorCorner}` (FR-7)
  with matching `UiAction`s + local-echo handlers in `actions/path.rs`;
  persistence is the mixed 4/12-slot `gpx_points` encoding
  (`fractalengine/src/AGENTS.md` §gpx); render/pick flattening is
  fe-terrain's `flatten_route` (`fe-terrain/src/mesh/AGENTS.md` §curve).
- **Press/Hold/Release gesture (FR-4).** The old press-time append became a
  release-time decision, keyed on `arbiter.phase()` (`router.rs` — its
  previously dead `phase()` is now consumed). Press claims `PathPlace` and
  captures the anchor into the `PenHandleDrag` resource (mirror of
  `PathPointDrag`); Hold updates `drag_vec` from the y=0 ray hit and
  observes Alt (`pen_observe_alt`: the FIRST mid-drag Alt freezes `−drag_vec`
  as `frozen_in`; Alt already down at press sets `alt_seen` with NO frozen
  value); Release runs the pure `pen_release_decision(drag_vec, alt_seen,
  frozen_in, default_kind, PEN_DRAG_THRESHOLD_M = 0.15 m)`: below threshold
  ⇒ `Corner` (legacy append) or `SmoothClick` per the tool-level default
  `ToolPanelState.pen_new_anchor_kind`; at/above without Alt ⇒
  `SymmetricDrag` (`handle_out = drag`, `handle_in = −drag`, kind
  Symmetric); at/above with Alt ⇒ the **ratified-Q8 combination anchor** —
  `handle_out` = final drag, `handle_in` = the frozen symmetric value
  (`None` when Alt was held from the press), kind Corner — preserving the
  just-drawn segment's curvature WITHOUT retro-mutating the previous anchor
  (full Illustrator behavior). `pen_anchor_fields` maps the decision to the
  row's bezier fields; `SmoothClick` derives collinear handles via
  `curve::derive_symmetric_handles` (direction `normalize(next − prev)` with
  missing-endpoint neighbor duplication, length `smoothness · ⅓ ·
  min-neighbor-gap`), falling back to the plain corner append on a first
  anchor with no tangent.
- **Gesture cancellation safety (post-review hardening).** Both drag
  machines capture their press-time context and validate it at Release
  instead of re-reading mutable state: `PenHandleDragState` carries
  `press_track_id`/`press_petal_id`, resolved by the pure
  `pen_gesture_fate` (track changed ⇒ Drop; auto-create fires only when it
  was intended at press AND the petal is unchanged — a staged-Escape
  `stop_editing`, track delete, or petal change mid-hold can therefore
  never auto-create a spurious track from a canceled anchor);
  `PathHandleDragState` carries `press_track_id` and commits only while it
  still equals `editing_track_id`. This is deliberately release-time
  validation rather than clearing the drag resources from
  `PathEditorState::stop_editing` — that method is a plain struct fn called
  from panel code (`path_editor_card` Back, `shortcuts` Escape,
  `actions`, `db_results`) with no access to the ECS drag resources, and
  press-context validation covers every cancellation path uniformly.
- **Stored smoothness = geometry truth.** Every commit that writes handles
  also stores the smoothness READBACK (`gis::smoothness_readback`,
  `|handle_out| / (⅓ · min-neighbor-gap)` clamped 0..1): drag-created pen
  anchors (`pen_anchor_fields`) and handle-drag releases
  (`commit_smoothness`), falling back to the prior value only when no
  neighbor gap exists (first anchor of a new track). This keeps the Q5
  slider from seeding 0.00 on a visibly-round anchor and from collapsing
  the curve on its first nudge; the card-side seed rule is
  `fe-ui/src/AGENTS.md` §path-editor.
- **Viewport handle editing (FR-5, `path_handle_interaction.rs`).** One cyan
  billboard quad (`PathHandleMarker { index, side }`, `dispatch::HandleSide::
  {In, Out}`) per `Some` handle side of the edited track, at `anchor +
  offset`; membership-keyed despawn-all rebuild + idle-only position sync
  (`sync_path_handle_markers` — a live drag owns the dragged anchor's marker
  transforms); Illustrator-style anchor→handle stems via `draw_handle_stems`
  (gizmo lines, draws only). Picking uses the SAME along-ray
  `PICK_RADIUS = 0.7` test as vertex markers (pure core
  `pick_nearest_handle`), and the system runs FIRST in the `.chain()`,
  claiming the top-rank `ClickPriority::PathHandle` — handle > vertex >
  gimbal by first-claim-wins construction (ratified Q7). The
  `MARKER_BODY_RADIUS = 0.7` vertex-body yield is untouched; radius equality
  is unit-tested so an overlapping handle + vertex resolves purely by chain
  order. Drag discipline is pure: `disciplined_opposite` (Symmetric =
  equal-and-opposite mirror; Smooth = collinear direction, opposite keeps
  its OWN length; Corner = independent; a missing opposite is never
  fabricated) plus a one-way Alt-break (`handle_observe_alt` freezes the
  opposite at its live disciplined value and demotes the gesture to Corner;
  Alt-at-press keeps the STORED opposite). Release commits ONE
  `PathSetAnchorHandles` (skipped when unmoved, mirroring the gimbal's no-op
  skip) plus `PathSetAnchorCorner(Corner)` on an Alt-break. Handles are a
  `HitTarget`, not a `SelectionKind` — the lone-vertex/segment "grab it
  wherever it's shown" gimbal arms are untouched (see §dispatch).
- **Read-only vs editable Pen affordance split (NFR-6).**
  `tool_inspector.rs` stays read-only BY CONSTRUCTION (its panel signature
  is unchanged): it renders the pure `anchor_readout` line ("Anchor #3:
  Smooth, smoothness 0.50") under the selection summary
  (`panels/AGENTS.md` §tool-inspector). Everything EDITABLE lives where
  `&mut` already flows: the per-anchor corner-settings card in
  `path_editor_card.rs` (FR-6/Q5 — `fe-ui/src/AGENTS.md` §path-editor) and
  the tool-level new-anchor default `pen_new_anchor_kind` in
  `tool_panel.rs` (`panels/AGENTS.md` §tool-panel).

## §path-points — viewport path-point editor

`path_point_interaction.rs` is the "click-in-the-viewport to place / drag /
annotate a track's points" editor. It runs only while `PathEditorState`
has an `editing_track_id`. Design notes:

- **Why fe-ui owns the markers.** fe-terrain is not a dependency of
  `node_manager`, so the editor spawns/despawns its own `PathPointMarker`
  spheres straight into the ECS from `PathEditorState.points` rather than
  reusing any terrain-side ribbon geometry. `sync_path_point_markers` keeps
  one marker per point; it despawns-all + respawns whenever the point count
  changes (counts are small, so a full rebuild is cheaper than tracking
  per-index moves), and lets the drag system write live positions straight
  into each marker `Transform` between count changes.
- **Who gets the click.** The editor, `viewport_pick`, and gimbal all want
  the same left-click. Ownership is arbitrated by `router.rs` (§input-router),
  not ad-hoc flags: `handle_path_point_interaction` claims `PathMarker` when it
  picks an existing marker and `PathPlace` on a Pen-mode empty click; because
  both outrank `NodePick`, `viewport_pick` yields the frame automatically.
  `handle_gimbal_interaction` early-returns for `Tool::Select | Tool::Pen`
  (only Move/Rotate/Scale run it) — otherwise a Pen-mode press near a projected
  axis would let `pick_axis` claim `Gimbal` (top priority) and start a no-op
  drag that swallows the PathPlace click. The Pen drag branch is already a
  no-op, so the early return costs nothing.
- **Ordering.** Registered in `mod.rs`'s `.chain()` after `resolve_pointer_frame`
  and the gimbal systems, BEFORE `viewport_pick::handle_viewport_click`, so a
  path-point claim lands before node-pick tries to claim in the same frame.
- **Interaction model.** Empty click on terrain (Y=0 plane) while `Tool::Pen`
  is active → queue `PathAppendPoint` (see §pen-tool). Plain click on a
  marker → **select** that vertex (`path_interaction_20260716` FR-2: sets
  `PathEditorState.selected_point`, clears `selected_segment`, highlighted
  orange by `sync_path_point_markers` + the Paths-tab list row) AND begin a
  drag on that marker's current y-plane; release commits a single
  `PathMovePoint` (no remove+append index churn). **FR-2 gate change:** marker
  pick/drag now works in `Tool::Select` too (not just Pen) whenever a track is
  open for editing — the guard is
  `pen_active || (editing && Select|Pen) || drag.active`. Markers only exist
  while editing, so this is safe; pen-APPEND stays Pen-only (guarded at the
  append branch), and Move/Rotate/Scale still yield to the gimbal (its handler
  runs first and claims). Holding **Ctrl** during the drag switches to vertical mode
  (FR-1a, `node_placement_z_axis_20260713`): vertical cursor motion raises/
  lowers the point's height (Bevy Y, the user's "z-axis") by
  `height_delta_from_cursor` at `HEIGHT_DRAG_SENSITIVITY` (0.01 world-units/px,
  same feel as gimbal rotate), decoupled from the ray-plane hit; x/z still
  track the ray, and the raised Y is preserved on release through the same
  `PathMovePoint`. Shift/Alt+click on a marker → open the inline
  annotation form (`PathEditorState::open_annotate_form`, rendered by
  `panels/path_editor_card.rs`). All hit tests are the same manual
  along-ray + radius test `viewport_pick` uses (no Bevy picking backend).
- **Bridge path.** `PathMovePoint` → `PathOp::MovePoint` →
  `fractalengine::gpx_bridge`, which repositions the point in place through
  the same FR-5 `in_flight_points` authoritative-buffer path as
  Append/Remove (preserves timestamps, no read-modify-write race). The
  editor never persists `gpx_points` itself.

Per-track line styling (color/line-style/visibility, "FR-10") is NOT part
of this port — it depends on `fe_terrain::terrain_plugin::GpxTrackStyle`,
which lives outside the current tree's scope.

## §data-icons — billboard face-camera system (`billboard.rs`)

`billboard_face_camera` (`data_icons_20260713`) keeps every entity carrying the
`fe_ui::plugin::Billboard` marker turned toward the viewport. It copies the
`OrbitCameraController` camera's world `rotation()` onto each billboard
`Transform.rotation` — a flat `Rectangle` icon quad (local XY plane, +Z normal)
kept parallel to the camera image plane reads as a 2-D icon instead of a solid.
Cheap by construction: one camera lookup + a rotation write over the
billboard-only set. Registered as a **standalone** `Update` system (not in the
selection `.chain()`) because facing is orientation-only and order-independent
of position edits. Applied to path-point markers and the single-point track
node — the latter keeps its `Mesh3d`/`Aabb` so §glb-mesh-picking still selects
it. See `fe-ui/src/AGENTS.md` §data-icons for the panel-glyph + overlay halves.

**MEDIUM-1 (selected node exclusion).** The single-point track node is a
`Billboard` that is *also* selectable and gimbal-rotatable (Rotate/Move/Scale).
Since `billboard_face_camera` writes `Transform.rotation` every frame, it would
overwrite a gimbal-Rotate on that node each frame. Fix: the system reads
`Res<NodeManager>` and **skips `node_mgr.selected_entity()`** — while a node is
selected its gimbal owns its transform. Every other billboard keeps facing the
camera. Skipping the whole selected entity (not just Rotate mode) is the simple
robust choice: a selected marker not perfectly camera-facing is harmless, a
stomped gimbal is not.

## §glb-mesh-picking — precise ray/AABB node selection (`viewport_pick.rs`)

`handle_viewport_click` picks the node under the cursor ray by intersecting the
ray with each `SpawnedNodeMarker`'s bounding volume, replacing the old
distance-to-origin sphere test (which missed clicks on a glb surface far from
the node origin and false-positived on empty space near it —
`glb_mesh_picking_20260713`).

- **AABB, not mesh-triangle.** `bevy::picking::mesh_picking::MeshRayCast` IS
  available under this workspace's feature set (`mesh_picking` is enabled), but
  the AABB slab test was chosen deliberately: it's a pure, unit-testable math
  helper (`ray_aabb_hit`), it iterates only node roots so it never false-hits
  the terrain plane / path-point marker spheres / gimbal, and the active-petal
  filter applies cleanly per-root. Mesh-triangle picking would need an
  ancestry-filter closure during the cast to achieve the same scoping. Upgrade
  to `MeshRayCast` only if per-triangle precision is later required.
- **Descendant-Aabb walk (FR-2 + depth fix).** glTF scenes attach the mesh +
  `Aabb` several levels BELOW the `SceneRoot`, not on the root marker entity or
  even an immediate child. The original one-level scan therefore found nothing
  for GLBs and deselected them — the regression fixed here. `pick_node_aabb` now
  does an iterative DFS over the root AND its whole subtree (pure helper
  `nearest_in_subtree`, unit-tested), slab-testing each candidate and keeping the
  nearest entry `t`; `gimbal.rs`'s `gimbal_center` mirrors it via `find_in_subtree`
  (first Aabb wins, else the SceneRoot translation). Primitives keep their `Aabb`
  on the root entity, so they were never affected. Whatever descendant geometry
  is hit, selection always resolves to the ROOT `entity` carrying
  `SpawnedNodeMarker` (its `node_id`/`petal_id`), exactly as before.
- **The math (`ray_aabb_hit`).** An `Aabb` is axis-aligned in the entity's
  LOCAL space. Rather than transform 8 corners to world space for an OBB test,
  the world ray is transformed INTO local space via `GlobalTransform::affine()`
  inverse (`transform_point3` for the origin, `transform_vector3` for the
  direction — the direction is left un-normalized so the parametric `t` stays
  identical to the along-world-ray distance and is comparable across entities).
  A standard 3-slab test then yields the entry `t`; boxes entirely behind the
  origin miss, and an origin already inside the box returns `t = 0`.
- **FR-4 (path-point markers) untouched.** `path_point_interaction.rs`'s
  `pick_marker` keeps its along-ray + `PICK_RADIUS` sphere test — correct for
  small fixed-size gizmo spheres and explicitly out of scope. No shared helper
  was extracted; the two picks are independent.

## §track-picking — track ribbon pick (`viewport_pick.rs`) + open-on-select bridge (`pointer/mod.rs`)

Track ribbons render in fe-terrain (`GpxTrackLine` + `Mesh3d`), which can't
attach fe-ui's `SpawnedNodeMarker` (no fe-terrain → fe-ui dep). The
`fractalengine` binary bridges that gap: `gpx_bridge::tag_track_lines_selectable`
tags each rendered ribbon with `SpawnedNodeMarker` a frame after the mesh
appears (idempotent via `Without<SpawnedNodeMarker>`), so `handle_viewport_click`
can then AABB-pick it like any node.

Selecting a track in the viewport must ALSO open it in the Paths tab, but
`NodeManager.selected` (viewport/tree selection) and `PathEditorState.editing_track_id`
(Paths-tab selection) are independent. `open_track_on_select` (`pointer/mod.rs`,
§pointer-manager) closes the loop:
it watches for a *change* in `NodeManager.selected` (a `Local<Option<String>>`
remembers the last one so it fires once per selection, not per frame) and, when
the newly selected `node_id` is a Paths-tab track, dispatches
`UiAction::PathSelectTrack`. fe-ui can't see `GpxTrackLine`, so "is this a
track?" is decided by membership in `PathEditorState.tracks` — the pure
`track_to_open` helper. It skips a track that's already being edited so it never
re-issues the `GetNodeProperties` round-trip that would clobber the in-flight
point buffer.

**FR-2 (`ui_shell_architecture_20260724`) — eager track-list load.**
`track_to_open`'s gate was starved: `PathEditorState.tracks` was populated
ONLY while the Data window's Paths tab rendered (`gis_panel.rs`'s
auto-populate), so a fresh session that never opened that window left the
list empty and every viewport track click silently no-op'd — the bridge above
never fired, so vertex/handle markers never spawned. `advance_petal_tracking`
(`pointer/mod.rs`) now issues the Paths tab's own request
(`UiAction::PathQueryTracks` → `actions::path::query_tracks`) on the
petal-entry/change branch that already existed, so the list is loaded before
the user ever needs to open the Data window. `request_track_list_refresh`
guards against a duplicate `RawQuery` when a request is already in flight;
the transition-only `Local<bool>`/`Local<Option<String>>` pair (unchanged)
guards against re-firing every frame. The two selection authorities stay
split (`ui_ux.md` §5, sacred) — this only feeds Authority B's track list.

**Precise ribbon picking (`path_interaction_20260716`, FR-1).** The old AABB
pick made a km-scale flat ribbon a giant flat box that swallowed clicks meant
for nearby objects. The bridge now also attaches a `TrackPickShape { points,
half_width, centroid }` (fe-ui component, re-exported at
`fe_ui::node_manager::TrackPickShape`, populated in `tag_track_lines_selectable`
from the track's route + style — the RAW world polyline, no y-lift). In
`handle_viewport_click`, an entity carrying `TrackPickShape` SKIPS the AABB slab
test and instead ray-tests the actual polyline: `ray_polyline_hit` returns the
along-ray `t` of the closest segment within `half_width + PICK_SLOP` (0.3 wu),
adding the ribbon's `RIBBON_Y_LIFT` (0.5, matching `track_mesh`) so the test
matches the drawn ribbon. That `t` is the closest-approach distance along the
ray, directly comparable to other nodes' AABB entry `t`, so a nearer real
object still out-picks a track behind it. `TrackPickShape` is refreshed every
respawn (the despawn/respawn redraw cycle re-tags via the `Without` filter), so
it stays in sync with style-width and point edits. `centroid` is the render
entity's baseline `Transform` translation — see §path-segments (FR-4 bake).

## §path-segments — ribbon-segment select, whole-path gimbal, measurement (`path_segment_interaction.rs`)

`path_interaction_20260716` added segment selection (FR-3), the whole-path
gimbal bake (FR-4), and live metric measurement, plus the `TrackPickShape`
picking geometry (FR-1, §track-picking). All the ray/segment math lives here as
pure helpers (`ray_segment_distance`, `closest_ribbon_segment`,
`ray_polyline_hit`, `nearest_segment`) shared by `viewport_pick` (FR-1) and the
two systems below.

- **Segment selection (FR-3).** `handle_path_segment_interaction` claims the new
  `ClickPriority::PathSegment` — ranked between `PathPlace` and `NodePick`. While
  a track is being edited, a fresh press that reaches it (a marker/gimbal didn't
  claim first) ray-tests the edited track's `PathEditorState.points` polyline; a
  hit within `half_width + PICK_SLOP` selects that segment
  (`selected_segment = Some(i)`, clears `selected_point`) and claims the frame so
  it wins the re-pick of the ribbon-as-node. A genuine empty click (no segment,
  and `!arbiter.is_claimed()`) clears the selection. Half-width comes from the
  edited track's live `edited_track_style.width`. `selected_point`/
  `selected_segment` are mutually exclusive and cleared on stop/start-editing and
  on any point-count change (`sync_path_point_markers`, via
  `PathEditorState::clear_path_selection`).
- **Measurement (FR-3).** `sync_path_measurements` writes
  `PathEditorState.total_length_m` (always, while editing) and
  `selected_segment_length_m` (when a segment is selected) as REAL meters =
  ground-plane (XZ) world distance / `PetalMapState.world_scale` (guarded ≤0/
  non-finite → 1.0, mirroring `fe_terrain::scale::sanitize_world_scale`). This
  runs here — not in the Paths panel — because the panel call site
  (`gis_panel.rs`, not editable) can't reach `world_scale`; the panel only
  formats the stored meters via its local `format_distance_m` twin.
- **Whole-path gimbal bake (FR-4).** `render_gpx_tracks` now centroid-anchors
  the ribbon (mesh vertices relative to the centroid, entity `Transform` at the
  centroid), so Move/Rotate/Scale pivot about the path center. On commit,
  `transform_broadcast::broadcast_transform` sees the entity carries a
  `TrackPickShape`, so instead of a node-transform DB write it bakes the gimbal
  delta into every gpx point — `bake_transformed_point(world, centroid,
  transform) = transform.transform_point(world - centroid)` — using the SAME
  `centroid` the mesh was anchored at, and dispatches
  `UiAction::PathTransformPoints { track_node_id, points }`. That resolves to one
  in-place `PathOp::MovePoint` per index (count-preserving; the bridge keeps each
  point's `time_seconds`), keyed on the EXPLICIT track id (not
  `editing_track_id`). The entity transform is then reset to the centroid
  baseline; the queued MovePoints drive the bridge's despawn/respawn redraw so
  mesh + markers + `TrackPickShape` resync at the new positions. N MovePoints
  each seed a `GetNodeProperties` on the first frame (existing bridge behavior),
  so a huge imported track costs N round-trips on commit — fine for a one-shot
  deliberate action; authored paths are small.

## §input-router — per-frame left-click arbitration (`router.rs`)

`router.rs` centralizes "who gets this frame's left-click" so consumers claim
ownership instead of racing ad-hoc booleans (replaces the old
`path_edit_capturing` flag). `ClickArbiter` is a `Resource`;
`resolve_pointer_frame` is the FIRST system in the `.chain()`.

- **Priority table** (highest first — mirrors the old implicit `.chain()`
  order, now explicit):

  | Priority      | Consumer system                     | Claims when                          |
  | ------------- | ----------------------------------- | ------------------------------------ |
  | `PathHandle`  | `handle_path_handle_interaction` (pen_curve_tool_20260722 FR-5, ratified Q7 — runs first) | press picks a bezier-handle marker of the edited track (see §pen-tool) |
  | `Gimbal`      | `handle_path_gimbal_drag` (FR-3 vertex/segment, runs first) → `handle_gimbal_interaction` (entity) | press hits a gimbal axis |
  | `PathMarker`  | `handle_path_point_interaction`     | press picks an existing marker (Pen mode, Select-while-editing per FR-2, or while a drag is active) |
  | `PathPlace`   | `handle_path_point_interaction`     | Pen-mode empty click on terrain      |
  | `PathSegment` | `handle_path_segment_interaction`   | editing + press hits a ribbon segment (FR-3) |
  | `NodePick`    | `handle_viewport_click`             | any remaining fresh press            |

- **How it works.** `resolve_pointer_frame` runs first: it clears `owner` to
  `None`, resolves the pointer `phase` (press/hold/release/hover), applies the
  egui pointer-capture + `ViewportRect` gating ONCE (setting `available`), and
  computes the shared cursor/`Ray3d`. Consumers no longer hold `EguiContexts`.
  Because consumers run highest-priority-first, `claim(who)` is first-claim-wins:
  it succeeds iff `available && owner.is_none()`. A consumer that declines to
  claim (e.g. a gimbal press that misses every axis, or path-point in Select
  mode) leaves the frame for the next consumer down the chain.
- **ab9c53c fix, structurally.** In Select mode path-point does NOT claim
  `PathPlace` on an empty click, so `NodePick` wins (model selection). In Pen
  mode path-point claims `PathPlace`, which outranks `NodePick`, so placement
  wins. No per-system flag bookkeeping needed.
- **Adding a consumer.** Register a `ClickPriority` variant at the right rank,
  add the system to the `.chain()` after `resolve_pointer_frame` in priority
  order, and have it `claim` its priority + read `arbiter.ray()`/`cursor()`.
  No edits to the other consumer systems (this is what unblocks
  `glb_mesh_picking_20260713`).
- **What stays.** Gimbal keeps its own drag hold/release state machine
  (`NodeSelection.drag` / `is_dragging()`) and path-point keeps `PathPointDrag`;
  the router only arbitrates the PRESS-time ownership contest. Hover detection
  (`update_hovered_axis`) is ambient and claims nothing.
- **Same-frame press+release coalesces to Release** (`resolve_phase`): a
  fast click landing in one frame previously reported `Press` with no later
  `Release`, permanently stranding the pen/handle drag machines (the stale
  state replayed on the NEXT unrelated click as a phantom anchor / corner
  set). Release-wins restores the pre-restructure drop-the-click semantics.
  As a second net, `sweep_stranded_drag` clears any live pen/handle/marker
  drag on a Hover frame — the button being up with a drag active can only
  mean its Release was missed.

## §pointer-manager — cross-authority bridge + claim-priority table (`pointer/mod.rs`, `router.rs`)

`ui_shell_architecture_20260724` FR-3. Two pointer/router concerns are now
explicit and consolidated:

- **Claim-priority table (`router.rs::hit_target_rank`).** The pure, total
  `hit_target_rank(&HitTarget) -> u8` makes the object-level pick preference
  explicit: `handle > vertex > segment > gimbal-axis > node > stamp > proposal
  > terrain-cell > empty` (lower rank = higher priority). This is the "which
  target owns a click that could resolve to several" ordering — distinct from,
  but consistent with, the per-system `ClickPriority` `.chain()` that ENFORCES
  it at runtime (§input-router). No in-tree consumer reads it yet
  (`#[allow(dead_code)]`, mirroring the FR-1 arbiter API seams); a coverage test
  asserts the ranks are a permutation of `0..N` over every `HitTarget` variant,
  independent of any derive ordering, so a new variant must be given a rank (the
  match is exhaustive, no wildcard) and slotted into the order.
- **The re-homed bridge (`pointer/mod.rs`).** `open_track_on_select` +
  `advance_petal_tracking` + `request_track_list_refresh` + `spawned_in_petal` +
  `track_to_open` moved VERBATIM out of `viewport_pick.rs` into `pointer/mod.rs`
  — the single home of the cross-authority coordination. Behavior is unchanged;
  the mechanics live in §track-picking (viewport track-select → `PathSelectTrack`,
  plus the FR-2 eager track-list load). Registered in `mod.rs`'s `.chain()` as
  `pointer::open_track_on_select`, still IMMEDIATELY after `handle_viewport_click`
  so it reacts to the same-frame `NodeManager.selected` change (FR-2, commits
  b12b646/190188f) — the chain position is load-bearing and was NOT reordered.
- **No-bypass claim routing.** The consumer systems' CLAIM/select decisions now
  route through `dispatch::resolve_operation` (the FR-2 table) instead of
  hardcoding the verb: `handle_viewport_click` (`Node`/`Empty` →
  `SelectNode`/`Deselect`), `handle_path_handle_interaction` (`PathHandle` →
  `MoveHandle`), `handle_path_segment_interaction` (`PathSegment` →
  `SelectSegment`), `handle_gimbal_interaction` (`GimbalAxis` →
  `BeginGimbalDrag`, entity-backed), and `handle_path_point_interaction`
  (`PathVertex` → `SelectVertex` on a marker pick; `Empty` → `PlacePathPoint`
  on a Pen empty press). `handle_path_gimbal_drag` was already routed (the
  reference impl). Each is behavior-preserving — the resolved `Operation` equals
  the previously-hardcoded action — and the bespoke Press/Hold/Release
  drag-commit state machines are UNTOUCHED (`Operation` doesn't model them);
  only the press-time claim/select decision is routed. For hits whose
  resolution is tool- and selection-INDEPENDENT (handle/vertex/segment), the
  routed `kind` is immaterial (a projected Authority-B kind, or `Empty` where no
  `PathEditorState` is in scope) — the call is a guard that keeps the verb
  decision in the one table, not a behavior change.

## §selection-read-model — typed selection facade (`selection.rs`)

`terrain_editor_overhaul_20260718` FR-1. Two selection authorities exist and are
deliberately NOT merged (codified in `conductor/code_styleguides/ui_ux.md §5`):
`NodeManager.selected` (entity / gimbal selection) and `PathEditorState`
(`editing_track_id` / `selected_point` / `selected_segment`). `SelectionState`
is a **read-only projection** over both, recomputed each frame by
`update_selection_state` (runs late in the `.chain()`, just before
`draw_gimbal_system`), so any system can ask "what kind of thing is selected?"
without touching either store.

- **`project_selection` priority** (pure, unit-tested truth table): path vertex →
  path segment → a selected entity — but the open track's own bridged ribbon
  (`node_id == editing_track_id`) reads as `PathTrack`, so ribbon-vs-node stays
  type-distinguishable for FR-2 — → an open track with no bridged entity → empty.
- **`SelectionKind::{Stamp, TerrainProposal}`** exist for FR-2 / the terrain
  proposal phases but have no selection path yet (nothing sets them).
- **Gimbal on path (FR-3).** `draw_gimbal_system` draws a gimbal for path
  selections even under Select/Pen (which have no gizmo of their own → it shows
  Move arrows). Vertex/segment resolve to a world point via `path_gimbal_target`
  (they have no entity); a whole track keeps its bridged ribbon-entity center so
  the drawn handle matches where `handle_gimbal_interaction`'s axis-pick expects
  it (no regression to the existing whole-track drag). The relaxed gate is for
  the VISUAL only — the axis-pick / drag gate on `handle_gimbal_interaction` is
  untouched.
- **Dragging a vertex/segment via the gimbal (FR-3 drag) — now implemented** in
  `path_gimbal_drag.rs` under `Tool::Move`. Because a vertex/segment has no
  entity, the drag writes `UiAction::PathMovePoint` (→ `PathOp::MovePoint`), not
  an entity `Transform`; a segment moves both endpoints by the same delta. It
  claims `ClickPriority::Gimbal` ahead of `handle_gimbal_interaction` so the
  entity gimbal yields. See §dispatch for the full interaction + the pure
  `drag_delta_to_position` math.

## §dispatch — object-aware left-click table (`dispatch.rs`) + FR-3 gimbal drag (`path_gimbal_drag.rs`)

`terrain_editor_overhaul_20260718` FR-2/FR-3.

**The table (`dispatch.rs`, FR-2).** `resolve_operation(tool, kind, hit) ->
Operation` is a PURE, total truth table keyed on `(active Tool, SelectionKind,
HitTarget)` — the single place that answers "what does left-click DO for this
object type?", and the headroom for the "more operations on left click" ask. It
does NOT replace `router.rs`'s first-claim-wins arbitration (§input-router): the
per-frame consumer systems still own the PRESS contest; the table is the shared
decision model they (and the FR-3 drag) consult. `HitTarget` is the raw pick
result (`Empty`/`Node`/`PathVertex`/`PathHandle`/`PathSegment`/`Stamp`/
`TerrainProposal`/`TerrainCell`/`GimbalAxis`); `Operation` is the resolved verb
(`SelectNode`/
`SelectVertex`/`SelectSegment`/`SelectStamp`/`SelectProposal`/`PlacePathPoint`/
`PlaceNode`/`BeginGimbalDrag`/`MoveVertex`/`MoveSegment`/`MoveHandle`/
`TerrainCellEdit`/
`Deselect`/`None`). Resolution is **hit-first**, modulated by tool/selection only
where intent changes: Pen keeps placing a point even over a grazed node; a
gimbal-axis press drags the current selection. Per the ratified decision
(2026-07-19 "grab it wherever it's shown"), a path vertex/segment resolves to
`MoveVertex`/`MoveSegment` in EVERY tool (its gimbal is always drawn as a Move
handle, so it must always be grabbable); an entity-backed selection (node / stamp
/ whole track) resolves to `BeginGimbalDrag` only in the transform tools
(`Move`/`Rotate`/`Scale`), matching the entity gimbal that stays closed in
Select/Pen. WHEN a given hit can occur is the router's gate, not the table's
concern — the table stays decoupled from the per-tool pick guards.

**Handle variants (pen_curve_tool_20260722 FR-5).** `HitTarget::PathHandle
{ idx, side }` (`HandleSide::{In, Out}`) resolves to `Operation::MoveHandle
{ idx, side }` in EVERY tool, selection-independent — like vertices, WHEN a
handle is pickable is the claiming system's gate (`path_handle_interaction.rs`,
§pen-tool), not the table's. `MoveHandle` is position-free (like `MoveVertex`;
consumers compute positions), preserving the `Operation` `Eq` derive. Handles
are a `HitTarget` only — never a `SelectionKind` — so `resolve_gimbal`'s
lone-vertex/segment "grab it wherever it's shown" arms are untouched; the
claim ordering (handle > vertex > gimbal, ratified Q7) lives in the router
chain, not here.

**FR-3 gimbal drag (`path_gimbal_drag.rs`).** A selected path vertex/segment has
NO entity, so it can't reuse `handle_gimbal_interaction`'s entity-`Transform`
drag. `handle_path_gimbal_drag` computes the new world position from the axis +
cursor delta (`drag_axis_delta`/`drag_delta_to_position`, pure + unit-tested;
the same `* 0.002` camera-distance feel as the entity gimbal via
`move_scale_factor`) and, on RELEASE, queues one `UiAction::PathMovePoint` per
affected index — a segment moves BOTH endpoints by the same delta (parallel
translate: length + orientation preserved). It:

- runs in EVERY tool (2026-07-19 "grab it wherever it's shown") — a
  vertex/segment gimbal is drawn as a Move handle in all tools, so it must be
  grabbable in all tools. It only ever claims on a real axis-hit over a
  vertex/segment selection, so a miss still yields to Pen append / node pick;
  `draw_gimbal_system` correspondingly draws vertex/segment as Move arrows in
  every tool (never a dead rotate ring), while a whole track keeps the entity
  gimbal and so draws only in the transform tools (no drawn-but-dead handle);
- runs in the `.chain()` immediately BEFORE `handle_gimbal_interaction`, and on a
  vertex/segment press that hits the axis it claims `ClickPriority::Gimbal`
  FIRST — so the entity gimbal's own `claim(Gimbal)` fails (first-claim-wins) and
  it never starts a competing entity drag on the bridged ribbon;
- reads the CURRENT selection by calling `project_selection` directly (NOT
  `SelectionState`, which is projected late in the chain and would be a frame
  stale), then asks `resolve_operation(Move, kind, GimbalAxis)` for the
  `MoveVertex`/`MoveSegment` index set;
- gives live feedback by writing the affected `PathPointMarker` transforms each
  hold frame. `handle_path_point_interaction` doesn't fight it: the Gimbal claim
  denies its `PathMarker` claim on the same press, and it keys off a separate
  `PathGimbalDrag` resource. Because `PathMovePoint` → `path::move_point` also
  updates the local buffer, no manual `PathEditorState` write is needed.

Whole-track gimbal drag stays on the entity path (the ribbon has an entity + the
FR-4 bake in §path-segments); FR-3 only adds the vertex/segment case.

**Terrain-cell seam (FR-5, NOT fully wired).** `HitTarget::TerrainCell →
Operation::TerrainCellEdit` exists and is tested, and
`dispatch::terrain_cell_proposal(brush, footprint, target_height, delta) ->
TerrainProposalEdit` builds the payload. `TerrainBrush` enumerates the
Cities-Skylines-style brushes (raise/lower/flatten/ramp/slope/pad/cut/fill);
every brush emits a PROPOSAL, never a destructive terrain write (NFR-1). Two
`TODO(ultrapilot)` seams remain: (a) there is no terrain-edit `Tool` variant to
produce `TerrainCell` hits (adding one touches `panels/toolbar.rs`, outside
node_manager's write scope); (b) the emit target
`crate::actions::UiAction::TerrainProposalAdd { op, footprint, target_height,
delta }` is owned by the p2p/terrain worker — once it lands, the terrain-cell
consumer pushes it directly instead of returning `TerrainProposalEdit`. fe-ui
must NOT depend on fe-terrain, so `TerrainBrush` is a local enum, not a re-export.

## §context-pick — right-click classification (`context_pick.rs`, contextual_controls T4)

`viewport.rs` opens `ActiveDialog::ContextMenu { target: None, .. }` on a
secondary click; `classify_context_menu` (chained right after
`handle_viewport_click`) fills `target` with a `dialogs::ContextTarget`. It
REUSES the left-click pick machinery — the exact `handle_viewport_click` loop
(`TrackPickShape` polyline else `pick_node_aabb` subtree DFS, active-petal
filtered) over a fresh camera ray built from the stored `screen_pos` (right
click never touches the left-click `ClickArbiter`). A hit whose entity carries
`PathAssetInstance` is a stamp: `(track, index)` comes from
`source_track_id` + `verse_manager::parse_stamp_marker_id` (the marker-id
format has one producer, `stamp_marker_id`). When the ray misses, the T2
`StampRenderIndex` ground pick at the click's `world_pos` (radius = one
`DEFAULT_CELL_SIZE_M` grid cell) catches small stamps; its entity resolves by
marker id (`Entity::PLACEHOLDER` mid-respawn — stamp verbs key on the payload,
never the entity). The pure core `resolve_context_target` is unit-tested.
Side effects mirror left-click: node hit → `NodeManager.select`; stamp hit →
`UiAction::SelectStamp` (idempotent, lazy promotion — N-3/N-9). Produces only
`Node`/`Stamp`/`Empty` today; vertex/handle/segment/proposal classification is
future headroom (the menu table is already total over them). Always resolves —
worst case `Empty` — so the menu can't hang unclassified (N-8).
