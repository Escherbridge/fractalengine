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
