# fe-database/src — module notes

Design rationale for fe-database source modules. Code carries terse one-line
doc comments; the "why" lives here.

## §replication-backpressure

The DB→sync replication bridge is two `crossbeam::bounded(256)` hops:
`replicate_row_with_petal` (this crate) → `repl_rx` bridge thread
(`fractalengine/src/main.rs` Phase E) → sync thread. Both hops use
**`try_send` + drop-and-count**, never blocking `send` — a stalled sync
thread must degrade to observable replication lag, not a frozen DB handler
(track `p2p_unblock_now_20260711` FR-1; this had to land before real
iroh-docs replaces the instant mock, which masked the freeze).

- On `Full`: the event is dropped, `REPLICATION_DROPS` (process-global
  `AtomicU64`, read via `replication_drop_count()`) increments, and a
  `tracing::warn!` fires with the running total. The main.rs hop keeps a
  thread-local `u64` + warn log instead (single owner, log-based metric).
- On `Disconnected`: silent no-op — that's shutdown, not backpressure, so it
  is *not* counted as a drop.
- Dropped events are safe to lose today: replication is best-effort ahead of
  the real iroh-docs wiring; the durable row is already committed in
  SurrealDB before the bridge send.
- Regression guard: `tests/replication_backpressure_test.rs` fills a
  bounded(1) channel and asserts prompt return + counter increment. It's a
  single test fn because the counter is process-global (parallel test fns
  would race the before/after reads).
- Do NOT "fix" this back to blocking `send`, and match the same pattern for
  any new bridge hop (siblings: scene-change and transform bridges in
  main.rs use the identical try_send shape).

## §geometry-inserts

`node.position` (`geometry<point>`) and `petal.bounds` (`geometry<polygon>`)
live on SCHEMAFULL tables. Writes to them MUST use explicit SurrealQL casts
(`<geometry<point>> [$x, $z]`, `<geometry<polygon>> {...}`) in a hand-written
`CREATE ... CONTENT` statement — a plain GeoJSON object bound as a parameter
is rejected by the schema and the whole statement fails.

Do NOT route these inserts through `fe_query::InsertBuilder` + `exec_query`:
the generic builder renders `CREATE {table} CONTENT $p0` and has no way to
emit a cast. A 2026-05 refactor (commit `059f381`) did exactly that and every
node/petal creation silently failed for weeks — "silently" because
`exec_query` also didn't call `.check()`, so statement-level errors were
swallowed. Both defects are fixed: the three geometry insert paths in
`handlers/crud.rs` are back on cast queries with `.check()`, and
`query_helpers::exec_query` now checks every response. `InsertBuilder`
remains fine for non-geometry tables.

Regression guard: `db_test.rs` reads a freshly created node back from the DB
(`SELECT ... WHERE node_id`) instead of trusting handler `Ok` — handler
success must mean the row is actually persisted.

## §gis

Petal GIS reads (`petal_gis_endpoints_20260711` track) ride the existing
`DbCommand::RawQuery` channel — no new `DbCommand` variant, since the
dispatch `match` lives in this quarantined `lib.rs`. `fe_query::gis` (see
`fe-query/src/AGENTS.md` §gis for the builder contract) renders the SQL;
callers convert `BuiltQuery::params` into the `RawQuery::vars` HashMap and
send it through unchanged.

**RawQuery's own guard rail** (`lib.rs`, the `RawQuery` match arm) only
accepts a single `SELECT` statement with no `;` and rejects a fixed list of
bare-word keywords (`CREATE`, `UPDATE`, `DELETE`, `DEFINE`, `REMOVE`,
`RELATE`, `INSERT`, `LET`, `RETURN`, `INFO`, `FOR`, `THROW`, `SLEEP`,
`BREAK`, `LIVE`, `KILL`, `IF`, `BEGIN`, `COMMIT`, `CANCEL`). Every
`fe_query::gis` builder is a plain `SELECT ... WHERE ...` and is unit-tested
against this exact keyword list (`fe-query/src/gis.rs`
`all_builders_pass_rawquery_keyword_filter`) so a future edit can't silently
make GIS reads start failing at the RawQuery gate.

**Annotation reserved-key contract** (shared with `fe-api`'s GIS endpoints
and the query-UI track): three flat keys on `node.properties`, set via the
existing `SetNodeProperty` command exactly like any other custom property —
`gis.annotation.title`, `gis.annotation.body`, `gis.annotation.color` (hex
string, optional). These are NOT nested JSON (`properties.gis.annotation.title`)
— `set_entity_property_handler` writes `properties[$key] = $val` with the
dotted string used verbatim as a single flat object key, so
`fe_query::gis::annotated_nodes` reads them back the same way
(`properties["gis.annotation.title"]`, not path traversal).

**Local-coords convention**: `node.position` is `geometry<point>` in
petal-local meters (not lat/lon) — see `schema.rs`'s doc comment on the
`node` table. `fe_query::gis::nodes_in_bbox` / `nodes_within_radius` take
local meters directly; lat/lon ↔ local conversion is an API-layer concern
(`fe-api`) using the owning petal's terrain origin, never a fe-database or
fe-query concern. This keeps the data layer free of any CRS/projection
dependency.

**Why `properties ?? {}` before indexing**: `properties` is
`option<object>` and is `NONE` on any node that never had a property set
(the `CreateNode` handler doesn't populate it). `annotated_nodes` coalesces
with `?? {}` before the bracket lookup so the filter degrades to "absent" on
a `NONE` properties field instead of relying on undocumented NONE-propagation
behavior for indexing expressions — matching the same `?? {}` pattern
already used in `delete_entity_property_handler`.

## §rbac-policy (auth_policy_pattern_20260710)

`rbac.rs` no longer owns role semantics: the old `WRITE_ROLES` string list is
gone and `require_write_role` delegates to `fe-policy` (`RoleLevelPolicy::standard()`
via a shared `PolicyEngine`). Public fn signatures are unchanged so callers
(`space_manager`, `queries`) did not churn. The pure decision lives in
`evaluate_write(peer_did, role, scope)` so it is unit-testable without I/O —
the DB fetch of the role stays in `get_role`, *before* evaluation, per the
track's acceptance criteria. `role_level.rs` is now a re-export shim: the
canonical `RoleLevel` moved to `fe-policy` (see fe-policy/AGENTS.md §role-level)
to avoid a dependency cycle; `fe_database::RoleLevel` paths still resolve to
the same type.
