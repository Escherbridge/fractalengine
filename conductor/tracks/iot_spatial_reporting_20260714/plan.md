---
type: Implementation Plan
title: IoT Spatial Reporting — implementation plan
tags: [plan, analytics, iot, reporting, timeseries, iot_spatial_reporting_20260714]
timestamp: 2026-07-15T00:00:00Z
resource: ./spec.md
---

# Plan: IoT Spatial Reporting

**Track:** `iot_spatial_reporting_20260714` · **P1 / CORE-ANALYTICS** · Crates: `fe-database`, `fe-query`, `fe-api`
Spec: `./spec.md` · Roadmap: `conductor/roadmap.md §1` · Depends on: `analytics_egress_20260714` (landed — query_guard/limits/export/share all exist as of f062921)

## Verified starting state (2026-07-15 — do not re-derive)

- fe-terrain's "IoT plumbing" (`fe-terrain/src/iot/{animation,path_tracker}.rs`) is **path-tracking/animation only** — there is NO sensor-reading store anywhere. FR-1 builds the first one.
- `fe-database/src/schema.rs` — `define_table!` macro; tables registered in `apply_all` + `ALL_TABLE_NAMES` (currently 14, test asserts the count). Handlers live in `fe-database/src/handlers/` (append pattern: `node_log.rs` — HLC via `crate::op_log::next_hlc_timestamp`, inserts via `fe_query::InsertBuilder`, exec via `query_helpers::exec_query`).
- Geometry rule (`fe-database/src/AGENTS.md §geometry-inserts`): geometry-typed columns need hand-written cast queries — **the reading row carries NO geometry** (position joins through the anchor node), so `InsertBuilder` is legal here.
- `fe-query/src/builder/` — typestate `QueryBuilder`, `Filter` (eq/gte/lt/d_within/in_subquery/raw), `render.rs` param-counter rendering. `Filter::in_subquery` remaps params. No time-series helpers yet.
- `fe-api` egress backbone (landed): `query_guard.rs` (`validate_select_sql` + `ALLOWED_TABLES` whitelist + `build_scope_filter`/`inject_scope_filter` + `check_rate_limit` + `run_guarded_query`), `limits.rs` (named caps), `export.rs` (petal-scoped auth idiom: `require_role` → `is_valid_ulid` → `resolve_petal_scope` → `require_scope`), routes in `server.rs`. Integration-test idiom: `fe-api/tests/export_share_test.rs` (in-memory SurrealDB + direct handler calls + READ-BACK).
- fe-api writes normally round-trip `DbCommand` over crossbeam to the DB thread; `db_reader` is a full `Surreal<local::Db>` handle typed identically to `fe_database::repo::Db`.

## Decisions (provisional — logged in open_decisions)

