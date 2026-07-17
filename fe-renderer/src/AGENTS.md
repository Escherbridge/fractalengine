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

`apply_camera_scale` never touches `near`/`min_distance` — those are set once at
spawn (`near: 0.01`) and in `OrbitCameraController::default()` (`min_distance:
0.05`), see `camera_focus_clip_20260716` FR-3. Reverse-Z (Bevy 0.18) keeps depth
precision sane at that near value.
