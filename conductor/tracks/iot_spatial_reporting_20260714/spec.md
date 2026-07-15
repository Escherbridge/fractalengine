---
type: Track Spec
title: IoT Spatial Reporting — live IoT data as queryable spatial rows
tags: [feature, analytics, iot, reporting, core, iot_spatial_reporting_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./metadata.json
---

# Specification: IoT Spatial Reporting

**Track ID:** `iot_spatial_reporting_20260714`
**Crates:** `fe-terrain`, `fe-query`, `fe-api` (+ `fe-database`)
**Alignment:** CORE-ANALYTICS · **P1** (parallel to / behind analytics_egress).
See `conductor/roadmap.md §1`.

## Vision / Why

The roadmap names "integrate with IoT services + existing APIs for highly
spatial reporting" as part of the primary goal. `fe-terrain` already has IoT
bits (animation/track-route + sensor plumbing), but there is no
**reporting-facing shape**: IoT readings aren't exposed as **queryable spatial
rows** that `/api/v1/query` + the analytics egress (`analytics_egress_20260714`)
can serve into PowerBI/spreadsheets.

## Functional Requirements

- **FR-1 — IoT ingestion → queryable spatial rows.** Define the reporting shape:
  an IoT reading is `(node/petal spatial anchor, timestamp, metric key, value,
  units)`, queryable by the existing `fe-query` GIS Filter DSL (spatial
  predicates + time range). Land the schema + the write path from the existing
  fe-terrain IoT plumbing into a queryable table/view.
- **FR-2 — Cap the `node_log` cache (from `p2p_unblock_now` FR-2).** The
  unbounded `node_log` clone is O(N)-per-update and "directly hostile to
  IoT-frequency twin updates." Cap it (ring/size bound) so high-rate IoT updates
  don't degrade. (This FR is shared with `p2p_unblock_now_20260711` FR-2 — do it
  here if that track hasn't landed it.)
- **FR-3 — Time-series query support.** Ensure the query surface supports the
  temporal dimension IoT reporting needs: latest-value-per-anchor, time-window
  aggregates (avg/min/max over a window), and spatial+temporal filters together.
  Reuse `fe-query` where possible; extend the GIS builders if needed (respecting
  the local-meters CRS convention).
- **FR-4 — External-API ingestion hook.** A path to pull from existing IoT/API
  services (webhook or poll) into the spatial-rows table, so "integrate existing
  APIs" is real, not just internal sensors. Scope the minimal v1 (one ingestion
  shape) — full connector catalog is future work.
- **FR-5 — Egress alignment.** The IoT spatial rows must flow through the same
  `analytics_egress_20260714` surfaces (parquet/OData/DuckDB) with correct CRS +
  RBAC, so a user reports on live IoT data in PowerBI the same way as static
  spatial data.

## Constraints

- Respect the local-meters↔lat/lon CRS seam (API-layer conversion).
- IoT-frequency writes must not degrade the render loop or the query path —
  bounded buffers, no per-frame O(N) clones.
- No rustfmt, no concurrent cargo, no quarantine contact, no `.unwrap()` in prod.

## Dependencies

- **analytics_egress_20260714** (P0) — the egress surfaces IoT rows flow through.
- **p2p_unblock_now_20260711** — FR-2 (`node_log` cap) overlaps; coordinate.
