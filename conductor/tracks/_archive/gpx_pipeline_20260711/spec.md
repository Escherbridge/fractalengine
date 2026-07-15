---
type: Track Spec
title: GPX Pipeline — Import to Endpoint End-to-End
tags: [feature, gpx, terrain, gpx_pipeline_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: GPX Pipeline E2E

**Track ID:** `gpx_pipeline_20260711`
**Crates:** `fe-ui` (import surface), `fractalengine` (bridge), `fe-terrain`
(gpx parsing modules), `fe-database` (handlers/tests; `src/lib.rs` quarantined)

## Problem

Phase 7 shipped the pieces (GPX parsing, `GpxTrackLine` rendering, track
stats endpoints) but there is no end-to-end path: a user cannot import a GPX
file in-app and see it as a petal-bound track with waypoints, nor does the
`gis/tracks` endpoint have real imported data to serve.

## Functional Requirements

- **FR-1 Import surface (fe-ui):** "Import GPX" button (rfd file picker)
  queueing `GpxOp::ImportFile { petal_id, path }` on a new `PendingGpxOps`
  resource + `GpxImportStatus` result resource — exact mirror of the
  `asset_ops` pending/status pattern.
- **FR-2 Bridge (fractalengine):** drains the queue, parses via fe-terrain's
  GPX parser, projects points through the petal terrain origin, persists a
  track node (`properties["gpx_type"] = "track"` + cached stats) and
  waypoint child nodes via existing DB commands only (no new DbCommand
  variants — dispatch lives in quarantined fe-database/src/lib.rs).
- **FR-3 Render + serve:** imported tracks render through the existing
  `GpxTrackLine` flow, bind to the `gpx_track` layer (petal_binding mapping
  delivered by the terrain worker), and appear in
  `GET /api/v1/petals/{petal_id}/gis/tracks`.
- **FR-4 Tests:** pure GPX→node-shape mapping tests + embedded-DB
  handler-level persistence tests.
