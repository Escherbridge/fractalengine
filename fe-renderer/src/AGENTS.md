# fe-renderer/src — module notes

Design rationale for fe-renderer source modules. Code carries terse one-line
doc comments; the "why" lives here.

## §camera-scale

The orbit camera (`camera.rs`) is tuned for human-scale scenes: far plane
1000 m, zoom-out limit 500 m. When terrain renders at a small `world_scale`
(a whole region compressed into a few hundred world units) those limits clip
the region and the linear scroll zoom becomes unusable at large distances.

`CameraScaleSettings { world_scale }` (a `Resource`, default `1.0`) is written
by **fe-ui** (which already depends on fe-renderer) whenever the active petal's
terrain scale changes. `apply_camera_scale` runs on change and drives:

- `OrbitCameraController.max_distance` = `scaled_max_distance(BASE_MAX_DISTANCE,
  scale)` = `500 / scale`, clamped to `[500, 100_000]`. Smaller scale ⇒ pull
  back further so the scaled-down region fits; clamped so 1:1 is unchanged and
  depth precision stays sane at extreme scales.
- `Projection::Perspective.far` = `scaled_far_plane` = `max(1000,
  max_distance * 2)` — always clears the farthest zoom-out plus the scene.

Both are pure functions with unit tests (1:1 unchanged, region scale grows the
limits, bad/non-finite scale falls back to base). Scroll zoom switched from the
linear `apply_zoom` to `apply_zoom_proportional` (log zoom): the step is a
fraction (`ZOOM_FRACTION`) of the current distance, so a notch feels the same at
5 units or 50 000 units. `apply_zoom` is kept (still `pub`, tested) for callers
that want linear behavior.

`apply_camera_scale` never touches `near` — set once at spawn (`near: 0.01`),
see `camera_focus_clip_20260716` FR-3; Reverse-Z (Bevy 0.18) keeps depth
precision sane at that value. Since `ux_interaction_hardening_20260718` FR-5 it
DOES drive `min_distance` = `scaled_min_distance(BASE_MIN_DISTANCE, scale)` =
`0.05 × scale`, clamped to `[0.02, 0.05]`: constant *real-world* zoom-in
approach across scales, never above base — so 1:1 close-GLB inspection behaves
exactly as camera_focus_clip tuned it, while region-scale worlds allow
proportionally closer approach for up-close planning. The 0.02 floor is 2× the
fixed near plane: `near < min_distance` must hold at EVERY scale or close zoom
sees through the focused object (near never scales — the invariant the floor
protects).

## §camera-easing

