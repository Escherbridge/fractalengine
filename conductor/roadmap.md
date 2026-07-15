---
type: Product Roadmap
title: FractalEngine Strategic Roadmap — Spatial Analytics Engine + Hexon Foundry
tags: [roadmap, strategy, analytics, bim, hexon, marketplace]
timestamp: 2026-07-14T00:00:00Z
resource: ./product.md
---

# Strategic Roadmap (2026-07-14)

Captured from user direction after the GPX-reorient + in-app-feedback work
landed. This is the north star that reframes near-term track selection. It
supplements `product.md` (the foundational P2P-3D-twin concept) — it does not
replace it.

## The repositioning

FractalEngine's **primary** identity is evolving from "P2P 3D digital-twin
editor" into a **spatial analytics / reporting engine** — a BIM-style reporting
solution. The 3D world-building stays, but the headline value becomes:
**highly-spatial reporting that plugs into existing BI + IoT tooling.**

Three separable initiatives, in priority order:

## 1. PRIMARY — FractalEngine as a spatial analytics engine (BI egress)

**The killer feature:** a user copies a **SQL-like query string or an API URL**
out of FractalEngine and pastes it into **PowerBI / a spreadsheet / a notebook**
to start reporting immediately, with FractalEngine as the spatial backend.
Should integrate cleanly with **IoT services + existing APIs** for spatial
reporting.

**Reality check — the backbone already exists (this is last-mile, not
greenfield):**
- `fe-api` `/api/v1/query` (`rest.rs::execute_query`) — authenticated,
  rate-limited (10 q/s/DID), injection-guarded **single-SELECT SQL endpoint**.
  A BI tool could hit it today. ✅
- `fe-query` — typed `QueryBuilder` + `Filter` DSL with **GIS predicates**
  (`d_within`, `within`, geo distance), plus `columnar/` (GeoParquet +
  arrow-style provider/context/udf), `duckdb_compat/syntax::translate` (SQL
  dialect translation toward **DuckDB** — the engine PowerBI/Python/spreadsheets
  all speak), `graphql/`, `geo/`. ✅ (scaffolding)
- `fe-api` GIS endpoints: `/petals/:id/gis/nodes`, `/gis/tracks` (GeoJSON-shaped).
- **CRS seam (critical, already designed):** `node.position` stores
  **petal-local meters**; lat/lon↔local conversion lives at the **API layer**
  via the petal's terrain origin (see `fe-query/src/AGENTS.md §gis`,
  `§local-coords`). BI egress MUST respect this — users think lat/lon, the store
  is local meters.

**The gaps (what "paste into PowerBI" actually needs):**
- GeoParquet export is a **stub** (`fe-query/columnar/geoparquet.rs` — "full
  geoarrow-rs integration deferred"). Finish it.
- No BI-consumable egress surface: no **OData feed**, no **parquet/CSV
  download**, no **DuckDB-attachable** endpoint, no **signed/shareable query
  URL**. Pick the egress shape(s) PowerBI + spreadsheets actually consume.
- No **copy-paste UX**: a panel that generates the query string / API URL /
  connection string from the current spatial selection and hands it to the user.
- IoT ingestion → queryable spatial rows (fe-terrain has IoT bits; needs the
  reporting-facing shape).

**Candidate track shape (for later planning):** `analytics_egress_*` —
(a) finish GeoParquet, (b) add BI egress (OData or DuckDB-attach or parquet
download — decide), (c) copy-paste query/URL UX, (d) IoT→spatial-rows reporting
path. This is the highest-leverage next investment.

## 2. Hexon crafting infrastructure — likely a SEPARATE PROJECT ("its own forge")

A dedicated **hexon foundry** + a **satellite-data → iroh-blob → hexon
pipeline** that auto-publishes satellite data as consumable hexons on P2P, while
letting others craft their own hexons.

**Business model (explicit):** the **hexon format stays OPEN**; the **foundry +
official registry are CLOSED-source** to enable a **marketplace**.

**Reality check — foundations exist across repos:**
- `fe-hexon` crate (registry, package, publisher, handlers, P2P) + WIT plugin
  interface; `remote` feature = `RemoteRegistryClient`.
- `fe-hexon-registry` crate — a hosted registry HTTP service (mirrors the relay
  container pattern; `docker/Dockerfile.hexon-registry`, `compose.dev.yml`).
- Sibling repo **`gis-tile-etl`** — already builds real US-region hexons from
  public APIs (the seed of the satellite pipeline).
- `hexon_scale_orchestration_20260712` (planned) — real-world scale +
  rulers/measurement.
- iroh 0.35 P2P blobs are the transport; relay EOL 2026-12-31 is a clock.

**Strategic call to make:** where does the line fall between the **open-core**
FractalEngine (engine + format + a reference foundry?) and the **closed**
commercial foundry/registry? This governs what lives in THIS repo vs. a new
dedicated project. Likely: open format spec + open reference tooling here;
closed foundry + official registry + marketplace as a separate product.

## 3. Cross-cutting — hardening + sleeker UX

- **Hardening:** the analytics pivot raises the bar (multi-tenant query safety,
  rate/cost limits, CRS correctness, RBAC on query results, blob provenance).
  Known standing gaps: RBAC not enforced in fe-hexon (Phase 8.4), replication
  mock-backed, `fe-plugin` should depend on `fe-sdk`.
- **UX:** refined editing + sleeker features. **The user will spec a dedicated
  UX track themselves after a thorough QA review** — do not pre-empt it; capture
  QA findings for that track when they surface.

## Sequencing recommendation

1. **Now:** finalize/harden what shipped (GPX/path/analytics surfaces), keep the
   conductor board honest (this pruning pass).
2. **Next high-leverage:** the analytics-egress last mile (initiative 1) — it's
   close and it's the primary goal.
3. **Parallel/independent:** scope the hexon-foundry open/closed split
   (initiative 2) as a separate project decision; the satellite→iroh→hexon
   pipeline can prototype in `gis-tile-etl`.
4. **User-driven:** UX track after the user's QA review (initiative 3).

See also project memory: [[gis-tile-etl-repo]], [[hexon-scale-orchestration-track]],
[[hexon-p2p-commons-decisions]], [[platform-vision-directives]],
[[track-selection-two-concepts]].
