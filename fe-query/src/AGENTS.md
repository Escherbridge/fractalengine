# fe-query/src — module notes

Design rationale for fe-query source modules. Code carries terse one-line
doc comments; the "why" lives here.

## §spatial-nodes (`spatial_nodes.rs`, track `endpoint_api_surface_20260725`, T5 FR-6)

Generic-node querying by **type tag**, so promoted stamps (T2) and earthwork
regions (T3) are served through one abstraction without depending on their
producers. The tag lives in `node.properties.node_kind` (`stamp` /
`earthwork_region`; absent = plain node); `NodeKind::{as_tag, from_tag}` is the
single vocabulary source. Type-specific property keys (`path_id`,
`instance_index`, `cut_volume_m3`, `fill_volume_m3`, `material`) form the JSON
contract T2/T3 write and T5 reads — the deliberate seam that keeps this crate
free of any fe-terrain/fe-renderer coupling. Builders (`nodes_of_kind_sql`,
`stamps_on_path_sql`, `earthwork_volume_sql`) return a `SpatialNodeQuery`
(`(sql, binds)`): the SurrealQL text carries `$petal_id`/`$path_id` placeholders
and the id values ride in `binds` — ids are ALWAYS bound, never interpolated, so
they are not a SQL-injection surface (tag values stay inline as compile-time
constants). Callers bind exactly like `fe-api::endpoint::load_node`. All are
tombstone-filtered (`tombstone = NONE`). Volumes are already real m³ (computed
through the terrain scale authority upstream, N-1) — this crate only sums them.

## §gis

`gis.rs` provides petal-scoped SurrealQL builders for the
`petal_gis_endpoints_20260711` track: `nodes_in_bbox`, `nodes_within_radius`,
`annotated_nodes`. They return the same `BuiltQuery { sql, params }` shape as
`builder::render`, so callers bind exactly like any other fe-query result —
convert `params: Vec<(String, Value)>` into a `HashMap<String, Value>` and
hand it to `DbCommand::RawQuery { sql, vars }`.

### Why NOT `geo::*` (and therefore not `Filter::within`/`d_within`)

SurrealDB's `geo::distance` is geodesic (haversine) and `geo::inside`
likewise interprets coordinates as **lon/lat degrees**. `node.position`
stores **petal-local meters** in `[x, z]` — a node 500 m east of origin
would read as longitude 500°, so `geo::*` results on this column are
semantically meaningless (5 local meters ≈ 555 km haversine). The spatial
builders therefore filter with plain arithmetic on the point's raw
components: `position.coordinates[0]`/`[1]` compared against bound bbox
edges, and squared Euclidean distance against a bound `$r2 = radius²` (also
avoids any sqrt/`math::` function dependency). Do not reintroduce `geo::*`
for `node.position`; geodesic queries belong at the API layer, after lat/lon
→ local conversion via the petal's terrain origin.

`Filter::within`/`Filter::d_within` (which render `geo::*` with a
whole-GeoJSON bind parameter) are left in place for genuinely-geographic
columns should one ever exist, but they are wrong for `node.position` for
both reasons above — the CRS mismatch, and the opaque-GeoJSON-parameter
pattern that `fe-database/src/AGENTS.md` §geometry-inserts warns against
(the commit `059f381` defect family). This CRS mismatch is also why the
builders are hand-written strings returning `BuiltQuery` rather than
`QueryBuilder` compositions: the fluent `Filter` API has no way to express
component-wise arithmetic on a geometry column.

### Annotation reserved-key contract

Shared with `fe-database/src/AGENTS.md` §gis and the `fe-api` GIS endpoints
and query-UI tracks: `gis.annotation.title`, `gis.annotation.body`,
`gis.annotation.color` (hex string, optional) are flat keys on
`node.properties`, not a nested `gis: { annotation: { title: ... } } }`
object — see `ANNOTATION_TITLE_KEY` / `ANNOTATION_BODY_KEY` /
`ANNOTATION_COLOR_KEY` constants in `gis.rs`.

### Local-coords convention

All three builders take **petal-local meters**, matching how
`node.position` is actually stored (see `fe-database/src/schema.rs`'s `node`
table doc comment — X/Z are local Cartesian, not lon/lat). Lat/lon ↔ local
conversion is deliberately kept out of fe-query and fe-database entirely —
it lives API-side (`fe-api`), using the owning petal's terrain origin,
because that's the only layer that knows a petal's real-world anchor point.
Pushing CRS/projection logic down into the data layer would couple
fe-query/fe-database to a decision (which terrain origin) that can change
per-petal and has no business being a query-builder concern.

