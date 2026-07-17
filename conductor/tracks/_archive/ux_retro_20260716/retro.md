---
type: Track Retro
title: Consolidated Batch Retro — 2026-07-16 UX Live-Testing Fixes (four P0 tracks)
tags: [retro, ux, ux_retro_20260716]
timestamp: 2026-07-17T00:00:00Z
resource: ./metadata.json
---

# Consolidated Batch Retro — 2026-07-16 UX fixes

Covers the four P0 UX tracks archived in the 2026-07-17 batch:
`path_interaction_20260716`, `gpx_stamp_persistence_20260716`,
`inspector_units_width_20260716`, `camera_focus_clip_20260716`.

## 1. What happened

The user's hands-on live-testing session on 2026-07-16 produced a
**9-finding batch** (logged as "user live-testing batch #1" in
[ux_qa_review findings](../../ux_qa_review_20260714/findings.md)):
stamp persistence across scene changes, vertex select/move, ribbon
over-selectability, missing whole-path gimbal, over-wide default ribbon,
segment selection + real-metric lengths + metric stamp spacing, inspector
panel blow-out, real-unit transform display, and camera focus/clip. All nine
were triaged same-day into the four implementation tracks above and fixed
that session.

## 2. Per-track outcome (detail in each metadata.json `notes`)

- **path_interaction** — `TrackPickShape` ray-vs-polyline narrow phase,
  vertex select/drag in Select+Pen, per-segment select with m/km readout,
  centroid-anchored ribbon + gimbal baked into `gpx_points` via per-index
  MovePoint ops, default width 0.5. ~28 new tests.
- **gpx_stamp_persistence** — `PathAssetCache` + `materialize_path_assets`
  petal-wide stamp re-materialization from persisted properties, per-track
  change gate, spacing in meters via `world_scale`.
- **inspector_units_width** — `copy_value_box` width-capped copyable
  property values (panel stays 260px), Position (m) / Rotation (deg) /
  Size (m) from node AABB.
- **camera_focus_clip** — `NodeCreated` echoes position, focus resolves live
  `GlobalTransform`, near 0.01 + min_distance 0.05.

## 3. Verification evidence

- Commit `906d9cc` — **local-only, NOT pushed**.
- Full sweep green: **1517 tests**, `clippy -D warnings`, `fmt`.
- **Caveat: in-app verification remains user-gated** — automated evidence
  only; the user has not yet confirmed the fixes hands-on in the running app.

## 4. Carried-forward flags

- **Ribbon-vs-terrain `world_scale` inconsistency** — ribbons/stamps live in
  raw petal-local meters while terrain tiles are scaled by `world_scale`;
  invisible at 1:1. Handed to `hexon_scale_orchestration_20260712` /
  `map_scale_authority_20260716` (audit 2026-07-17 confirmed the two
  disagreeing authorities; see that spec §Audit evidence).
- **Gimbal-commit cost** — committing a gimbal transform on an N-point track
  emits N MovePoint ops in one shot; cheapest first target for any future
  one-shot Paths undo.
- **`road_builder_ux_20260716` `depends_on`** now points at archived tracks
  (`path_interaction_20260716`, `gpx_stamp_persistence_20260716`) — these are
  satisfied deps per the tracks.md convention, not blockers.

## 5. Batch note

The same 2026-07-17 archive batch also retired `tauri_host_shell_spike_20260630`
(shelved spike, exit report delivered — CONDITIONAL GO, OFF-STRATEGY),
`bim_primitives_on_paths_20260712` (FR-8 seam folded into
`iot_spatial_reporting_20260714`), and `hexon_path_asset_20260713` (FR-5
subsumed / FR-6 inherited by `hexon_unification_20260716`); their outcomes
live in their own folders.
