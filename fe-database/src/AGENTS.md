# fe-database/src — module notes

Design rationale for fe-database source modules. Code carries terse one-line
doc comments; the "why" lives here.

## §diagnostics

Two read-only examples inspect the live SurrealKV store (`FE_DB_PATH`, default
`data/fractalengine.db`): `examples/dump_db.rs` (raw row dump of
verse/petal/node/asset) and `examples/inspect_db.rs` (table discovery via
`INFO FOR DB`, per-table row counts, top petals by node count, node property
histograms, gpx_points sizes, node_log/asset breakdowns, disk size). Both open
the store normally but run only SELECT queries — safe against the user's live
DB, but they DO take the SurrealKV lock, so the app must not be running.

`examples/seed_join_verse.rs` (M1 user-testing validation fixture, 2026-10-07)
is the one WRITING example: `gen` (fresh verse ULID + namespace secret),
`seed` (verse row with the `derive_namespace_id` namespace_id +
optional Editor role row — the rows `create_verse_handler` writes, via the
same `Repo` helpers; REST-created verses get an in-process secret an operator
cannot extract, so a validator that owns both relays' environments seeds the
joinable verse directly), `readback` (whitelisted-table SELECT, one JSON line
per row). Same SurrealKV lock rule: host process must be stopped.

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

**Polygon casts need fully inline numeric literals (A6 finding,
2026-10-07).** `node.position` is forgiving: `SET position =
<geometry<point>> [$x, $z]` with bound scalars casts fine. `petal.bounds` is
not: this SurrealDB line rejects a cast around a *bound* GeoJSON object
(`<geometry<polygon>> $geo`) AND around an object literal with a bound rings
parameter (`{ type: 'Polygon', coordinates: $coords }`) — both fail the
schema check with "could not cast into `geometry`". Only the inline numeric
literal form (`{ type: 'Polygon', coordinates: [[[-10.0, -10.0], …]] }`,
the shape `crud.rs`/`seed.rs` use) casts. The replicated-petal path
(`handlers/replicated_row.rs::render_polygon_rings`) therefore extracts the
payload's rings and renders them inline — every token comes from a validated
finite f64, so there is no injection surface.

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

**`get_role` projects full rows (A3 fix, 2026-10-07).** It used to render
`SELECT role FROM role`, so any *existing* role row failed the typed-`Role`
deserialization ("missing field `peer_did`") — explicit roles never resolved;
only the owner (`created_by`) and `default_access` paths ever worked, and
`assign_role_checked` for a non-owner Manager would error. The A3 inbound
gate's editor/viewer tests surfaced it; the projection is now `SELECT *`.

## §iot-readings (iot_spatial_reporting_20260714)

`iot_reading` is the first real sensor-reading store (fe-terrain's "IoT"
modules are path-tracking/animation only). Design decisions:

- **No geometry on the reading row.** Position joins through the anchor
  `node_id` — spatial queries filter
  `node_id IN (SELECT VALUE node_id FROM node WHERE <spatial predicate>)`
  (see fe-query `builder/timeseries.rs`). This keeps IoT-frequency inserts
  on the cheap `InsertBuilder` path; §geometry-inserts does not apply.
- **`petal_id` is denormalized onto every row** so fe-api's
  `build_scope_filter`/`inject_scope_filter` scope guard applies to
  `FROM iot_reading` exactly as it does to `FROM node`.
- **Three timestamps.** `recorded_at` (RFC-3339 UTC, sensor time, humans),
  `recorded_at_ms` (epoch-ms i64 — the canonical range/window filter column,
  indexed via `idx_iot_reading_series`), `hlc_timestamp` (packed HLC,
  ingestion order / distributed merge, same convention as `node_log`).
