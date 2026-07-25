# fe-terrain/src/iot — route tracking + animation (module notes)

- `animation.rs` — `TrackAnimator` (entity-along-route playback),
  `TimestampedRoutePoint`, and — render-gated — `TrackRouteMap` /
  `TrackStyleMap` + `TrackStyle` (see `../AGENTS.md` §track-styling).
- `path_tracker.rs` — `PathTracker`/`SnapResult`: snap a position to a
  recorded route (progress / deviation / distance-remaining metrics).

## §route-point-bezier — anchor fields on `TimestampedRoutePoint` (pen_curve_tool_20260722)

`TimestampedRoutePoint` is the render/persist twin of fe-ui's `PathPointRow`
(it flows through the fractalengine bridge's `in_flight_points` and the
`gpx_points` wire format — see `fractalengine/src/AGENTS.md` §gpx). Phase 1
added the per-anchor bezier fields: `handle_in`/`handle_out:
Option<[f64;3]>` (RELATIVE meter offsets from `position`; `None` = no
handle), `corner: CornerKind` (`Corner`/`Smooth`/`Symmetric`, a fe-terrain-
LOCAL enum — deliberately NOT imported from fe-ui, NFR-4; `to_code`/
`from_code` map it to the wire's `corner_code` float), and `smoothness: f32`
(the 0..1 "corner settings" knob). `is_plain_corner()` is the encoder's
compact-form predicate: no handles + `Corner` + `smoothness <= 0` ⇒ the
legacy 4-slot `gpx_points` row.

Only the mesh/pick flattener consumes the handles
(`mesh/curve.rs::flatten_route`, see `../mesh/AGENTS.md` §curve). The IoT
consumers are curve-agnostic on purpose: `TrackAnimator` interpolates between
raw route-point positions/timestamps and `PathTracker` snaps against the raw
segment list — both read `position`/`time_seconds` only, so curved tracks
change nothing for animation or snap metrics (an animator travels the chord,
not the flattened curve; acceptable until a use case says otherwise).
