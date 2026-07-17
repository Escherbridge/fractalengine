---
type: Track Spec
title: GPX Path-Asset Stamp Persistence + Metric Spacing
tags: [bug, ui, gpx, path-asset, persistence, gpx_stamp_persistence_20260716]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Stamp Persistence + Metric Spacing

**Track ID:** `gpx_stamp_persistence_20260716`
**Crates:** `fe-ui`, `fe-sdk`

## Vision / Why (user UX testing, 2026-07-16)

"Stamping on gpx does not persist from scene change states" and "the path stamp
spacing should use real metric units."

## Root cause (investigated 2026-07-16)

Stamped instances are ephemeral Bevy entities (`spawn.rs:73-106`) spawned by
`reconcile_path_asset` (`path_asset_reconcile.rs:61-121`), which is gated on the
Paths-tab `editing_track_id`. `respawn_on_petal_change` despawns them by
`petal_id` and only respawns real `VerseManager` nodes — nothing re-consumes the
persisted `path_asset` descriptor on petal load, and the single-slot
`PathAssetApplied` gate is never invalidated on despawn. The FR-5 PetalHexon
bake (hexon_path_asset_20260713) was never built.

## Functional Requirements

- **FR-1 — Petal-wide re-materialization.** Stamps regenerate from the persisted
  `path_asset` + `gpx_points` node properties on petal (re)entry, independent of
  the Paths-tab selection — mirroring `primitive_materialize.rs`.
- **FR-2 — Per-track change gate.** Replace the single-slot `PathAssetApplied`
  with a per-track keyed cache, invalidated when the stamp group is despawned or
  descriptor/points change. Live editing restamps promptly, without
  double-stamping against the petal-wide materializer.
- **FR-3 — Metric spacing.** `spacing_value` is interpreted as METERS; sampling
  converts to world units via the active petal's world scale
  (`PetalMapState.world_scale`, world units per meter; real = world / scale).
  Stamp UI labels the field in meters.

## Out of scope

Durable FR-5 PetalHexon bake (remains on hexon_path_asset_20260713).