- **Append-only, no derived counters.** Unlike `node_log.row_version`, no
  row derives state from prior rows, so `handlers::iot_reading::insert_readings`
  is safe to call from *any* runtime — including fe-api's multi-thread tokio
  via `db_reader` — without the DB-thread single-writer invariant. The batch
  is all-or-nothing at the validation level (every anchor checked against the
  petal, every timestamp parsed, before the first insert); a mid-batch DB
  error can still leave a partial batch (acceptable: rows are independent
  facts, and the caller gets an error to retry idempotently-enough).
- Errors are typed (`IotIngestError`, thiserror) so fe-api maps real HTTP
  statuses instead of string-sniffing.

**Emit seam (A11, F5).** `insert_readings` is now a thin delegate;
`insert_readings_with_replication(db, petal_id, verse_id, rows, blob_store,
repl_tx)` is the seam that also publishes replication. Order matters and is
deliberate: validate everything, insert durably, THEN emit — so a peer never
sees a row the local store cannot serve (the same durable-first rule as
§replication-backpressure). One `ReplicationEvent` per accepted row, carrying
the caller's `verse_id` (fe-api derives it from the resolved petal scope),
`petal_id`, `table: "iot_reading"`, `record_id: <reading_id>` and the row
bytes' content hash. Missing verse/blob-store/sender degrades to
durable-but-unpublished (warns once when a sender is wired but the context is
missing) — never to a failed ingest.

## §replication-mode (F5)

`replication_mode.rs::ReplicationMode` is the canonical per-table replication
shape: `ReplicationMode::for_table("iot_reading") == Timeseries`, every other
table is `Row`. It lives in this crate (not `fe-sync`) because the *inbound*
applier here must dispatch on it; `fe-sync` re-exports the enum for producers
so there is one definition, not two that can drift.

- `Row` — whole-row CRDT semantics: an incoming payload is merged into the
  local row by `merge.rs` (tombstone-honoring, stale-HLC rejecting).
- `Timeseries` — **union/append-only** semantics (A12). A reading is an
  immutable fact keyed by `reading_id`: the applier inserts it only when that
  key is absent, and NEVER overwrites an existing row, in either direction
  (a re-delivered row is idempotent; a "newer" payload for a known
  `reading_id` is discarded). No wall-clock ordering is involved — two peers
  that both hold a reading hold the same value, so any apply order converges
  to the same set.
- The wire **tombstone form** stays the empty `Doc::del` entry, but for a
  timeseries table it is `NotApplicable`: a row tombstone cannot retract a
  fact that the whole point of the union is to keep, so the local row (and
  its A3 denial path) is untouched. Timeseries retention/GC is a separate
  concern, deliberately not wired here.
- The A3 role gate (`admit_inbound_row`, Editor+) runs **before** mode
  dispatch, so union semantics never widen authority — a viewer-authored
  reading is denied exactly like a viewer-authored row.


## §session-cache (absorbed from fe-auth, 2026-07-17)

`session_cache.rs` is the former `fe-auth` crate's surviving surface, moved
here verbatim when fe-auth was retired (its verse-invite logic was already
superseded by `invite.rs`, and its handshake was a petal-only migration
bridge with no consumers). Rationale carried over from the deleted crate:

- `SESSION_TTL_SECS = 60` bounds how long a stale role can be served after a
  role change elsewhere; `get` treats expired entries as absent and
  `prune_expired` reclaims them.
- `revoke_session` enters `op_log::commit_operation` before evicting the
  cache entry, so a failed append leaves the cache unchanged. Its
  `sig: "00".repeat(64)` is one of the workspace's known
  placeholder op-log signatures (13 sites tracked by the hexon-p2p-commons
  research); broadcasting the revocation to peers
  (`NetworkCommand::BroadcastRevocation`) is still deferred (Sprint 5B).

## §reconcile

`reconcile.rs` is the sole startup data-fixing mechanism. The former
`migrations.rs` was removed because tracked-once migration semantics don't
work in P2P — peers can't coordinate "which migration ran". Reconciliation
rules are instead **idempotent invariants**: each describes what the DB
*should* look like, checks for violations, and fixes them; every rule is
safe to re-run on every startup, with no tracking table. This converges in
P2P because peers on different client versions all reach the same correct
state, new rules are additive (old clients ignore fields they don't know
about), and no coordination between peers is needed. `RULES` is an ordered,
append-only list.

