# fe-terrain/src/mesh — mesh builders (module notes)

Per-module rationale for this directory. Cross-cutting render-pipeline notes
(track styling, ribbon centroid anchoring, height-field feed) live in
`../AGENTS.md` §terrain_plugin / §ribbon-centroid-anchor.

## Module map

- `mod.rs` — wiring; pure grid helpers (`interp`, `skirt`) always compile,
  bevy-dependent builders (`marker`, `terrain`, `track`, `curve`) are
  `render`-gated.
- `interp.rs` — pure bilinear elevation-grid upsampling (`../AGENTS.md`
  §terrain_plugin).
- `skirt.rs` — pure tile-seam skirt geometry (`../AGENTS.md` §terrain_plugin).
- `terrain.rs` — elevation-grid → terrain surface mesh (+ skirt).
- `track.rs` — `track_mesh` width-aware ribbon + `track_centroid`
  (`../AGENTS.md` §ribbon-centroid-anchor).
- `marker.rs` — waypoint pin/teardrop mesh.
- `curve.rs` — anchor-bezier flattener (§curve below).

## §curve — anchor-bezier flattener (`curve.rs`, pen_curve_tool_20260722)

`flatten_route(points, samples_per_seg) -> Vec<[f32;3]>` turns per-anchor
bezier route points (`TimestampedRoutePoint` with `handle_in`/`handle_out`
relative-meter offsets — see `../iot/AGENTS.md`) into the dense polyline the
rest of the pipeline consumes. It sits UPSTREAM of everything: the flattened
polyline is what gets finite-filtered, centroid-recentred, `track_mesh`ed,
RDP-simplified, and pick-tested — none of those stages know curves exist.

- **One flattener, two consumers.** `render_gpx_tracks`
  (`terrain_plugin.rs`) and the fractalengine bridge's `track_pick_shape`
  (`gpx_bridge.rs`) both call `flatten_route(..., SAMPLES_PER_SEGMENT)` on the
  same route points, so the clickable `TrackPickShape` polyline is the SAME
  geometry as the visible ribbon — clicks hit the curve the user sees. The
  shared `SAMPLES_PER_SEGMENT` const (fixed subdivision, ratified Q10;
  adaptive flattening deferred) is the coupling point: change it in one place
  or render and pick drift apart. Both callers flatten BEFORE the finite
  filter + `track_centroid` recenter, so the §ribbon-centroid-anchor contract
  ("centroid computed from the identical filtered position list the mesh
  uses") holds unchanged for curved tracks.
- **Straight passthrough = byte-identity (NFR-3).** A segment whose BOTH
  bounding handles (`a.handle_out`, `b.handle_in`) are `None` emits the single
  straight endpoint — zero added points — so an all-corner (legacy) track
  flattens point-for-point to the polyline it always rendered: identical mesh,
  identical RDP input, identical pick shape. Only handle-carrying segments are
  subdivided; a one-sided handle still samples the cubic (the absent side
  contributes a zero offset, i.e. a control point AT the anchor).
- **Cubic construction.** Segment i→i+1 samples the de Casteljau cubic
  `[P_i, P_i+out_i, P_{i+1}+in_{i+1}, P_{i+1}]` — a duplicate (~15 lines) of
  `fe_ui::node_manager::curve::push_cubic`, kept local because fe-terrain must
  not depend on fe-ui (NFR-4; the two crates likewise keep separate local
  `CornerKind` enums by design). Pure `[f32;3]` math, no bevy types, so both
  consumers (one of them in another crate) flatten identically and the unit
  tests need no ECS.
- **RDP note.** The `SIMPLIFY_THRESHOLD = 10_000` RDP gate applied inside
  `track_mesh` (`track.rs`; const in `../simplify.rs`)
  operates on the FLATTENED polyline. Authored curves stay far below it
  (critique downgraded this risk), but it is the reason to only subdivide
  handle-carrying segments — an all-corner mega-import must not balloon into
  the simplifier.
- **Raw petal-local meters — no `world_scale` (NFR-1, sacred).** Anchors,
  handle offsets, samples, and ribbon width all live in the same raw meter
  frame; nothing in this directory multiplies geometry or width by
  `world_scale`/`effective_world_scale` (the terrain surface scales; the
  ribbon deliberately does not — `track.rs` doc, `projection.rs`). CODE
  references to `world_scale` under `fe-terrain/src/mesh` must stay at zero
  (comment/doc mentions of the rule itself are the only permitted grep hits);
  the `positions_pass_through_without_scaling` test guards it. A prior
  session's width×world_scale attempt collapsed GPX ribbons to hairlines —
  this is the #1 regression guard.