- **D1 — Ingestion write path:** the fe-api handler calls `fe_database::handlers::iot_reading::insert_readings` **directly on `db_reader`** (no new `DbCommand` variant). Rationale: the DB-thread single-writer invariant only protects derived counters (`node_log.row_version`); `iot_reading` rows are append-only with ULID ids and no derived state, so concurrent inserts are safe, and IoT-frequency batches should not queue behind the render-loop's DB channel. Documented in `fe-database/src/AGENTS.md §iot-readings`.
- **D2 — Timestamp shape:** three fields — `recorded_at` (RFC-3339 string, sensor time, normalized to UTC), `recorded_at_ms` (i64 epoch-ms, **the canonical range-filter/window column**), `hlc_timestamp` (packed HLC i64, ingestion order / distributed merge, same as `node_log`). Client may supply `recorded_at`; server time is the default.
- **D3 — Scope seam:** `petal_id` is denormalized onto every reading row so the existing `build_scope_filter` (`petal_id = '<p>'`) applies; `inject_scope_filter` is extended to also inject on `FROM iot_reading`.
- **D4 — Ingestion limits:** Editor+ role (it's a write), batch cap 1 000 readings/request, 10 ingest req/s per DID — named constants in `limits.rs`.
- **D5 — Spatial predicate:** readings carry no geometry; spatial+temporal queries filter `node_id IN (SELECT VALUE node_id FROM node WHERE <spatial predicate>)` — the anchor node owns the position.

Style constraints (all phases): ~300-line files, terse one-line doc comments (rationale in directory `AGENTS.md`), no `.unwrap()`/`.expect()` in prod paths, thiserror + `?`, tracing logs, no new ad-hoc role checks (reuse `require_role`/`require_scope`), READ-BACK tests for persisted state, single test sweep at session end.

---

## Phase 0 — FR-2: node_log cap (PRE-SATISFIED)

- [x] **Task 0.1 — `node_log` ring cap.** *(Pre-satisfied: landed via `p2p_unblock_now_20260711` FR-2 in commit `9ef76a1` — bounded node_log ring in `fe-entity-store`. Nothing to do here; recorded per spec's "do it here if that track hasn't landed it" clause — it has.)*

## Phase 1 — FR-1: IoT reading schema + write path [fe-database]

- [x] **Task 1.1 — `iot_reading` table schema.** *(Done 2026-07-15: `IotReading` in schema.rs exactly as specced; 3 indexes incl. `idx_iot_reading_series (node_id, metric, recorded_at_ms)`; `ALL_TABLE_NAMES` 14→15; 3 new schema unit tests incl. a no-geometry guard test.)*
  Files: `fe-database/src/schema.rs`.
  `define_table!` for `iot_reading` → `IotReading` (id: `reading_id`): `reading_id`, `node_id` (anchor), `petal_id` (D3 denormalized), `metric`, `value: f64`, `units` (default `''`), `recorded_at` (RFC-3339), `recorded_at_ms: i64`, `hlc_timestamp: i64`, `source_did` (default `''`). No geometry columns (D5). Register in `apply_all` + indexes (`node_id`; `petal_id`; `node_id, metric, recorded_at_ms`) + `ALL_TABLE_NAMES` (count 14→15, fix the count test).
  Accept: schema tests assert field DDL present; count test updated.
- [x] **Task 1.2 — Write handler `handlers/iot_reading.rs`.** *(Done 2026-07-15: as specced; thiserror added to fe-database deps (workspace `1`); validation is fully front-loaded (anchors + timestamps + metrics) before the first insert; per-row `InsertBuilder` CREATEs — a mid-batch DB error can leave a partial batch, documented in AGENTS.md §iot-readings.)*
  Files: `fe-database/src/handlers/iot_reading.rs` (new), `handlers/mod.rs`, `fe-database/src/AGENTS.md` (§iot-readings: D1/D2 rationale).
  `IotReadingInput { node_id, metric, value, units, recorded_at: Option<String> }`; `insert_readings(db, petal_id, source_did, &[IotReadingInput]) -> Result<usize, IotIngestError>` (thiserror: `UnknownAnchor`/`InvalidTimestamp`/`EmptyMetric`/`Db`). Validates **all** anchor nodes exist in the petal before inserting anything (all-or-nothing), parses/normalizes `recorded_at` (reject invalid RFC-3339), stamps HLC + ULID ids, inserts via `InsertBuilder` (legal — no geometry).
  Accept: compiles; no `.unwrap()` in prod path.
- [x] **Task 1.3 — READ-BACK tests for the write handler.** *(Done 2026-07-15: 7 tests in `fe-database/tests/iot_reading_test.rs` covering everything listed, incl. UTC-normalization round-trip and server-time-default bounds check.)*
  Files: `fe-database/tests/iot_reading_test.rs` (new).
  In-memory SurrealDB + `apply_all` + geometry-cast node seeding (export_share_test idiom). Tests: batch insert → SELECT back verifies every field (metric/value/units/petal_id/source_did/recorded_at round-trip, recorded_at_ms parsed, hlc monotonic); unknown anchor → `UnknownAnchor` + **zero rows persisted**; anchor in a different petal rejected; invalid `recorded_at` → `InvalidTimestamp`; empty batch → Ok(0); default server timestamp populated when `recorded_at` absent.

## Phase 2 — FR-3: time-series query support [fe-query]

- [x] **Task 2.1 — `select_value` + `timeseries` builder module.** *(Done 2026-07-15: `builder/timeseries.rs` with (a)–(d) exactly as specced (`READINGS_TABLE` const, correlated `$parent` latest form, half-open windows, `anchors_within` via new `QueryBuilder::select_value`); re-exported as `fe_query::timeseries`; render.rs untouched. AGENTS.md §timeseries records the no-arg-max-in-SurrealQL rationale.)*
  Files: `fe-query/src/builder/timeseries.rs` (new), `builder/mod.rs` (`select_value` on `QueryBuilder<Initial>` + `pub mod timeseries`), `lib.rs` re-exports, `fe-query/src/AGENTS.md` (§timeseries: correlated-`$parent` latest form + D5 anchor-join rationale).
  Builders (all return `BuiltQuery`, all parameterized):
  (a) `latest_per_anchor(petal_id: Option, metric: Option)` — correlated `$parent` max-timestamp form;
  (b) `window_aggregate(metric, start_ms, end_ms, petal_id: Option)` — `math::mean/min/max(value)` + `count()` `GROUP BY node_id` over a `recorded_at_ms` half-open window;
  (c) `anchors_within(x, z, radius_m) -> Filter` — `node_id IN (SELECT VALUE node_id FROM node WHERE geo::distance(position, $p) <= $p)` (D5);
  (d) `readings_in_window(...)` raw-rows window query accepting the spatial filter — combined spatial+temporal.
  Accept: no render.rs behavior change for existing builders.
- [x] **Task 2.2 — Rendered-SQL tests.** *(Done 2026-07-15: 7 exact-string tests in timeseries.rs (scoped/unscoped latest, window agg with/without petal, spatial filter alone, combined spatial+temporal incl. subquery param remap, temporal-only) + `select_value` test in builder/mod.rs.)*
  Files: `timeseries.rs` `#[cfg(test)]`, `builder/mod.rs` test for `select_value`.
  Exact-string asserts on rendered SQL + param names/values for (a)–(d), incl. combined spatial+temporal composition and param-remap ordering.

## Phase 3 — FR-4: ingestion endpoint + FR-5 seam [fe-api]

- [x] **Task 3.1 — Batch/rate constants.** *(Done 2026-07-15 as specced.)*
  Files: `fe-api/src/limits.rs`.
  `IOT_INGEST_MAX_READINGS = 1_000`, `IOT_INGEST_RATE_PER_SEC = 10` (D4).
- [x] **Task 3.2 — `POST /api/v1/petals/:petal_id/iot/readings`.** *(Done 2026-07-15: `fe-api/src/iot.rs` + route in server.rs authenticated router; typed `IotIngestError` → 422 (anchor/timestamp/metric), DB → 502; body reuses `fe_database::handlers::iot_reading::IotReadingInput` directly so the API and handler shapes cannot drift. AGENTS.md §iot-ingest.)*
  Files: `fe-api/src/iot.rs` (new), `lib.rs`, `server.rs` (authenticated route), `fe-api/src/AGENTS.md` (§iot-ingest).
  Body `{ readings: [{node_id, metric, value, units?, recorded_at?}] }`. Guard order (export.rs idiom): `require_role("editor")` → `is_valid_ulid(petal_id)` → `resolve_petal_scope` → `require_scope` → rate limit (`iot:<sub>` key) → batch cap (413) → non-empty (400) → `insert_readings` on `db_reader` (D1). Error mapping: `UnknownAnchor`/`InvalidTimestamp`/`EmptyMetric` → 400/422, DB → 502. Response `{ ok, accepted }`.
- [x] **Task 3.3 — Wire `iot_reading` into the egress guard (FR-5 seam).** *(Done 2026-07-15: `IOT_READING` whitelisted; `inject_scope_filter` now also injects on `FROM IOT_READING`; 2 new unit tests. Pre-existing quirk left untouched: `contains("FROM NODE")` also matches `FROM NODE_LOG` — noted, not this track's fix.)*
  Files: `fe-api/src/query_guard.rs`.
  Add `IOT_READING` to `ALLOWED_TABLES`; extend `inject_scope_filter` to inject the petal filter on `FROM iot_reading` (valid — rows carry `petal_id`, D3). Unit tests for both. This routes IoT rows through `/api/v1/query` + shared-URL redemption (JSON) — the landed egress surfaces.
- [x] **Task 3.4 — Ingestion integration tests (READ-BACK).** *(Done 2026-07-15: 4 tests in `fe-api/tests/iot_ingest_test.rs` covering all six listed scenarios; the FR-5 scope test drives `guard_and_prepare_query` + `run_guarded_query` end-to-end over real ingested rows.)*
  Files: `fe-api/tests/iot_ingest_test.rs` (new, export_share_test idiom).
  Happy-path batch → 200 + rows read back with all fields; wrong-scope token → 403 + zero rows; viewer role → 403; foreign-petal anchor → 4xx + zero rows; over batch cap → 413; guarded `/query`-path read of `iot_reading` is scope-filtered (petal-A token cannot see petal-B readings).

## Phase 4 — FR-5 remaining polish + sweep (NOT this session)

- [ ] **Task 4.1 — Reading-shaped parquet/CSV export.** `export.parquet`/`export.csv` are node-snapshot-shaped (`prepare_export` rejects non-`node` tables); a readings export needs a flat reading→row mapping (+ optional anchor-position join and `coords=latlon` via the petal `Projection`) before IoT rows flow to DuckDB/PowerBI **files** — until then `/query` JSON + shared URLs are the IoT egress. Coordinate with `analytics_egress_20260714` Phase-6 e2e.
- [ ] **Task 4.2 — FR-4 "external API" puller.** This track lands the minimal v1 ingestion shape (authenticated batch POST — webhook-able). A poll-based connector catalog is explicitly future work per spec.
- [ ] **Task 4.3 — Integrated sweep.** `cargo test -p fe-database -p fe-query -p fe-api` + clippy, once, at session end per test policy.

## Cross-track notes

- FR-2 is shared with `p2p_unblock_now_20260711` — landed there (9ef76a1); Phase 0 records it, nothing duplicated.
- `analytics_egress_20260714`: this track deliberately reuses its guard pipeline verbatim; any guard change here (Task 3.3) keeps its tests green.
- CRS seam: readings have no coordinates, so no CRS surface is added; anchor-join exports (Task 4.1) must reuse `fe-api/src/crs.rs` when they land.
