---
type: Track Spec
title: GPX Path Editor — Author, Annotate, Export Paths on Terrain
tags: [feature, gpx, ui, terrain, gpx_path_editor_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: GPX Path Editor

**Track ID:** `gpx_path_editor_20260711`
**Crates:** `fe-ui` (editor surface), `fractalengine` (path bridge),
`fe-terrain` (gpx writer), `fe-database` (tests only)

## Vision (user, 2026-07-11)

"You can create gpx with a path editor inside of your nodes. Or a series of
gpx with paths — basically we need a gpx editor as a tool for planning,
defining and annotating paths and processes in the domains." Paths are
first-class planning artifacts: authored on the terrain, annotated,
persisted with the petal, exportable as standard .gpx.

## Functional Requirements

- **FR-1 Paths tab (fe-ui):** GIS panel gains a Paths tab listing the
  petal's track nodes; create-new (named), select-to-edit, delete. Editing
  shows an ordered point list (index, position, per-point remove), append
  points from the 3D cursor position, and an Annotate action per point
  (creates a waypoint node at that position via the existing annotation
  property contract).
- **FR-2 Ops contract:** `PathOp` queue + `PathEditStatus` result resource
  mirroring the gpx_ops/asset_ops pattern; fe-ui does no persistence I/O.
- **FR-3 Persistence (bridge):** trackpoints persist as a `gpx_points` flat
  node property (JSON array of `[x, y, z, time_seconds]`, petal-local
  meters); live edits update `TrackRouteMap` + the `GpxTrackLine` entity
  immediately.
- **FR-4 Materialization:** on petal load/switch, track nodes with
  `gpx_points` repopulate `TrackRouteMap` and respawn `GpxTrackLine` —
  imported AND authored paths render across sessions (closes the
  gpx_pipeline session-scoped-render residual). Import path also persists
  `gpx_points` going forward.
- **FR-5 Export:** `ExportGpx` writes a standard GPX 1.1 file (rfd save
  dialog) — local coords inverse-projected to WGS84 via the petal terrain
  origin; pure writer in fe-terrain's gpx module, unit-tested for
  parse-roundtrip with the existing parser.

## Out of scope (v1)

3D drag-gizmo point manipulation (list-based move/remove suffices);
multi-segment tracks; process semantics beyond paths + annotations.
