# fe-ui/src/node_manager — single source of truth for selection

- `mod.rs` — `NodeManager`/`NodeSelection`/`AxisDrag` types +
  `NodeManagerPlugin` (registers the `.chain()`-ed system order inside
  `UiSet::Selection`) + unit tests for the selection state machine.
- `shortcuts.rs` — keyboard tool-switch shortcuts + Escape-to-deselect.
- `sidebar_sync.rs` — resolves `NodeManager.pending_sidebar_select` (a
  node_id set by a sidebar click) into an ECS `Entity` + `select()` call.
- `gimbal_interaction.rs` — hover detection, axis pick → drag → commit, and
  the gizmo draw system. The largest submodule; keep pure geometry helpers
  (`pick_axis`, `segment_dist_2d`, etc.) private to this file.
- `viewport_pick.rs` — 3-D viewport click → nearest-node select/deselect via a
  precise ray/AABB raycast (see §glb-mesh-picking).
- `path_point_interaction.rs` — path-point viewport editor (see §path-points).
- `billboard.rs` — `billboard_face_camera`, the per-frame face-camera system
  for `Billboard`-tagged icon quads (see §data-icons + `fe-ui/src/AGENTS.md`
  §data-icons).
- `router.rs` — per-frame left-click arbitration (see §input-router).
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

## §pen-tool — pen (phase 1 polyline + phase 2 curves/shapes)

`Tool::Pen` (`panels/toolbar.rs`, hotkey `P` via `shortcuts.rs`) is the
click-to-place tool for drawing a track's polyline. Phase 1 places straight-
segment control points; phase 2 (`curve.rs` + the Tools-panel Pen section)
resamples them into curves and generates shape rings.

- **Gating, not a separate system.** Pen behavior is folded into the
  existing `handle_path_point_interaction` (no new system/registration).
  The empty-click-on-terrain → `PathAppendPoint` branch claims the router's
  `PathPlace` priority only when `tool.active_tool == Tool::Pen`; a marker
  pick claims `PathMarker`, but only in Pen mode (or while a drag is already
  active) — the system's `if !pen_active && drag.active.is_none() { return; }`
  guard returns before `pick_marker` runs in Select/Move/Rotate/Scale.
  Because `PathPlace` outranks
  `NodePick`, a Pen-mode empty click wins the frame and node-pick yields;
  in Select mode path-point declines to claim `PathPlace`, so `NodePick`
  gets the click. This structurally reproduces the ab9c53c fix (node
  selection wins in Select mode) — see §input-router.
- **No new action.** Reuses `UiAction::PathAppendPoint` unchanged; the pen
  tool only changes *when* a click is allowed to emit it.
- **Auto-create on the first click** (`pen_autocreate_track_20260713` +
  HIGH-1/HIGH-2 correlation-id hardening). A Pen empty-click with
  `PathEditorState.editing_track_id == None` no longer no-ops: it claims
  `PathPlace` (so `NodePick` still yields), then — because the new track's
  `node_id` doesn't exist until the DB round-trips — generates a fe-ui-side
  correlation id (`gis::next_pen_correlation_id`), stashes
  `PathEditorState.pending_pen_create` (that id + the click's Y=0 world position)
  and queues `UiAction::PathCreateTrack { petal_id, "New Path", correlation_id:
  Some(id) }`. The petal is the active one, sourced from
  `NavigationManager.active_petal_id` exactly like `viewport_pick`; no active
  petal → keep the no-op (one-line log hint). A second click before the create
  resolves is suppressed by the `has_pending_pen_create()` guard. The deferred
  append lands in `verse_manager::db_results` on the track's
  `DbResult::NodeCreated`, matched by the **echoed correlation id**
  (`take_pending_pen_create_if(cid)`) — NOT the old `!has_asset && active-petal`
  content heuristic, which a concurrent GPX-import/dialog create could hijack. On
  a match it calls `start_editing(new_id)` and pushes the stashed point as the
  track's first `PathAppendPoint`. The full id threading + the `DbResult::Error`
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
  marker → begin a drag on that marker's current y-plane (Pen mode, or while a
  drag is already active — the `pen_active || drag.active` guard gates it);
  release commits a single `PathMovePoint` (no remove+append index churn). Holding **Ctrl** during the drag switches to vertical mode
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
- **Child-Aabb walk (FR-2).** glTF scenes attach the mesh + `Aabb` to a CHILD
  of the `SceneRoot`, not the root marker entity. `pick_node_aabb` mirrors
  `gimbal.rs`'s `gimbal_center` walk — try the root's own `Aabb`, then scan
  immediate children — but slab-tests each candidate instead of centering, and
  keeps the nearest entry `t`. Whatever child geometry is hit, selection always
  resolves to the ROOT `entity` carrying `SpawnedNodeMarker` (its
  `node_id`/`petal_id`), exactly as the old proximity path did.
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

## §track-picking — clicking a track ribbon opens it for editing (`viewport_pick.rs`)

Track ribbons render in fe-terrain (`GpxTrackLine` + `Mesh3d`), which can't
attach fe-ui's `SpawnedNodeMarker` (no fe-terrain → fe-ui dep). The
`fractalengine` binary bridges that gap: `gpx_bridge::tag_track_lines_selectable`
tags each rendered ribbon with `SpawnedNodeMarker` a frame after the mesh
appears (idempotent via `Without<SpawnedNodeMarker>`), so `handle_viewport_click`
can then AABB-pick it like any node.

Selecting a track in the viewport must ALSO open it in the Paths tab, but
`NodeManager.selected` (viewport/tree selection) and `PathEditorState.editing_track_id`
(Paths-tab selection) are independent. `open_track_on_select` closes the loop:
it watches for a *change* in `NodeManager.selected` (a `Local<Option<String>>`
remembers the last one so it fires once per selection, not per frame) and, when
the newly selected `node_id` is a Paths-tab track, dispatches
`UiAction::PathSelectTrack`. fe-ui can't see `GpxTrackLine`, so "is this a
track?" is decided by membership in `PathEditorState.tracks` — the pure
`track_to_open` helper. It skips a track that's already being edited so it never
re-issues the `GetNodeProperties` round-trip that would clobber the in-flight
point buffer.

## §input-router — per-frame left-click arbitration (`router.rs`)

`router.rs` centralizes "who gets this frame's left-click" so consumers claim
ownership instead of racing ad-hoc booleans (replaces the old
`path_edit_capturing` flag). `ClickArbiter` is a `Resource`;
`resolve_pointer_frame` is the FIRST system in the `.chain()`.

- **Priority table** (highest first — mirrors the old implicit `.chain()`
  order, now explicit):

  | Priority     | Consumer system                     | Claims when                          |
  | ------------ | ----------------------------------- | ------------------------------------ |
  | `Gimbal`     | `handle_gimbal_interaction`         | press hits a gimbal axis             |
  | `PathMarker` | `handle_path_point_interaction`     | press picks an existing marker (Pen mode, or while a drag is already active) |
  | `PathPlace`  | `handle_path_point_interaction`     | Pen-mode empty click on terrain      |
  | `NodePick`   | `handle_viewport_click`             | any remaining fresh press            |

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