### Parameter safety / trade-off

`DbCommand::RawQuery` (`fe-database/src/lib.rs`) supports genuine bind
parameters via its `vars: HashMap<String, Value>` field — every builder in
`gis.rs` uses bind parameters exclusively; no user-supplied value (petal_id,
coordinates, radius) is ever formatted into the SQL text. `petal_id` is
additionally validated against a conservative alnum/`-`/`_` charset before
rendering (defense in depth for any future caller that might embed it
outside a bind position — none currently do) and all f64 inputs are checked
`is_finite()` before binding, since `serde_json::Value::from(f64)` silently
maps NaN/Infinity to `null` rather than erroring, which would otherwise turn
a malformed input into a silently-wrong query instead of a build-time error.

RawQuery's own keyword/statement guard (single `SELECT`, no `;`, no bare
CREATE/UPDATE/DELETE/DEFINE/etc.) is documented and regression-tested next to
the builders themselves — see `fe-database/src/AGENTS.md` §gis.

## §geoparquet

`columnar/geoparquet/` writes/reads `EntitySnapshot` rows as GeoParquet 1.0
(`analytics_egress_20260714` FR-1). `mod.rs` owns the public API + `geo`
footer metadata; `codec.rs` owns the snapshot↔Arrow/WKB mapping.

- **Plain `parquet`/`arrow` 54.x, not geoarrow-rs**: 54.x matches
  datafusion 46's arrow line (single arrow in the tree when both features are
  on); geoarrow-rs's writer surface was still churning at evaluation time
  (2026-07-14) and its dep tree is far larger, while a Point-Z-only ISO WKB
  codec is ~40 lines. The crates are declared in fe-query only — the workspace
  root doesn't carry them; hoist to `[workspace.dependencies]` when a second
  crate needs them.
- The `parquet` cargo feature is deliberately lighter than `datafusion` so
  fe-api can export parquet without dragging DataFusion in; the `datafusion`
  feature implies `parquet` (columnar/ used to be all-or-nothing on
  datafusion; only `geoparquet` is available under `parquet` alone).
- Column shape: `position` (**petal-local meters**, §local-coords) is the
  geometry column as ISO WKB Point Z (code 1001, little-endian) in a Binary
  column; rotation/scale are flattened to six Float32 scalar columns
  (BI/DuckDB-friendly); `properties` is a nullable JSON-string Utf8 column;
  `node_log` is intentionally NOT exported (audit log ≠ analytics egress)
  and reads back empty.
- **CRS honesty (FR-5 seam):** the `geo` metadata `crs` field carries a
  descriptive *string* — default `"PETAL-LOCAL:meters;origin=unset"` — never
  a silent EPSG:4326 claim, because coordinates are petal-local meters. The
  API layer (which owns the petal terrain origin) overrides it via
  `write_nodes_parquet_with_meta`, either stamping the real origin string or
  converting to lat/lon and only then labeling EPSG:4326 (track Phase 5). A
  string in `crs` deviates from strict GeoParquet-1.0 PROJJSON; that is a
  deliberate trade — honest-but-nonstandard beats standard-but-wrong.

## §timeseries (iot_spatial_reporting_20260714)

`builder/timeseries.rs` shapes the IoT reporting queries over `iot_reading`
(table defined in fe-database `schema.rs`; rows carry NO geometry):

- **Latest-per-anchor uses a correlated `$parent` max-subquery** rather than
  GROUP BY, because SurrealQL has no ordered arg-max aggregate — GROUP BY can
  give the newest timestamp per anchor but not the value that goes with it.
  The correlated form re-scans per row; acceptable at reporting cardinalities
  and backed by the `idx_iot_reading_series (node_id, metric, recorded_at_ms)`
  index.
- **Windows are half-open `[start_ms, end_ms)`** on `recorded_at_ms` (epoch
  ms, the canonical query timestamp) so adjacent windows tile without
  double-counting.
- **Spatial predicates join through the anchor node**: `anchors_within`
  renders `node_id IN (SELECT VALUE node_id FROM node WHERE geo::distance(...))`
  via the new `QueryBuilder::select_value` projection. Keeping geometry off
  the reading row keeps IoT-frequency inserts on the cheap non-geometry path
  and preserves the single source of truth for position (local-meters
  convention unchanged — see §local-coords).