## §hlc

`op_log.rs` replaced the former static `LAMPORT_CLOCK: AtomicU64` with a
Hybrid Logical Clock: wall-clock milliseconds in the upper 48 bits of a
packed `u64`, a monotonic counter in the lower 16. Guarantees:

- **Monotonicity** — timestamps never go backwards, even if the wall clock
  drifts or multiple events land in the same millisecond (the counter
  saturates into the next millisecond rather than wrapping past `0xFFFF`).
- **Restart safety** — `init_hlc()` takes the highest persisted
  `lamport_clock` and advances past it, so a restart never re-issues an old
  timestamp. The coordinator (`lib.rs`) must call it once during DB startup,
  after querying `SELECT math::max(lamport_clock) FROM op_log` and before
  any op-log write; `next_hlc_timestamp()` panics if it hasn't run.
- **Sortability** — the packed `u64` sorts identically to the
  `(wall_ms, counter)` pair, so SurrealDB `ORDER BY lamport_clock` remains
  correct.

`next_hlc_timestamp()` returns `(packed_u64, human_string)`; the human
string is `"<wall_ms>:<counter_hex>"`, stored in the `hlc_timestamp` column
for debugging / external tooling. HLC state sits in a `Mutex` purely for
`static` safety — the DB thread is single-threaded (current_thread tokio),
so contention is zero in practice.

## §log-first-commit

Every current op-log mutation enters `op_log::commit_operation`. The seam
assigns the local HLC, persists the op-log row, then invokes the local
materialization closure. An append error means the closure is never called;
callers return that error to their command path. A materialization error can
leave an already-appended operation without its local row update, which is a
recoverable pending replay until the verified materializer and its atomic
boundary arrive. `append_operation_log` is private so handlers cannot bypass
this order.

**PRECONDITION VALIDATION PRECEDES APPEND.** A caller must verify every cheap,
locally decidable precondition — including the target's existence and its
authorized scope — before it builds or appends an `OpLogEntry`. A normal
not-found or invalid-scope request therefore returns its original command
error and never creates a replay-poisoning log row. The materializer still
asserts its matched rows as the TOCTOU backstop: only a crash or a target that
vanishes after the precheck may return the appended-but-materialization-failed
outcome, which recovery handles by replay and discard.

Caller coverage is deliberate: node property and transform mutations preflight
their node; room/model creation resolves and authorizes the real petal scope
(and model placement requires its asset); SpaceManager metadata mutations bind
the target record to that same scope before append; and role assignment or
revocation verifies the entire verse/fractal/petal chain. Node hard-delete and
tombstone callers already resolve their targets before append; session-cache
revocation deliberately has no cache-entry-exists precondition because a cold
cache must still retain its durable revocation. Legacy Verse/Fractal/Petal
creation contracts remain the D-CL20 / SPEC-4 §10 deferral and MUST NOT be
used as canonical admission paths until their parent-scope and authority
contracts are specified.

Legacy `CreateNode` and hard `DeleteNode` now record `NodeCreated` and
`NodeDeleted` intentions through this seam as well. `DeleteNode` retains its
pre-track physical-row semantics, including its direct-GPX-waypoint cascade;
the newer tombstone commands remain the only sync-safe deletion path.

This is substrate hardening only. `OpLogEntry` still uses the legacy local
format and placeholder signatures; do not add canonical-envelope, signature,
or wire semantics here.

## §handlers

`handlers/` holds the command handlers extracted from the DB dispatch loop,
one sub-module per domain:

