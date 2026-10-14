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
tombstone-filtered (`tombstone = NONE`) and petal-confined
(`petal_id = $petal_id` — `stamps_on_path_sql(petal_id, path_id)` took only
the path until 2026-10-10, so a caller authorized for one petal could list
another verse's stamps by naming a foreign path). Volumes are already real m³ (computed
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
(the commit `fb3c2f0` defect family). This CRS mismatch is also why the
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
- **CRS honesty (FR-5 seam) + DEC-C16 spec compliance:** the `geo` metadata
  `crs` key is always spec-legal `null` (petal-local frames have no PROJJSON
  CRS; `null` means "unspecified", never a silent EPSG:4326 claim). The
  descriptive *string* — default `"PETAL-LOCAL:meters;origin=unset"` — moved
  to the custom `fe:crs` key in the same `columns.<geometry>` object
  (spec-tolerated custom keys). The API layer (which owns the petal terrain
  origin) still overrides that string via `write_nodes_parquet_with_meta`,
  either stamping the real origin string or converting to lat/lon and only
  then labeling EPSG:4326 (track Phase 5). This closed the stock-DuckDB
  interop gap (`enable_geoparquet_conversion` no longer needs to be disabled).
  **M4 minor #7 (2026-10-09):** `GeoParquetMeta.lonlat_crs84 = true` (set by
  fe-api for `coords=latlon` output) OMITS the `crs` key — GeoParquet 1.0
  defines an absent `crs` as OGC:CRS84 (lon/lat), exactly the `[lon, lat,
  ele]` order fe-api writes. Petal-local output keeps an explicit `crs: null`
  (null ≠ absent: null = "unspecified").

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
- **Distributed partials (M2/F7)**: `window_aggregate_partial` is the
  per-shard aggregate for the fan-out's `WindowAggregate` spec — one
  (anchor, clamped-range) query per shard, returning raw per-(anchor,
  window) rows carrying the aggregate inputs (`sum`/`count`/`min`/`max` as
  columns), NOT the final mean: the merge on the collector side folds
  count/sum/min/max and derives mean only at the end, so multiple mirrors of
  a shard and multi-host shards merge exactly (see fe-sync
  `distributed_query.rs`). `readings_in_window`/`readings_for_petal` are
  the raw partials (union-dedupe by `reading_id` upstream, so no shard
  attribution needed). The executing host renders this SQL from the
  structured spec with bound parameters (fe-database `handlers/ts_partial.rs`)
  — remote SQL is never accepted from the wire.

## §observation-hexon (`columnar/observation/`, SPEC-10 B2, critic tick 5 AC-25 / tick 7 AC-36a/C21)

Writer/reader for the columnar observation hexon — `docs/spec/canonical-log/observation-plane.md`
§3 is the normative source; this section is the implementation record. Feature-gated behind
`parquet` (same gate as §geoparquet); its own feature-matrix gate is
`cargo check -p fe-query --features parquet --tests` + the matching clippy (AC-19 lesson: a
feature-gated path that never gets its own build silently bitrots).

- **Never touches disk.** The hexon is held decrypted in memory start to finish — `writer.rs`
  returns `Vec<u8>` buffers, `reader.rs` reads through `fe_format::HexonArchive::import`/
  `read_member` (in-memory `Bytes`), never a temp file. `HexonArchiveIndex` (the on-disk,
  `LazyFile`-backed reader) stays the right tool for a *file-backed* hexon such as a terrain
  tileset; it is the wrong one here (critic tick 7 AC-36b/C21).
- **One packer, two readers, no third.** `fe_format::HexonArchive::export_stored_deterministic`
  (fe-format AGENTS.md §archive) is the ONLY zip writer this module uses — it canonical-JSON-encodes
  `manifest.json`, writes the fixed empty `entries.json`, then the caller's members, all `Stored`
  with the MS-DOS epoch timestamp, so archive bytes are a pure function of member bytes. Reading
  goes through `HexonArchive::import` (manifest + entries + integrity) plus its companions
  `member_order`/`read_member` (member name/order/storage-method check and named-member extraction
  under the same decompression-limit guard `import` uses) — not a hand-rolled zip parse.
- **`profile.rs`** pins every SPEC-10 §3.2 rule 1 writer-property constant (`CREATED_BY =
  "fe-observation-hexon-v1"`, the one `fe.observation.hexon_format_version` key-value entry,
  `MAX_ROW_GROUP_ROWS = 50_000`, `DATA_PAGE_SIZE_LIMIT = 1 MiB`, writer version `PARQUET_1_0`,
  `Compression::UNCOMPRESSED`, dictionary encoding off, `EnabledStatistics::Chunk`) plus the exact
  Arrow schemas for `readings.parquet` (12 columns), `payload_refs.parquet` (11 columns, 2 of them
  `List<Utf8>`/`List<FixedSizeBinary(32)>`), and the statistics artifact (§3.3, 12 columns). A change
  to any pinned value is a new `hexon_format_version`, never a silent edit.
