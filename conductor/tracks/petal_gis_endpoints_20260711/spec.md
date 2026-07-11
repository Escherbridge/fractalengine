---
type: Track Spec
title: Petal GIS Endpoints — Petal-Scoped Geo Data over REST
tags: [feature, api, gis, petal_gis_endpoints_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Specification: Petal GIS Endpoints

**Track ID:** `petal_gis_endpoints_20260711`
**Crates:** `fe-api` (additive modules only), `fe-query`, `fe-database`
(handlers/tests only — `src/lib.rs` quarantined)

## Overview

Expose each petal's GIS data through petal-scoped REST endpoints so external
consumers (dashboards, IoT orchestrators, the future P2P bucket) can read the
geo layer: nodes with positions + `gis.annotation.*` properties, GPX tracks,
and spatial queries. Data access rides the existing gateway channel
(`DbCommand::RawQuery` — no new DbCommand variants; the dispatch match lives
in quarantined `fe-database/src/lib.rs`).

## Contracts (shared with gis_query_ui_20260711)

- **Annotation properties:** reserved node-property keys
  `gis.annotation.title`, `gis.annotation.body`, `gis.annotation.color`
  (hex string, optional). Geo position = the node's existing
  `position` geometry + `elevation`.
- **fe-query gis builders (W5):** pure functions rendering SurrealQL for
  RawQuery — nodes-in-bbox (local coords), nodes-within-radius, and
  annotated-nodes-for-petal. Positions are stored in petal-local meters;
  lat/lon ↔ local conversion happens API-side via the petal's terrain origin.

## Functional Requirements

- **FR-1** `GET /api/v1/petals/{petal_id}/gis/nodes` — geo-positioned nodes
  with annotations; optional bbox (lat/lon or local) / radius filters.
- **FR-2** `GET /api/v1/petals/{petal_id}/gis/tracks` — GPX tracks bound to
  the petal (mirror the existing `gpx.rs` patterns).
- **FR-3** Auth: Bearer + hierarchical scope RBAC, mirroring the `assets.rs`
  precedent (Viewer+ read). Deny-by-default.
- **FR-4** Additive quarantine-safe wiring: new `fe-api/src/gis.rs`, `mod`
  line in `fe-api/src/lib.rs`, routes in `fe-api/src/server.rs`. MUST NOT
  touch `fe-api/Cargo.toml`, `fe-api/src/rest.rs`, `fe-api/src/assets.rs`,
  `fe-database/src/lib.rs`. Existing deps only.
- **FR-5** W5 data layer: fe-query gis builders + fe-database handler-level
  tests (RawQuery round-trips through an embedded DB) proving the SQL against
  the real schema (geometry casts per `fe-database/src/AGENTS.md`).
