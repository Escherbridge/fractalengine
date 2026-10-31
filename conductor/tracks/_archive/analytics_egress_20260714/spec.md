---
type: Track Spec
title: Analytics Egress — copy-paste SQL/URL into PowerBI/spreadsheets (BI last-mile)
tags: [feature, analytics, bi, egress, geoparquet, duckdb, core, analytics_egress_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./metadata.json
---

# Specification: Analytics Egress (BI last-mile)

**Track ID:** `analytics_egress_20260714`
**Crates:** `fe-query`, `fe-api`, `fe-ui`
**Alignment:** CORE-ANALYTICS · **P0** — the single highest-leverage investment
toward the primary roadmap goal. See `conductor/roadmap.md §1`.

## Vision / Why

The roadmap's killer feature: a user copies a **SQL-like query string or an API
URL / connection string** out of FractalEngine and pastes it into **PowerBI / a
spreadsheet / a notebook** to do spatial reporting, with FractalEngine as the
backend. The backbone already exists — this track is the **last mile**, not
greenfield.

**What exists (verified):**
- `fe-api` `/api/v1/query` (`rest.rs::execute_query`) — authed, rate-limited
  (10 q/s/DID), injection-guarded **single-SELECT SQL endpoint**.
- `fe-query` — typed `QueryBuilder` + GIS `Filter` DSL (`d_within`/`within`),
  `columnar/` (provider/context/udf/geoparquet), `duckdb_compat/syntax::translate`,
  `graphql/`, `geo/`.
- Shipped GIS surfaces: `/petals/:id/gis/nodes`, `/gis/tracks` (GeoJSON).

**The gaps this track closes:**
- `fe-query/columnar/geoparquet.rs` is a **STUB** ("full geoarrow-rs integration
  deferred") — no real GeoParquet export.
- No **BI-consumable egress**: no OData feed, no parquet/CSV download, no
  DuckDB-attachable endpoint, no shareable/signed query URL.
- No **copy-paste UX** that hands the user a ready-to-paste string.

## Functional Requirements

- **FR-1 — Finish GeoParquet export.** Replace the stub in
  `fe-query/src/columnar/geoparquet.rs` with real geoarrow-rs (or an equivalent)
  writer producing spec-valid GeoParquet, with correct geometry column + CRS
  metadata (see FR-5). Round-trip test (write→read).
- **FR-2 — BI-consumable egress surface (pick the primary, one fast-follow).**
  Recommended primary: **parquet/CSV download + a DuckDB-attachable endpoint**
  (DuckDB is the lingua franca of PowerBI/Python/spreadsheets, and
  `duckdb_compat::translate` already exists). Fast-follow: an **OData feed**
  (PowerBI's native "Get Data → OData"). Add the `fe-api` route(s):
  e.g. `GET /api/v1/petals/:id/export.parquet?query=...`, and/or a DuckDB
  ATTACH-compatible endpoint. Reuse the existing `execute_query` guards
  (single-SELECT, rate limit, injection). Do NOT weaken those guards.
- **FR-3 — Copy-paste UX (fe-ui).** A panel/affordance that, from the current
  spatial selection / query, generates and copies to clipboard: (a) the SQL
  string, (b) the API URL, (c) the connection string (DuckDB/OData). Show the
  three forms; one-click copy. This is the "start reporting in 30 seconds"
  moment.
- **FR-4 — Query cost/complexity limits (hardening — the pivot introduces this).**
  `execute_query` is rate-limited but a single query has **no cost bound**. Add:
  a row-count cap, a statement timeout, a result-size ceiling. A pasted
  auto-refreshing URL must not be able to issue an unbounded spatial scan. See
  `roadmap.md §4.1`.
- **FR-5 — CRS correctness at the egress boundary (hardening).** The store is
  **petal-local meters**; users/BI think **lat/lon**. Every egress path
  (parquet, OData, GeoJSON, DuckDB) must carry correct CRS metadata and apply
  local↔geographic conversion consistently, at the **API layer** via the petal's
  terrain origin (per `fe-query/src/AGENTS.md §gis`, `§local-coords`). A
  local-meters-labeled-as-degrees export is a data-integrity landmine — make it
  an explicit acceptance test. Depends on `hexon_scale_orchestration_20260712`
  FR-1..5 (the `crs`/scale metadata) for trustworthy numbers.
- **FR-6 — RBAC on query results (hardening; coordinate with `auth_policy_pattern`).**
  Endpoint-level "can this DID call /query" ≠ "which petals/nodes may appear in
  the result set." A shareable query URL must carry + the engine must enforce a
  scope ceiling; downloaded parquet must be pre-filtered to the subject's scope.
  Minimal slice can land with `auth_policy_pattern_20260710`'s first slice.

## Constraints

- Reuse `execute_query`'s hardening; never format user values into SQL (bind
  params only — `fe-query` already does this). No `.unwrap()`/`.expect()` in prod.
- Bevy 0.18, no rustfmt, no concurrent cargo, no quarantine contact.
- CRS conversion stays at the API layer (do NOT push projection into
  fe-query/fe-database — see `fe-query/src/AGENTS.md §local-coords`).

## Dependencies

- **hexon_scale_orchestration_20260712** (P0, FR-1..5) — CRS/scale metadata for
  metrically-correct egress (FR-5).
- **auth_policy_pattern_20260710** (P1 slice) — RBAC-on-results (FR-6).

## Audit deltas (2026-07-14)

- **FR-4:** a 5s statement timeout ALREADY exists (`fe-api/src/rest.rs` ~902)
  and per-DID rate limiting exists (~796); the remaining gap is the row-count
  cap + result-size ceiling only.
- **FR-6:** scope-filter injection for node-table queries ALREADY exists
  (`build_scope_filter`/`inject_scope_filter`, `rest.rs` ~890); the remaining
  delta is the shareable-URL scope ceiling + export pre-filtering, not
  greenfield RBAC.