- `crud` — Verse/Fractal/Petal/Node creation, GLTF import, hierarchy loading
- `entity` — rename, delete, description updates
- `entity_property` — custom property CRUD for nodes
- `field_def` — field definition schema CRUD
- `transform` — node position/rotation/scale and URL persistence
- `preconditions` — shared cheap target/scope checks required before append
- `rbac` — role resolution, assignment, revocation
- `invite` — verse invite generation and join-by-invite
- `api_token` — API token minting, revocation, and listing
- `seed` — default data seeding
- `admin` — database reset
- `crate_registry` — hexon crate registry install/uninstall
- `petal_terrain` — per-petal terrain config get/set
- `iot_reading` — append-only IoT sensor-reading ingestion (§iot-readings)
- `ts_partial` — local partial execution for distributed timeseries queries
  (§ts-partials): renders the structured spec's SQL via the fe-query builders,
  never accepting SQL from the wire
- `node_log` — append-only per-node operation log (§node-log)
- `replicated_row` — inbound P2P row apply (A3+A4): `DbCommand::ApplyReplicatedRow`
  → **role-gated admission** (`admit_inbound_row`: the author's role resolved
  from the local tables at the verse scope via `role_manager::resolve_role`,
  decided by the same fe-policy standard write gate as the local path —
  deny-by-default, Editor+, never wire-supplied) → **per-table mode dispatch**
  (`dispatch_by_mode`: `ReplicationMode::Row` → whole-row merge; `Timeseries`
  → append-only union for `iot_reading`, §replication-mode) →
  geometry-safe, tombstone-honoring durable write (`merge.rs`) + petal-scoped
  `SceneChange`;
  loop-free by construction (no replication sender in scope). Two admission
  rules worth knowing: a **bootstrap window** admits rows for a verse this
  store does not know yet (replica capability = the admission; denying would
  deadlock the verse manifest's own convergence — A2 depends on it, and
  out-of-order sync would otherwise permanently lose interior rows), and a
  verse row whose payload `verse_id` ≠ the replica's verse is denied
  (scope-injection guard). The wire **tombstone form is the empty entry**
  (`Doc::del`): the handler synthesizes the node payload and resolves the
  scene change's petal from the durable row; an empty entry on a static
  hierarchy table is `NotApplicable` (no tombstone semantics there). A
  third rule (F20, 2026-10-07): an inbound **verse manifest's
  `default_access` must be `"viewer"` or `"none"`** — anything else is
  denied in `apply_static_row`. The schema column carries only a
  `DEFAULT 'viewer'` (no ASSERT), so without this a replicated manifest
  could set `"editor"` and make every unknown peer resolve to a writer for
  that verse, defeating A3 deny-by-default on the one path that writes it.

## §timeseries-settings (M2/F6)

`handlers/verse_settings.rs` owns the per-verse timeseries fabric settings
(the `ts_mode`/`ts_replication_factor`/`ts_bucket_width_ms` columns on
`verse`, non-optional with `DEFAULT 'mirror'`/`1`/`86400000` — a pre-F6 row
therefore parses as pure mirror, which IS the pre-F6 behavior).
`SetVerseTimeseriesSettings` sanitizes through
`fe_runtime::timeseries::VerseTimeseriesSettings::sanitized` (the canonical
enum lives in fe-runtime; see `fe-runtime/src/AGENTS.md` §timeseries),
persists `UPDATE verse SET ts_* = …`, and replies with the authoritative
READ-BACK echo `DbResult::VerseTimeseriesSettingsSet` carrying the sanitized
values — the same read-back shape as `PetalTerrainLoaded`, so the settings
surface's optimistic update is confirmed by fact, not trust.

**The handler also re-emits the verse manifest `ReplicationEvent`** carrying
the new `ts_*` fields. This is load-bearing, not redundant: the manifest doc
row IS the replication path for settings — peers' fabrics
(fe-sync `sharding.rs::VerseFabric::note_verse_row`) learn
mode/R/bucket-width only from the verse doc row, so without the re-emission a
settings change would never leave the local verse. The re-emitted row is read
back from the durable store after the UPDATE (never the request values), so
what crosses the wire is what the store actually holds. The convergence test
pins this: a settings change must produce a manifest event whose `ts_*`
fields match the persisted row.