- **Determinism (§3.2 rule 8).** The writer sorts every row by `(anchor, hlc_wall_ms, hlc_counter,
  id)` itself before writing — callers may hand rows in any order. `parquet` is built
  `default-features = false` (`fe-query/Cargo.toml`): no compression codec is compiled in at all,
  so "pages are uncompressed" (D-CL32) cannot regress by a future default change, only by a
  deliberate Cargo edit (flagged as a determinism hazard in the campaign plan's own risk register).
- **`segmentby`/`orderby` (§3.2 rules 5-6).** The writer groups already-sorted rows into consecutive
  same-anchor runs and calls `ArrowWriter::write` + `.flush()` once per run — `write` itself further
  splits a run at `MAX_ROW_GROUP_ROWS` (parquet-rs's own row-group-size behavior), so one flush per
  anchor is sufficient to guarantee "a row group never mixes anchors" without any bespoke row-group
  bookkeeping. The reader re-derives the same guarantee by rebuilding one `ParquetRecordBatchReaderBuilder`
  per row group (`.with_row_groups(vec![rg])`) — re-parsing the small footer per group is an accepted
  simplicity/correctness trade (same precedent as fe-format's own documented sealed-member
  double-decompress, `fe-format/src/AGENTS.md` §archive-index), not a hot path.
- **`anchor` column = `fe_sdk::legacy_identifier(LegacyTable::Node, node_id)`** (SPEC-10 §4.5; the
  ONE mapping, `fe-sdk/src/AGENTS.md` §legacy-identifier) — computed once per distinct `node_id`,
  never reimplemented here. A non-ASCII node id surfaces as `ObservationHexonError::LegacyIdentifier`.
- **`source_index`** on every row is the caller's (B3's) index into the kind-5 payload's `sources`
  list (§4.3 key 8); this module only range-checks it against the caller-supplied `source_count`
  and carries it opaquely — it has no `ObservationSource` type of its own and no
  fe-canonical-log dependency.
- **Statistics artifact (§3.3)** groups rows by `(anchor, table, key)` in a `BTreeMap`, which yields
  the required "strictly ascending by that triple" order for free (no separate sort step); `table`
  discriminants are `STATS_TABLE_READING = 1` / `STATS_TABLE_PAYLOAD = 2`. Per rule 2's literal
  wording, `min_hlc_wall_ms`/`min_hlc_counter` (and the `max_*` pair) are independent per-column
  aggregates over the group's rows, NOT a single row's paired HLC value — unlike the overall
  `hlc_range` summary field, which IS the true lexicographic `(wall_ms, counter)` extremum (the
  two are intentionally different reductions; do not unify them).
- **No chrono/time dependency.** `writer.rs::epoch_ms_to_rfc3339` is a ~20-line, dependency-free
  epoch-ms → RFC 3339 formatter (Howard Hinnant's `civil_from_days` algorithm) used only for the
  manifest's `created_at`/`updated_at` (a pure function of the batch's max ingestion HLC wall time,
  per §3.1 rule 4). Adding chrono/time for one field was judged not worth a new dependency edge.
- **Error type.** `ObservationHexonError` distinguishes a caller-input defect (`EmptyBatch`,
  `SourceIndexOutOfRange`, `LegacyIdentifier`) from a profile violation found in bytes being read
  back (`ProfileViolation`, with a message naming the rule) — B3 can branch on the variant instead
  of matching error-message substrings.
- **New dependency edges, all gated by the `parquet` feature (`fe-query/Cargo.toml`):**
  `fe-sdk` (critic tick 5 AC-25/§8 C15 — `legacy_identifier`), `fe-format` with
  `default-features = false` (critic tick 7 AC-36a/C21 — the `runtime` feature would pull
  fe-runtime → bevy, which this crate must not gain), `bytes` (already pinned at the workspace
  level — the only new edge; needed for `parquet::file::reader::ChunkReader` over an in-memory
  buffer), and `blake3` (already pinned at the workspace level — `hexon_id` per §3.1 rule 4).
- **Deferred to B3 (the sealer/hydrator, not owned here):** sealing the plaintext hexon + statistics
  artifact into `LaneClass::BulkArtifact` bulk artifacts, building the kind-5 CBOR payload from
  `ObservationHexonSummary`, window-membership selection, and the §5.2 hydration cross-check between
  a hexon's actual contents and its signed payload (this module only validates the hexon against
  *itself* — the container and Parquet profile — never against a payload it has no access to).
