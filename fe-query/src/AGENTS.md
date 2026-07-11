# fe-query/src — module notes

Design rationale for fe-query source modules. Code carries terse one-line
doc comments; the "why" lives here.

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