`load_hierarchy` (crud.rs) surfaces the columns via
`VerseTimeseriesSettings::from_verse_row` — missing/invalid values fall back
to the mirror defaults, so hierarchy consumers never see a parse error from a
settings column.

## §ts-partials (M2/F7 — A15)

`handlers/ts_partial.rs::execute_ts_partial` runs THIS host's slice of a
distributed timeseries query on the DB thread (the single-writer connection —
the same class of read `RawQuery` performs; it is read-only, so it is safe on
the DB thread's runtime by construction). The input is the structured
`fe_runtime::distributed_query` spec: **SQL is rendered here via the fe-query
builders with bound parameters, never accepted from the wire**, so a peer
request can only ever run one of the sanctioned shapes restricted to a
petal/window/shard — the spec is the authorization surface, same as the
fe-api side.

Partial semantics are dictated by the merge contract (fe-sync
`distributed_query.rs`, the commutative merges):

- **`WindowAggregate` runs ONE query per requested shard** (anchor + range),
  clamping each shard's bucket range to the query window — per-shard
  attribution is what lets the merge prevent double-counting when multiple
  mirrors of a shard answer, while the sum/count/min/max monoid still adds
  up correctly across shards hosted by different peers. A shard whose
  clamped range is empty contributes an empty `ShardRows` (absence of data is
  not absence of the shard — the A16 covered/missing metadata depends on it).
- **`ReadingsInWindow` / `LatestPerAnchor` run ONE whole-window query**: their
  merges (union-dedupe-by-`reading_id`, max-by-timestamp) are exact under
  duplicate answers, so no shard attribution is needed.
- **`row_cap` is clamped** (`clamp_partial_row_cap`) — an executing host
  never trusts an unbounded request; the partial is truncated row-wise and
  flagged `truncated` rather than dropped.

## §node-log

The `node_log` table is append-only: each row is an immutable fact recording
a single mutation on a node, INSERT-only — no UPDATE or DELETE is ever
issued against it. `row_version` is monotonically increasing per `node_id`
and serves as hidden metadata for "most recent state" queries;
`hlc_timestamp` is the HLC-packed `u64` (§hlc) for time-series ordering and
distributed merge.

`handlers::node_log::append_node_log` derives `row_version` as
`SELECT math::max(row_version)` + INSERT, which is NOT atomic. It is safe
only because the DB thread runs on a single-threaded tokio runtime
(`current_thread`), so no two calls execute concurrently. **This invariant
must be maintained** — if the DB thread ever moves to a multi-threaded
runtime, replace the derivation with an atomic subquery or a DB-level
sequence.

## §repo

`Repo<T>` (`repo.rs`) provides typed CRUD over any `Table` implementor. All
methods are associated functions (no `&self`) — there is no instance state.
For domain-specific queries (spatial lookups, full-text search, SurrealQL
`VERSION`, etc.) use `Repo::query_raw` or write free functions that accept
`&Db` and return `T`.

Quick start:

```rust
// Insert
Repo::<Petal>::create(&db, &petal).await?;

// Lookup
let p = Repo::<Petal>::find_by_id(&db, "01HZ…").await?;

// Partial update
Repo::<Petal>::merge_by_id(&db, "01HZ…", json!({"name": "new"})).await?;
```

JSON `null` values are stripped before every insert (`create`/`create_raw`)
because SurrealDB's `option<T>` rejects explicit `null` — it expects the
field to be absent (interpreted as `NONE`).

## §schema-macro

`schema.rs` defines every SurrealDB table exactly once via `define_table!`,
which generates (1) a `pub struct` with `Debug, Clone, Serialize,
Deserialize` and (2) an `impl Table` (see `repo.rs`) providing `TABLE_NAME`,
`ID_FIELD`, `schema()` (the SurrealQL DDL), and `id_value()`. The generated
DDL uses `DEFINE TABLE/FIELD IF NOT EXISTS`, so it is fully idempotent and
safe to run on every startup.

Syntax:

```rust
define_table! {
    /// Doc comment on the struct.
    table "surreal_table_name" => RustStructName (id: id_field_name) {
        field_a: String         => "TYPE string",
        field_b: Option<String> => "TYPE option<string>",
    }
}
```

The right-hand side of `=>` for each field is the SurrealQL type clause
(everything after `ON TABLE <name>`); it can include `ASSERT`, `VALUE`,
`DEFAULT`, and `FLEXIBLE` modifiers.

## §scope

Scope strings are `VERSE#<v>[-FRACTAL#<f>[-PETAL#<p>]]` (`scope.rs`).
Access resolution is hierarchical: a scope at a higher level covers every
resource beneath it —

- `VERSE#v1` covers `VERSE#v1-FRACTAL#f1-PETAL#p1`
- `VERSE#v1-FRACTAL#f1` covers `VERSE#v1-FRACTAL#f1-PETAL#p1`
- `VERSE#v1-FRACTAL#f1-PETAL#p1` does NOT cover `VERSE#v1-FRACTAL#f1`

`scope_contains` implements this as "token scope equals, or is a proper
prefix of, the resource scope ending at a `-` keyword boundary" — the
boundary check stops `VERSE#v1` from covering `VERSE#v10`. IDs may
themselves contain `-`, so `parse_scope` splits on the literal markers
`-FRACTAL#` / `-PETAL#`, never on bare `-`.

## §lifecycle (node_lifecycle_addressing_20260725 FR-1/2/5/6)

The **durable path is the source of truth**; the in-memory `fe_entity_store::EntityStore`
mirror is driven by the `SceneChange` / `LifecycleEvent` seams the DB thread
emits, not a parallel world.

**Tombstone = soft delete, never a raw row drop (FR-1 / N-4).**
`tombstone_node_handler` (crud.rs) stamps a durable `tombstone` object
(`{ hlc, source_did, tombstoned_at }`) on the `node` row — the row *persists* so
the delete survives reload and P2P merge — and records a `NodeTombstoned` op-log
entry. Every scene read filters `WHERE tombstone = NONE` (`load_hierarchy`,
`load_nodes_by_petal`, `get_node_transform`), so a tombstoned node is invisible
without being physically gone. The node's direct gpx waypoints
(`properties.gpx_track_id`) are tombstoned in the same atomic statement
(inverse-orphan protection). The legacy `delete_node_handler` / `DeleteNode`
retains a hard row drop — it is the pre-track hard-delete op, superseded by the
tombstone for anything sync-safe.

**Cascade = one materialized subtree update (FR-2).**
`cascade_tombstone_node_handler` BFS-collects the N-level descendant subtree
over the durable parent edges (`properties.parent_id` / `gpx_track_id` /
`owning_path_id`, de-duplicated, depth-capped), appends its operation through
§log-first-commit, then stamps the whole set in one UPDATE statement. The
operation append and row update are not yet one transaction: a failed update
leaves a replayable pending operation rather than unlogged state.
The dispatcher emits one petal-scoped `SceneChange::NodeRemoved` per id in the
materialized subtree, so `EntityStore` and WebSocket mirrors discard every
affected descendant rather than only the cascade root.
An already-tombstoned root is an idempotent no-op: the handler returns an empty
affected set before `commit_operation`, so it appends no second operation,
updates no rows, and emits no duplicate scene or lifecycle event.

**Merge non-resurrection (`merge.rs`, N-4).** `apply_replicated_node` is the
durable counterpart of `EntityStore::upsert`'s tombstone guard: an incoming
*live* row for a locally-tombstoned node is skipped (`SkippedTombstoned`), an
incoming tombstone converges the local row to deleted. fe-sync's `reconcile_petal`
drives this for inbound peer rows.

**Authorization (N-5).** For each lifecycle command the dispatch loop resolves
the node/petal scope *first*, then builds the caller's `fe_policy::AuthContext`
(`lifecycle_auth_context`), then calls `authorize_node_delete` /
`authorize_instance_promotion` before any mutation — sub-Editor callers get a
`DbResult::Error`, no row touched. `CallerAuth::Identified`/`Anonymous` carry an
already-resolved role and map synchronously (`caller_auth_to_context`).
`CallerAuth::Local` is what the local desktop UI sends for its *own* commands: it
asserts NO role, so the DB thread resolves the local user's real role at the
target scope via `resolve_local_role_handler` (the UI never self-authorizes). The
recorded actor DID is the resolved subject (`AuthContext::subject_label`), so a
`Local` tombstone is attributed to the local DID, not a UI-supplied string.

**Lifecycle events (FR-6).** With a lifecycle sender wired
(`spawn_db_thread_with_sync_and_lifecycle`), each op emits exactly one
`LifecycleEvent` carrying the stable `fe://` address (`lifecycle_uri`, a local
mirror of `NodeAddress::to_uri` kept in lock-step to avoid a new crate dep):
create → `NodeCreated`, promote → `NodePromoted`, tombstone → `NodeDeleted`
(+ `PathReflow` for a stamp with an owning path), rename → `NodeRenamed`,
duplicate → `NodeCreated` (a copy is an ordinary new node to every subscriber).
`PathReflow.deleted_index` carries the tombstoned stamp's
`properties.instance_index` (read in `read_node_lifecycle_meta`, threaded via
`TombstoneDurableOutcome.deleted_instance_index`) so T2's re-flow knows which
slot vanished; the field is `Option` + serde-defaulted so pre-field payloads
still parse. HLC init (`op_log::init_hlc`) is a precondition for all op-log
writes, including legacy node creation and hard deletion.

**Scene-change attribution.** Every node-scoped `SceneChange` emitted by the
DB dispatcher carries an owning `petal_id`. Delete and tombstone paths use
their handler outcome; transform, property, and node-rename paths resolve the
petal after the durable write. A failed or absent lookup suppresses that scene
event rather than broadcasting an unscoped delta. Generic `RenameEntity` has
no node variant and emits no node scene change.

**Rename / duplicate / descendant count (spatial-builder Step 1).**
`RenameNode` / `DuplicateNode` authorize through the shared
`fe_policy::authorize_node_edit` (Editor+, same gate as delete/promote — one
threshold, no per-verb drift); both use the tombstone-aware scope-resolve →
role-resolve → authorize → mutate order above, and a missing or tombstoned
target is a `DbResult::Error`, never a silent success. `rename_node_handler`
mirrors `rename_entity_handler` against `node.display_name`.
`duplicate_node_handler` copies the full row under a fresh ulid with
`"{source} (copy)"` and a +1.0 m x/z offset (raw petal-local meters, N-1), then
strips the path-binding identity keys (`strip_path_identity_properties`):
`owning_path_id`, `path_id`, `instance_index`, `stamp.override.arc_m`
(curve-domain), and `node_kind` only when it is `"stamp"` — a copy is not
path-bound so it must not claim a stamp identity, but other kinds
(`earthwork_region`) and visual overrides (`stamp.override.scale` etc.) carry
over. The dispatch arm replays the CreateNode side-effect set so the copy needs
zero UI changes. `CountNodeDescendants` is read-only (no auth, like
`GetNodeProperties`) and shares `collect_live_subtree` with the cascade BFS so
the confirm-dialog count and the actual cascade can never drift; the root is
excluded ("its N child nodes") and tombstoned rows don't count.

**Promotion write-side contract.** `promote_instance_handler` writes
`properties: { node_kind: 'stamp', path_id, owning_path_id, instance_index }` —
`node_kind`/`path_id`/`instance_index` are fe-query's read contract
(fe-query/src/spatial_nodes.rs `KIND_KEY`/`STAMP_PATH_ID_KEY`/
`STAMP_INSTANCE_INDEX_KEY`); `owning_path_id` stays because the cascade BFS and
the reflow hook traverse it as the durable parent edge.