Scroll zoom and focus fly-to steer *targets* (`target_distance`,
`target_focus`) that `orbit_camera_system` eases toward with frame-rate-
independent exponential decay (`exp_approach`, rate `EASING_RATE`/s). The snap
threshold is `EASING_SNAP_EPS × viewing distance` — relative to how far the
camera is from its focus, NOT to the target's coordinate magnitude; a
magnitude-relative epsilon teleports the final ~50 world units of a fly-to for
a node 50 km from the projection origin (review finding, 2026-07-18). A fly-to
target under the terrain is floored to the surface each frame so the flight
converges instead of stalling pending forever. `easing_rate: 0.0` is raw
mode — targets apply instantly — kept for tests and as an escape hatch.
Manual pan or WASD/fly cancels a pending `target_focus` (interrupt semantics:
the user's hand always wins over an in-flight fly-to). fe-ui's
`apply_camera_focus` writes targets, not `focus`/`distance`, so node focus is
a smooth fly-to instead of the old two-step teleport. Orbit (yaw/pitch) stays
un-eased deliberately: mouse-drag orbit is already continuous and adding lag
there feels like input latency, not smoothing.

## §terrain-height

`terrain_height.rs` defines `TerrainHeightField` — resident terrain heights
keyed by tile `(zoom, x, y)` — so the camera can clamp above ground without
per-frame tile decodes (ms-scale, rejected), `Assets<Mesh>` vertex readback
(`VertexAttributeValues` access gotcha), or per-frame raycasts (deliberately
avoided, fe-ui node_manager AGENTS.md). The TYPE lives here because fe-ui and
fe-terrain both must not invert layering (fe-ui ↛ fe-terrain); fe-terrain's
render feature (which optionally deps fe-renderer) POPULATES it in lockstep
with `TerrainChunk` spawns/despawns/petal-switch — the same channel pattern as
`CameraScaleSettings`. Grids are bilinear-downsampled to ≤65×65 (~16 KB/tile,
≤ ~4 MB at max_chunks=256) — the camera clamp needs meter-scale fidelity, not
mesh-scale. `height_at` scans resident tiles (≤ max_chunks, no allocation)
preferring the highest zoom; readers treat the resource as `Option<Res>`
(headless-safe).

Ground avoidance (`orbit_camera_system` tail) is two-stage. The orbit `focus`
floors at the terrain surface (margin 0 — focusing a point ON the ground is
legitimate). The camera body then stays `ground_margin(world_scale)` clear of
terrain **primarily by clamping pitch** (`min_pitch_above_ground`): the camera
stays exactly on the orbit sphere, so `controller.distance` never diverges from
the on-screen viewing distance — a transform-level y-raise alone creates a
zoom/pan dead-zone (scroll shrinks `distance` invisibly, pan speed collapses;
review finding, 2026-07-18). The y-raise + re-`look_at` remains only as the
steep-slope fallback (neighboring ground higher than the sampled column), and
`apply_pan` uses the max of `controller.distance` and the actual camera-focus
separation to stay usable on those fallback frames. `ground_margin` =
`GROUND_MARGIN_M × world_scale` floored at `MIN_GROUND_MARGIN` (0.03 = 3× the
fixed near plane): a purely scaled margin drops below the near plane at region
scales and the near volume clips a hole through the terrain the camera is
"safely" above — same invariant family as the `scaled_min_distance` floor. No
height data ⇒ no clamp (petals without terrain keep free flight). Pure pieces
(`clamp_above_ground`, `min_pitch_above_ground`, `ground_margin`,
`HeightTile::sample`) are unit-tested headless.

Known limitation (accepted 2026-07-18, revisit if it annoys in-app): the field
tracks chunk *existence*, not rendered visibility — terrain hidden via a layer
toggle still floors the camera. Syncing with `sync_layer_visibility` is the
follow-up if that proves wrong in practice.

## §terrain-overlay

`terrain_overlay.rs` is the material/marker factory for fe-terrain's
NON-destructive terrain proposal ghosts (terrain_editor_overhaul_20260718 FR-5).
It lives HERE, below fe-terrain in the dep graph, because the ghost material and
the `ProposalGhost` marker are pure render concerns and fe-terrain already deps
fe-renderer under `render`. The proposal *geometry* + records stay in fe-terrain
(`terrain_proposal.rs`); this file only produces the look.

- `ghost_material(rgba) -> StandardMaterial`: translucent (`AlphaMode::Blend`),
  `unlit` (tint reads as intent, not lighting), `cull_mode: None` (a thin
  proposal surface is visible from below).
- `op_tint(op_snake: &str) -> [f32;4]`: distinct translucent tint per op so
  raise/lower/cut/fill/… read apart (added=warm/green, removed=red/orange,
  reshaped=blue/violet); unknown tags fall back to `PROPOSAL_GHOST_RGBA` (cyan).
  Takes a **string**, not `TerrainOp` — that enum lives in fe-terrain, above this
  crate, so importing it would invert the dep graph. fe-terrain passes
  `proposal.op_snake()`.
- `GHOST_ALPHA = 0.35` is the shared translucency; `ProposalGhost` is the scene
  marker component for spawned ghosts.
- `sample_base(&TerrainHeightField, x, z)`: READ-ONLY passthrough to `height_at`
  for grounding a ghost. Takes `&TerrainHeightField` (shared ref) so it can never
  mutate the true heightfield — the NFR-1 analytics contract at the type level.
- Sculpt brush cursor (T3): `brush_ring` / `brush_overlay_positions` /
  `BRUSH_OVERLAY_RGBA`. fe-ui consumes them via immediate-mode `Gizmos`
  (`sculpt_cursor.rs`), so the once-planned `BrushOverlay` entity marker was
  removed (no entity lifecycle, no dead code).
- `EarthworkVolumeReport { petal_id, region_id, cut_m3, fill_m3 }` (bevy
  `Message`): the fe-renderer-as-shared-seam idiom — written by fe-terrain's
  earthwork bake, consumed by fe-ui's volume persistence. Both sides
  `add_message` it (idempotent); neither may dep the other.

### P1+P2 seam types (terrain_proposal_delta_viz_20261025, S1b)

New cross-crate types for cut/fill heatmap visualization (P1) and volume reporting
(P2); all live here to avoid fe-ui→fe-terrain dependency (NFR-5, OD-10).

- `cut_fill_rgba(dh_m, ramp_max_m, neutral_m) -> [f32;4]`: Diverging color ramp
  (OD-1, OD-2). Red family for cut (dh < 0), blue for fill (dh > 0), transparent
  inside ±neutral_m band, saturated beyond ±ramp_max_m. Intensity graded by
  |dh| in real meters. Pure function; fully tested via table-driven flow test.
- `heatmap_material() -> StandardMaterial`: Unlit, blended, double-sided (like
  `ghost_material`) but white base for per-vertex color tinting (FR-2).
- `ProposalPreview` (Resource, S1b): Armed tool draft state for live preview
  (palette op / shape / brush stroke). Holds petal_id, op, footprint, target,
  delta, strength, and a `PreviewSource` discriminator. The render system in
  fe-terrain reads this and draws the lattice ghost + heatmap without DB or
  field writes (FR-3).
- `ProposalPreviewReport` (Message, S1b→S2→S3): Live cut/fill/net from the armed
  tool, sent by fe-terrain's preview system, consumed by fe-ui's readout panel.
- `VolumeReportRequest` (Message, S6→S5): User selection request (footprint,
  petal, correlation ID). Reserves an optional elevation band for P4 3D volumes.
- `VolumeReportResult` (Message, S5→S6): Complete volume report (area, cut/fill,
  net, lattice step, per-item breakdown, overlap flag, scaled indicator). Each
  item includes its standalone volumes clipped to the selection footprint.
  Consumed by the Volume report panel to render P2 FR-7/8.

## §camera-capture

`agent_vision_20261031` V3 gives the orbit camera a lock and two marker
components so an agent-posed screenshot/`GET /camera` capture (V5, in
`fractalengine`) can hold, move, and share the main camera without fighting
`orbit_camera_system` or `deactivate_foreign_cameras`.

- **`orbit_from_look_at(position, target) -> Option<(focus, distance, yaw,
  pitch)>`** is the exact inverse of `compute_orbit_transform`: `pitch =
  asin(offset.y / distance)`, `yaw = atan2(offset.x, offset.z)`. It returns
  `None` on non-finite input or `position == target` (zero-length offset —
  no direction to recover), never panics. A camera-kind pose read back
  through `restore: false` and `GET /camera`'s orbit form both go through
  this; roll from a non-`Y` `up` cannot be expressed in orbit form and is
  dropped there (documented at the call site, not here).
- **`pose_transform(position, target, up) -> Option<Transform>`** wraps
  `Transform::looking_at` with the same non-finite/degenerate guards plus a
  parallel-`up`-to-view-direction check (`forward.cross(up)` vanishing) —
  `looking_at`'s own degenerate case, caught before it reaches bevy. This is
  the `smart_aim` NaN crash class (RUNBOOK, `ui-semantics-audit-20260808`):
  every float an agent supplies is validated finite *and* every geometric
  degeneracy is rejected, so a capture request can never hand the render
  world a NaN transform.
- **`CameraCaptureLock`** (`Resource`, unit struct — its *presence* is the
  lock, not a bool field) + **`camera_unlocked(Option<Res<CameraCaptureLock>>)
  -> bool`**, registered as `orbit_camera_system.run_if(camera_unlocked)`.
  This is a `run_if`, not an early `return` inside the system, because
  `orbit_camera_system` recomputes `*transform` from controller state EVERY
  frame (see module doc) — a raw `Transform` write made while the system is
  unlocked would otherwise be overwritten on the very next frame it runs.
  Gating entry is the only way a capture job can hold a pose across frames.
  `orbit_camera_system` cannot run in a headless test `App` (it needs
  `bevy_egui`'s render-feature `EguiUserTextures` resource as a
  `SystemParam`, which a bare `MinimalPlugins` app doesn't provide — the
  system would silently be skipped by Bevy's param validation, making any
  assertion about it a tautology). `camera.rs`'s test instead proves the
  identical `run_if(camera_unlocked)` wiring against a bare `Schedule` with a
  stand-in system; the real wiring is covered by `fractalengine`'s V5
  Bevy-routing tests and the user-gated in-app verify (AC-11).
- **`CaptureCamera`** (marker) and **`StreamingFocus`** (marker):
  `spawn_orbit_camera` always adds `StreamingFocus` (the main camera is
  always a streaming focus for fe-terrain's LOD ring, V4 §tile-residency).
  `deactivate_foreign_cameras` gained `Without<CaptureCamera>` on its query —
  without it, a scene-mode capture camera (V5 spawns a second `Camera3d`)
  would be deactivated the same frame it appears, since the query only
  excludes `OrbitCameraController` today. `CaptureCamera` carries no data; a
  capture job's lifetime is this component's lifetime (spawn on job start,
  despawn on job end).
- **`frame_world_box(min, max, view_dir, fov) -> LookAtPose`** (advocate
  pass, 2026-11-01) is the `focus` pose's framing math: keep the current view
  direction and back off to `radius / sin(fov/2) × FRAMING_MARGIN` so the
  box's bounding sphere fits the vertical FOV (`MIN_FRAMING_RADIUS` keeps a
  bare point frameable; a vertical view swaps `up` to +Z so `pose_transform`
  never sees a parallel `up`). It moved here from `fractalengine`'s
  `vision_bridge.rs` — **Humble Object**: every pose formula (orbit, look-at,
  box framing here; geo/tile framing in fe-terrain `camera_framing.rs`) is a
  pure, table-tested function, and the Bevy orchestrator only gathers ECS
  inputs and applies the result.

## §instancing

`instancing.rs` (stamped_asset_nodes_20260725 T2 FR-4 / D-A6 / N-9) is the
CPU-side data layer for rendering + picking tens of thousands of path-asset
stamps without a per-stamp ECS entity. Deliberately render-backend-agnostic:
pure data + math, no bevy render internals, so it unit-tests headless and the
concrete draw wiring stays in the app.

- `StampInstanceData { position (m), rotation (xyzw), scale, stamp_index }` +
  `to_matrix() -> [f32;16]` — a manually-composed column-major TRS matrix (no
  external math dep, explicit + tested). Overrides (FR-3) fold into rotation/
  scale here so the instanced draw agrees with the individually-addressed node.
  Position is petal-local meters (N-1) — no `world_scale`.
- `InstanceBatch` groups instances by asset (one mesh/material handle → one
  instanced draw); `batch_by_asset` splits a mixed stream. `instance_matrices`
  is the flat per-instance buffer a custom instanced pipeline uploads. Bevy's
  automatic instancing already coalesces entities that share a `Mesh3d` +
  material, so the app can start there and graduate to a custom pipeline later.
- `StampSpatialIndex` — a uniform-grid hash over the XZ plane
  (`DEFAULT_CELL_SIZE_M = 4 m`). `pick_nearest(x, z, radius)` scans only the
  cells overlapping the radius (a constant 3×3 window at the default), so a
  viewport pick is O(1) amortized regardless of total stamp count — this is the
  FR-4 "pick returns the correct individual stamp at ≥10k" guarantee. The
  returned index IS the stamp's instance index (feeds `PromoteInstance`).
- `TARGET_INSTANCE_CEILING = 10_000` is the documented budget; the
  `bench_10k_build_and_pick` test is the scale/correctness guard (a timed
  criterion harness is a follow-up). Degenerate inputs (zero/NaN cell size,
  empty index, zero radius) are guarded, never panic.


## ?pointer-region (QA-0019)

The UI supplies `CameraPointerRegion` in egui points and the captured pixels-per-point factor. Orbit
controls read the current physical window pointer and convert once; wheel/motion outside this region
are consumed without changing the camera. Foreground egui areas also block pointer controls. This is
independent of `is_using_pointer`, which only describes active widget interaction and misses hovering
scroll areas. The renderer owns the region type, preserving fe-ui ? fe-renderer dependency direction.
A standalone renderer without this optional resource keeps its existing behavior; UI initializes it to
NOTHING until its first layout. Last completed layout is used by Update, before the next egui pass.
