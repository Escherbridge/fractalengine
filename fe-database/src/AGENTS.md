# fe-database/src — module notes

Design rationale for fe-database source modules. Code carries terse one-line
doc comments; the "why" lives here.

## §qa-input-lease

`ImportGltf.file_lease` is optional operation-owned file/ancestry protection for isolated QA imports.
The database command arm retains it through `import_gltf_handler(...).await` and drops it after that read/write
operation returns. Normal callers pass `None`; authorization and imported asset persistence are unchanged.

## §diagnostics

Two read-only examples inspect the live SurrealKV store (`FE_DB_PATH`, default
`data/fractalengine.db`): `examples/dump_db.rs` (raw row dump of
verse/petal/node/asset incl. `content_hash`, plus a listing of the legacy
`imported/` backfill source — §asset-resolution) and `examples/inspect_db.rs` (table discovery via
`INFO FOR DB`, per-table row counts, top petals by node count, node property
histograms, gpx_points sizes, node_log/asset breakdowns, disk size). Both open
the store normally but run only SELECT queries — safe against the user's live
DB, but they DO take the SurrealKV lock, so the app must not be running.

`examples/seed_join_verse.rs` (M1 user-testing validation fixture, 2026-10-07)
is the one WRITING example: `gen` (fresh verse ULID + namespace secret),
`seed` (verse row with the `derive_namespace_id` namespace_id — the row
`create_verse_handler` writes, via the same `Repo` helpers; REST-created verses
get an in-process secret an operator cannot extract, so a validator that owns
both relays' environments seeds the joinable verse directly), `readback`
(whitelisted-table SELECT, one JSON line per row). Same SurrealKV lock rule:
host process must be stopped. Its old `--role-peer-did/--role-scope/
--role-level` flags (they wrote an arbitrary-level `role` row for a remote
peer — not an owner row, as the role_propagation plan first cited) are
**retired** and refuse loudly: `role` is a materialized cache rebuilt from
statements at startup (§role-propagation), so a seeded row would vanish.

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
emit a cast. A 2026-05 refactor (commit `fb3c2f0`) did exactly that and every
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

**`role` is a materialized cache (role_propagation RP-2).** Its rows are the
raise-only projection of the per-grantor role statements (fe-policy
`EffectiveRoles::cache_rows`, §role-propagation), written only by
`role_grants::repartition`/`rematerialize`. `resolve_role`/`get_role` are
unchanged: walking to the most specific cache row returns exactly
fe-policy's `role_at`, so the hot path never learns about statements.
`rbac::assign_role`/`revoke_role` survive only as raw cache writes for test
fixtures. Manager+ gates go through `rbac::require_manage_role`
(`resolve_role` + fe-policy `Action::Manage`), which over the cache IS
subtree authority.

## §iot-readings (iot_spatial_reporting_20260714)

`iot_reading` is the first real sensor-reading store (fe-terrain's "IoT"
modules are path-tracking/animation only). Design decisions:

- **No geometry on the reading row.** Position joins through the anchor
  `node_id` — spatial queries filter
  `node_id IN (SELECT VALUE node_id FROM node WHERE <spatial predicate>)`
  (see fe-query `builder/timeseries.rs`). This keeps IoT-frequency inserts
  on the cheap `InsertBuilder` path; §geometry-inserts does not apply.
- **`petal_id` is denormalized onto every row** so fe-api's
  scope guard (`query_guard::prepare_scoped_sql` FROM-substitution, M4 fix
  B1) applies to `FROM iot_reading` exactly as it does to `FROM node`.
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
`insert_readings_with_replication(db, petal_id, verse_id, source_did,
source_class, readings, blob_store, repl_tx)` is the seam that also publishes
replication (`source_did` is the ingesting caller's DID, threaded onto each
reading row — added after the original F5 signature, re-verified F23
2026-10-08; `source_class` added CDL-SPEC10 A3, see below). Order matters and
is deliberate: validate everything, insert durably, THEN emit — so a peer
never sees a row the local store cannot serve (the same durable-first rule as
§replication-backpressure). One `ReplicationEvent` per accepted row, carrying
the caller's `verse_id` (fe-api derives it from the resolved petal scope),
`petal_id`, `table: "iot_reading"`, `record_id: <reading_id>` and the row
bytes' content hash. Missing verse/blob-store/sender degrades to
durable-but-unpublished (warns once when a sender is wired but the context is
missing) — never to a failed ingest.

**R5 `payload_id` link + `source_class` (CDL-SPEC10 A3).** `iot_reading.payload_id`
(additive, `option<string>`) is "decoded from / sent with this payload". An
unlinked reading OMITS the field (`repo::strip_json_nulls` on the door row):
SurrealDB's `option<T>` rejects an explicit JSON null, so a `"payload_id":
null` row fails its insert — the same rule covers `device_payload.verse_id`
and `device_format.schema`. A
`Some` value must name an existing `device_payload` row or the whole batch is
rejected (`IotIngestError::UnknownPayload`, same all-or-nothing validation
pass as the anchor check). `source_class` is NOT a column on `iot_reading`
itself: it is the caller-declared `fe_sdk::source_class::SourceClass` (SPEC-10
§7.1 rule 6 — never inferred from `source_did`) that the door threads into
§window-membership below, so B3's future sealer can build the G6
`ObservationSource` set without re-deriving it. `insert_readings`'s shim
signature is unchanged (it always passes `verse_id: None`, so window
membership — gated on a known verse — never fires on that path; its fixed
`SourceClass::Device` default is therefore never actually read).

**R24 extension provenance (extension_platform_20261107 E10W, OD-E1 stamp-and-stop).**
`insert_readings_with_provenance` is the door body; `insert_readings_with_replication`
delegates to it with `None`. The PluginCommit flush passes the admission's host-minted
`fe_runtime::plugin_host::PackageIdentity`: `handlers/plugin_host.rs::flush` calls
`insert_readings_with_provenance`, and the scene journal fact carries `payload.ext`. E4 closed
G2-C4, and since I-DB the DB arm calls `flush` (§plugin-host-flush). So every reading an extension
writes carries `ext_package_id` + `ext_content_hash` (additive `option<string>`
columns, omitted when unstamped: the `payload_id` precedent). The guest never names
either value. The pair replicates with the row; `replicated_row::confine_reading`
denies a half stamp or one `PackageIdentity::new` would refuse (strictest reader).
Revocation never touches these rows: no exclusion at the scope-substitution seam or
in ts partials, so the stamp is a filter key, not a visibility switch. Limits: an
older peer's SCHEMAFULL table lacks the columns and fails the create of a stamped
row (peers upgrade together, as with `payload_id`); the window-membership door digest
does not cover the stamp yet (a canonical-log contract change, deferred). Scene
versions carry the same pair in the local journal fact (`node_log.payload.ext`),
never in the replicated `_fe_scene` marker (docs/extensions/governance.md
Provenance section).

**Typed rejection across the DB-thread seam (F24).** The dispatch loop's
`InsertIotReadings` arm maps a validation failure (`IotIngestError::{
UnknownAnchor, InvalidTimestamp, EmptyMetric}`) into
`DbResult::IotReadingsRejected` carrying
`fe_runtime::messages::IotIngestRejection` (same reply family as
`IotReadingsInserted`), NOT the generic `DbResult::Error` — so fe-api's
no-`db_reader` ingest fallback maps real HTTP statuses (422 vs 502) without
string-sniffing, with `Display` wording byte-identical to the direct path
(parity test in `tests/iot_reading_test.rs`). Only DB failures degrade to
`DbResult::Error`.

## §replication-mode (F5)

`replication_mode.rs::ReplicationMode` is the canonical per-table replication
shape: `ReplicationMode::for_table("iot_reading" | "device_payload") ==
Timeseries` (CDL-SPEC10 A3 widens this from `iot_reading` alone), every other
table is `Row`. It lives in this crate (not `fe-sync`) because the *inbound*
applier here must dispatch on it; `fe-sync` re-exports the enum for producers
so there is one definition, not two that can drift.

`ReplicationMode::union_key_field(table)` is the field `apply_timeseries_row`
(handlers/replicated_row.rs) keys its idempotent insert-only check on:
`reading_id` for `iot_reading`, `payload_id` for `device_payload`, `None` for
every other table. `asset` and `device_format` stay `Row` mode by the default
arm, but `apply_row_mode` gives them their own bespoke apply functions
(`apply_anchored_asset_row` / `apply_device_format_row`) rather than the
tombstone-aware merge the static hierarchy tables use — see §device-payload.

## §device-payload (R5, CDL-SPEC10 A3)

R4 override D-5/D-17: the existing `asset` table (`content_type` + iroh blob
`content_hash` + `owner_did` + anchor `petal_id`) is the generic asset model,
shared by IoT device payloads, path assets and `hexon_p2p_bucket` — one model,
not a foundry-only one. `device_format` declares an ingest format
(`{format_id, verse_id, mime, schema, declared_by, created_at}`, immutable per
`(verse_id, format_id)` — a conflicting `mime` is refused, the same `mime`
re-declared is an idempotent no-op); `device_payload` is the queryable index
of opaque bytes (`{payload_id, node_id, petal_id, verse_id, format_id,
recorded_at(_ms), hlc_timestamp, asset_ids: array<string>, source_did}` —
`asset_ids` is ORDERED, not deduplicated: the list IS the payload).

**`handlers::device_payload`** owns:
- `register_format` / `fetch_format` / `list_device_formats` — the format
  registry, keyed `(verse_id, format_id)` by an app-level WHERE check (the
  SurrealDB record id is NOT `format_id` — two verses may legitimately
  declare the same string — so uniqueness is enforced by `fetch_format`
  before every `CREATE`, the same read-then-write pattern `iot_reading`'s
  union key uses).
- `insert_device_payloads` / `list_device_payloads` — mirrors
  `insert_readings_with_replication` exactly: validate everything first
  (anchor in petal, format declared for the petal's OWN verse — resolved
  locally via `resolve_petal_scope_handler`, never trusting the caller's
  replication-only `verse_id` — every asset exists/anchored/MIME-matched,
  timestamp parses), durable insert, THEN publish (asset rows first, then
  the payload row — SPEC-10 §2.3 ingestion-story order), THEN record
  §window-membership. A client `payload_id` must be a ULID
  (`is_valid_payload_id`). A known one is an idempotent no-op ONLY when the
  existing row is in the same petal AND has the same `source_did`; any other
  holder of the id gets `PayloadIdConflict`, which names nothing about the
  existing row (Phase A review, MANDATORY finding 2: returning the row's
  `asset_ids` to any replayer was an IDOR). `fetch_payload` is petal-scoped
  and `payload_id_taken` answers only yes/no. A caller-supplied replication
  `verse_id` must be the petal's own verse, or the call fails before anything
  is written, so a payload is never published into a foreign verse's
  replica. `asset_ids` is bounded by the ONE shared cap,
  `fe_runtime::messages::DEVICE_PAYLOAD_MAX_ASSETS` (REST, MCP, this door and
  the inbound gate all read it).
- Payload links are petal-scoped: the reading door's `fetch_known_payloads`
  filters by the reading's petal, and so does the inbound `iot_reading` gate
  (MANDATORY finding 1: an unscoped lookup let a reading in petal P link to
  petal Q's payload).
- `record_window_membership` — `pub(crate)`, shared with
  `handlers::iot_reading`; see §window-membership.
- `node_is_in_petal` — `pub(crate)`, the local door's anchor check. The
  inbound gate no longer shares the boolean helpers (role propagation RP-4,
  AC-43): a boolean conflates "foreign" with "not here yet", so
  `handlers::replicated_row` resolves the same rows three-valued
  (`locate_petal`/`locate_node_in_petal`, one resolver for every table).

**Blob GC extension (`handlers::asset_gc`).** The referenced-asset set now
also flattens every `device_payload.asset_ids` array (a dedicated query — the
column is `array<string>`, so `SELECT VALUE asset_ids` returns one `Vec<String>`
per row that must be flattened client-side, unlike the scalar `strings()`
helper the node-reference query uses). Without this, an asset referenced only
by a device payload would be deleted after the grace period — a data-loss
trap found in the campaign plan's §2.2 recon. The query fails closed: an
undecodable result aborts the pass instead of reading as "nothing
referenced".

**Placement.** `crud::create_node_with_asset_handler` refuses any asset whose
`content_type` is not `model/gltf-binary`/`model/gltf+json`, so a device
payload asset never becomes a `blob://<hash>.bin` scene path handed to the
AssetServer even if a caller skips fe-api's `place_asset_core` check.

**Inbound replication (`handlers::replicated_row`).**
- `asset`: insert-if-absent, never overwrite, ANCHORED rows only (a legacy
  `petal_id: NONE` row is never shadowed by a peer's).
- `device_format`: insert-if-absent, immutable per key (re-declaration is
  silently ignored, never a conflict error — the admission gate is what
  refuses a bad actor, not this apply function). Outbound, the DB-thread
  `RegisterDeviceFormat` arm calls `register_format_with_replication`, which
  publishes a NEWLY created row (record id = `format_id`, durable first); an
  idempotent re-declare publishes nothing. Before the CDL-SPEC10
  review-and-fix a declared format never left its peer, so every inbound
  payload naming it stayed unusable there.
- `device_payload`: the generalized `Timeseries` union path, keyed on
  `payload_id`.
- **Admission (critic C3/C4; role propagation RP-4, AC-43).** All three tables go
  through the ONE parent-chain resolver every replicated table shares (§handlers
  "replicated_row"): `asset.petal_id`, `device_payload.{petal_id, node_id,
  asset_ids[*], format_id}` and `device_format.verse_id` must resolve to the
  replica's OWN verse. Three-valued: known-here passes, known-foreign or
  mismatched is **Denied**, unknown locally is **Deferred `Parent{table, id}`**
  (`petal`, `node`, `asset`, `device_format` keyed by `format_id`,
  `device_payload` for an `iot_reading` link) and applies with no second
  delivery when that parent lands. Before the manifest every one of them is
  Deferred `Manifest` (supersedes AC-34's pre-manifest Deny). `device_format`
  needs **Manager+ at the verse scope** (schema-defining — "strictest reader",
  critic C3); `asset`/`device_payload` use the standard Editor+ gate at the
  petal's scope.
- **Inbound shape (Phase A security review, 2026-10-15).** A check that skips
  what it cannot read is a check a peer can step around, so the gate denies
  instead of skipping: unparseable JSON on any R5 table; a `device_payload`
  with no `node_id`, no/empty/oversized `asset_ids`, a non-string entry, or a
  `verse_id` other than the replica's. Last, the door's SPEC-10 §2.2 rule 5
  applies to the peer's row: its `format_id` must be declared locally for the
  replica's verse and every listed asset must carry that format's MIME; an
  undeclared format is Deferred `Parent{device_format}` (RP-4), a MIME
  mismatch on a known asset is Denied. `apply_anchored_asset_row` denies an
  `asset_id` that fails `fe_runtime::safe_id::is_safe_path_id` (the legacy
  backfill joins it into a path), a missing/non-hex `content_hash`, a missing
  or negative `size_bytes` and a `content_type` that fails
  `fe_sdk::device_format::is_valid_mime`, and DROPS `owner_did`: the upload
  quota sums `size_bytes` by `owner_did`, so a peer could otherwise exhaust
  (or, with a negative size, refund) a local uploader's quota.
  `apply_device_format_row` applies the local door's `format_id` and MIME
  grammar. An inbound `iot_reading` must name a petal of the replica's verse
  and a node of that petal; a `payload_id` link must name a payload of the
  same petal (another petal's: Denied; not known yet: Deferred).

## §window-membership (SPEC-10 §3.4 rule 5, CDL-SPEC10 A3, A1 hand-back item 3)

`observation_window_member` (`{id, table, row_id, petal_id, source_class,
window_bucket, hlc_wall_ms, hlc_counter}`) is LOCAL-ONLY by construction:
never added to `ALL_TABLE_NAMES` (admin dump/clear) or given a
`ReplicationMode`. `handlers::device_payload::record_window_membership` is
`pub(crate)` and called ONLY from the two local ingest doors
(`insert_readings_with_replication`, `insert_device_payloads`), after their
durable insert, gated on `verse_id.is_some()` (no verse context, no window to
seal into — independent of whether a blob store / replication sender is
actually wired, since window membership is about what THIS node locally
accepted, not about publication).

**Why this table exists at all (the signing-oracle gap).** Without it, a
future batch sealer (Phase B) would have to pick window membership by
scanning hot tables or trusting `source_did` — and a malicious peer could
replicate a row stamped with THIS node's own DID and get it included in a
batch this node signs. `handlers::replicated_row` (the inbound path) MUST
NEVER write to this table, under any input, including a replicated row whose
`source_did` names this node itself —
`tests/device_payload_test.rs::inbound_replicated_rows_never_enter_window_membership`
is the regression pin; mutating `apply_row_mode`'s inbound arm to also insert
here must turn it red.

`source_class` on each row is the caller-declared
`fe_sdk::source_class::SourceClass` (SPEC-10 §7.1 rule 6) — recorded exactly
as the caller passed it, never inferred from `source_did`. This is why
`fe-sim/src/session.rs` sets `source_class: Simulation` explicitly even
though its `source_did` is the simulation lab's own host-peer DID: without
the explicit field, that DID would otherwise read as `device`/`connector`.

`OBSERVATION_WINDOW_BUCKET_MS` (one minute) is the sealing window width.
**CDL-SPEC10 B3 (2026-10-15) changed two things here:**

- **Mode gate (SPEC-10 §8 rule 2).** `record_window_membership` now records
  only when `FE_MIGRATION_MODE` is `dual_emit_shadow` or `shadow_rebuild`
  (`canon_log::observation::door_records_window_membership`, read per call).
  In `legacy_only`/`seam_ready` the door writes nothing extra, so the ingest
  path is byte-identical to the pre-SPEC-10 one. An unreadable flag records
  nothing (the direction in which this node signs nothing).
- **Bucket = ingestion HLC, not sensor time (§3.4 rule 2).**
  `window_bucket = window_bucket_of(hlc_wall_ms)`. Bucketing by `recorded_at_ms`
  would let a late sensor timestamp land in a window the sealer already closed;
  the local HLC is monotonic, so a closed window stays closed.

The table also gained `sealed_op_id` and `uncovered` (both `option<string>`),
written only by the B3 sealer (`canon_log/AGENTS.md` §observation), and
`door_digest` (`option<string>`, Phase B security review 2026-10-15), written
by the door: a commitment to the exact accepted row that the sealer re-checks
before signing (`canon_log/AGENTS.md` §observation, door digest).

**AC-7 (asset_backfill path guard).** `asset_backfill.rs`'s legacy-file join
(`dir.join(format!("{asset_id}.{ext}"))`) was safe only because inbound
`asset` rows fell to `NotApplicable` before this slice — nothing peer-supplied
ever reached it. Now that anchored `asset` rows replicate insert-if-absent, a
peer-chosen `asset_id` can reach this startup join on any machine that still
has legacy `imported/` files. The fix reuses the ONE shared path-id guard,
`fe_runtime::safe_id::is_safe_path_id` (already used by P3P4 S1's hexon-id
guard and the inbound `petal.terrain` check above) — NOT a third hand-rolled
guard ("shared cores + separate guards = drift", RUNBOOK §5) — refusing
before `legacy_file`/the write path ever builds a path from the id, counted
as a typed `failed`, never `missing`.

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
- **Strictest-reader field validation (p2p_asset_streaming S6, RUNBOOK §5
  "a writer of a shared column must satisfy the strictest reader").**
  `apply_static_row` MERGEs a whole petal row, so an Editor+-admitted but
  malformed field can still reach the local row. `replicated_row.rs` adds a
  per-field check BEFORE the merge, denying the whole row on failure:
  `render_distance` must be `NONE` or finite in `[0, RENDER_DISTANCE_MAX]`
  (mirrors `petal_render_distance::validate_render_distance`); `terrain` must
  be `NONE` or a JSON object under a byte cap whose `tileset_hexon_uris` (if
  present) is an array of hexon-id-charset strings (fe-database has no
  fe-terrain dependency, so this is a structural check only —
  `terrain_bridge::terrain_config_from_petal_json` is the second line of
  defence). See `handlers/petal_render_distance.rs` and the `is_valid_inbound_*`
  helpers in `replicated_row.rs`.


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

**`orphaned_creator` never claims a replicated or joined verse (role propagation
RP-4, owner integrity).** It used to rewrite `verse.created_by` to the local DID
for any verse with no third member — so after a restart an invitee (whose join
writes no other member row) claimed every replicated verse as Owner and judged
role statements against the wrong owner. It now claims only a verse whose creator
is NOT a `did:key:` identity (a legacy placeholder or a pre-RP-4 hex inviter
address) AND that no invite ever joined (no `verse_member` row with
`invited_by != peer_did`). Claiming a verse after a keypair rotation (old
`did:key` creator) is no longer automatic — deliberately: a real owner is always
a did:key, and the rule cannot tell an old local identity from a remote owner.
**The inbound side closes the loop (refine R2, OF-4 verify):** a trust-on-first-use
manifest whose `created_by` is not a `did:key:` identity is Denied at the admission
gate (§handlers "replicated_row" step 3), using the SAME predicate
(`reconcile::is_owner_identity`) — so no replicated manifest can ever land in the
shape this rule claims (the residual RP-4 left: a peer publishing a placeholder
creator would have handed its verse to every receiver at their next restart).
`held_role_statements_survive_restart` runs the restart with no extra member row.

**`ClaimOrphanedVerse` (F6) is the one explicit exception, not a reopening of
OF-4.** `handlers::claim_orphaned_verse` is a user-confirmed command, never
automatic, for the shape `orphaned_creator` refuses on purpose: a verse this
node itself created under a PRIOR local identity (an OS-keystore read error
minted a new, unpersisted DID — see `fractalengine/src/AGENTS.md`
§identity-tiers). It reuses `is_owner_identity` the same way the inbound gate
does, but to require `did:key:`-shaped (not to deny it), and factors the
shared "never invited a third party" check out of `claimable_verses` into
`reconcile::verse_never_shared` so both call sites stay provably identical.
It additionally refuses if any `role_grant`/`role_statement_pending` row in
the verse names a grantor other than the stale creator — a role issued by
anyone else means this verse's role fabric is not purely local-to-self, even
if no `verse_member` row recorded how.

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

**Wall-clock source override (F8 sim lab).** `set_wall_clock_source(Some(fn))
` installs a pluggable "now" that HLC stamping (`init_hlc` + `next_hlc_timestamp`)
reads instead of the system clock; `set_wall_clock_source(None)` restores real
time. This exists solely so the simulation lab's fleets stamp HLC from their
virtual accelerable `SimClock` (every simulated peer reads time from it, D3),
keeping monotonicity intact while making sim runs deterministic. It is a plain
`fn` pointer (no closure state — fe-sim's shim reads its process-global current
clock), installed only by sim scenarios; production processes never touch it.

**Sim-session snapshot/restore (DEC-C13, 2026-10-09).** `HLC_STATE` is
process-global and HLC is forward-only, so a sim session that resets it
(`init_hlc(0)` per harness peer spawn, reading the SimClock) would otherwise
leak sim time into any production DB thread sharing the process: a PAST sim
start breaks op-log monotonicity, a FUTURE one persists into production stamps
and survives restarts (`init_hlc(max_persisted)`). `snapshot_hlc()` copies the
state before the session installs its source; `restore_hlc(snapshot)` (after
the source is uninstalled) sets it to `max(snapshot, (system_now, 0))` —
reading the SYSTEM clock directly, never an installed source. An uninitialised
snapshot restores to real now (a later stamp never panics). fe-sim calls both
from its `HlcSourceGuard`, inside the scenario run lock. Residual: stamps a
co-resident production DB thread issues DURING a session still read sim time
— hence fe-sim's `start_ms ≤ now` validation and the relay's `FE_SIM_ALLOW`
opt-in (fe-sim `src/AGENTS.md` §hlc-sim).

`next_hlc_timestamp()` returns `(packed_u64, human_string)`; the human
string is `"<wall_ms>:<counter_hex>"`, stored in the `hlc_timestamp` column
for debugging / external tooling. HLC state sits in a process-global `Mutex`,
so stamping is correct from any thread. Callers are the DB thread's serial
command loop plus the API's direct IoT ingest on the shared handle
(§iot-readings, §shared-handle), so contention is low but not zero.

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

**Atomic variant — `commit_operation_atomic(db, entry, statements,
bindings)` (2026-10-11).** When the materialization is a list of SurrealQL
statements (no Rust-side logic between them), prefer this seam: it stamps the
HLC and runs `BEGIN; CREATE op_log CONTENT $__op_entry; <statements>; COMMIT`
as ONE transaction, so a failing statement rolls back the log row too — no
appended-but-unmaterialized window (proven by
`a_failing_statement_rolls_back_the_op_log_row_too`, the
`apply_marker_store` SPEC-4 §3.5 recipe). It returns the lamport clock and
each executed caller statement's rows: the result slots are
`[BEGIN, CREATE, caller…, COMMIT]` (BEGIN and COMMIT each occupy a NONE slot
in SurrealDB 3.0.5), so caller rows are slots `2..len-1`; a caller `RETURN`
ends the transaction's statement list early, so fewer rows than statements
can come back. Bindings starting `__op_` are refused (reserved). On failure
the error is the first statement error that is not SurrealDB's generic
"not executed due to a failed transaction" echo (`most_specific_error`).
Both seams serialize the entry through one helper (`op_entry_json`, nulls
stripped like `Repo::create_raw`). `commit_operation_atomic_with(db, entry,
|lamport| (statements, bindings))` is the same seam for statements that must
carry the op's own clock; `commit_operation_atomic` delegates to it. Callers:
`handlers/elevated_query.rs`, and `TombstoneNode` / `CascadeTombstoneNode`
(`crud.rs::commit_tombstone_atomic`, §lifecycle). Do NOT wrap a
call in `tokio::time::timeout` expecting a rollback: dropping the future does
not cancel the embedded engine's spawned query task (the transaction still
commits) — bound statements with a DB-side `TIMEOUT` instead.

This is substrate hardening only. `OpLogEntry` still uses the legacy local
format and placeholder signatures; do not add canonical-envelope, signature,
or wire semantics here.

## §handlers

`handlers/` holds the command handlers extracted from the DB dispatch loop,
one sub-module per domain:

- `crud` — Verse/Fractal/Petal/Node creation, GLTF import, hierarchy loading;
  API asset registration (`create_asset_reply` / `AssetRegistration`, which
  writes the `owner_did` + anchor `petal_id` columns — fe-api AGENTS.md §assets)
- `asset_gc` — `GcUnreferencedAssets` (fe-api AGENTS.md §asset-ingest): deletes
  `asset` rows no `node` references (tombstoned nodes count) older than the
  grace, then removes only THEIR blobs that no remaining `asset` /
  `crate_entry.asset_hash` / node `properties.path_asset.asset_path`
  (`blob://{hash}.{ext}` stamp descriptor) names. Never scans the blob directory (replication
  and hexon blobs share the store). Fire-and-forget: success replies
  `AssetsGarbageCollected`; failure is only logged, because an uncorrelated
  `Error` would be delivered to the oldest API waiter
- `asset_backfill` — one-time startup ingest of legacy `imported/` GLBs into
  the blob store (§asset-resolution)
- `blob_migration` (OF-1/S1b, 2026-10-14) — `referenced_asset_hashes` (every
  hash a live row still names; `asset_gc` shares the same three queries
  through `referenced_hashes_among(db, Some(doomed))`, which binds its doomed
  subset so a GC pass never loads every asset hash — final review) and
  `migrate_legacy_blob_store`: a marker-gated, copy-only, crash-safe one-time
  pass that backfills the per-install blob store from the pre-2026-10-14
  per-user one, called from the binaries' `main` right after the DB thread's
  shared handle is ready (fe-sync/src/AGENTS.md §blob-store). S8 hardening
  (2026-10-14): a legacy file whose bytes do not hash to its name is skipped
  (never carried forward), and when the new root resolves to the legacy
  directory itself (`FE_BLOB_DIR` pointed there) the pass returns without
  writing anything, not even the marker — the legacy dir is never written.
  Hash ids come from the DB but pass `hash_from_hex` (64 hex) before
  `canonical_blob_path`, so a DB row cannot name a path outside either
  store. Crash-retry is tested, not assumed:
  `a_crash_mid_migration_is_retried_without_loss_or_legacy_writes`.
- `entity` — rename, delete, description updates
- `entity_property` — custom property CRUD for nodes
- `field_def` — field definition schema CRUD
- `transform` — node position/rotation/scale and URL persistence
- `preconditions` — shared cheap target/scope checks required before append
- `rbac` — role resolution; `AssignRole`/`RevokeRole` issue local role
  statements (§role-propagation)
- `device_payload` (CDL-SPEC10 A3) — R5 device format descriptor registry +
  device payload ingest/list; see §device-payload
- `invite` — verse invite generation and join-by-invite
- `api_token` — API token minting, revocation, and listing
- `seed` — default data seeding
- `admin` — database reset
- `crate_registry` — hexon crate registry install/uninstall
- `petal_terrain` — per-petal terrain config get/set; `*_reply` builders answer
  correlated (API) callers privately (`PetalTerrainFetched` / `RequestFailed`).
  S6 (critic step 8): now op-logs (`OpType::SetPetalTerrain`) before the
  `UPDATE … RETURN AFTER`, then replicates the read-back row via
  `replicate_row_with_petal` — closes the prior parity gap (this write used to
  neither op-log nor replicate, while S6's own render-distance write would
  otherwise have carried the whole row, `terrain` included, unvalidated).
  **Petal terrain trust (S8b).** `replicated_row::is_valid_inbound_petal_terrain`
  is the strictest-reader shape check: a JSON object of at most 64 KiB whose
  `tileset_hexon_uris` are `fe_runtime::safe_id::is_safe_path_id` ids, whose
  `cache_dir` (a filesystem path the disk tile cache reads and prunes) is
  relative safe components only, and which carries no
  `elevation_api_key_env` (which env var a node reads a secret from is
  never a peer's choice). The inbound gate denies the WHOLE row on failure,
  and the LOCAL writer (`set_petal_terrain_handler`) runs the same check
  before the op-log, so this node never writes a row its peers would deny.
  fe-terrain's `safe_cache_dir` re-checks `cache_dir` at the reader.
- `petal_render_distance` — "Petal render distance" (p2p_asset_streaming S6):
  `set_petal_render_distance_handler` resolves the petal's scope →
  `visibility_groups::authorize_edit` (Editor+, the SAME gate `SetScopeHidden`
  uses) → range-validates (`[0, RENDER_DISTANCE_MAX]` = 50 000, mirrors the
  schema `ASSERT`) → op-logs (`OpType::SetPetalRenderDistance`) →
  `UPDATE … RETURN AFTER` → replicates the read-back row. `None` clears
  (`render_distance = NONE`, inherit the global). One reply shape for both
  Set and Get: `PetalRenderDistanceLoaded { petal_id, render_distance,
  correlation_id }`.
- `visibility_groups` — petal Group registry get/set + persisted scope eyes
  (`*_reply` builders, Editor+ via fe-policy; §lifecycle "Visibility groups")
- `raw_query` — `RawQuery` reply (`raw_query_reply`): SELECT-only keyword
  blocklist, `QueryResult`/`QueryFailed` echoing the caller's correlation id
- `elevated_query` — `ElevatedQuery` reply (`elevated_query_reply`), the
  `/api/v1/query/elevated` single-writer path (fe-api AGENTS.md
  §elevated-query): refused before any work when `request.statements.len()`
  exceeds `MAX_ELEVATED_STATEMENTS` (32, the single definition — fe-api's
  `validate_elevated_sql` fast-fails against it first, but this check is the
  authority), OR when SurrealQL's own parser (`surrealdb_core::syn::parse`,
  same 3.0.5 the engine runs) counts a different number of statements in the
  joined script (EL-1/EL-3, refine R2: the API splitter does not model regex
  literals, so `/'/` could hide `;`-separated statements from the cap and from
  their `TIMEOUT`); DB-side table mirror `ELEVATED_WRITABLE_TABLES`; a
  read-only script runs as-is; a write script is capped at
  `ELEVATED_DIFF_MAX_ROWS` (20 000) rows across its write tables, then
  snapshot → `commit_operation_atomic` (one `OpType::ElevatedQuery` entry) →
  post-snapshot diff → `edit_seq += 1` on updated nodes → one `node_log`
  `elevated_write` fact per changed node (§node-log) → `SceneChange`s →
  `replicate_row_with_petal` per created/updated `node`/`fractal`/`petal` row
  (durable first, then events). Node scene mapping reuses
  `replicated_row::node_dto_from_row`. No wall-clock budget wraps the call
  (a dropped future does not cancel the transaction — §log-first-commit);
  DML statements arrive with `TIMEOUT 5s`
- `mod.rs::correlated_failure` — `RequestFailed` for a correlated caller, the
  wildcard `Error` otherwise (fe-runtime src/AGENTS.md §api-reply-correlation)
- `iot_reading` — append-only IoT sensor-reading ingestion (§iot-readings)
- `ts_partial` — local partial execution for distributed timeseries queries
  (§ts-partials): renders the structured spec's SQL via the fe-query builders,
  never accepting SQL from the wire
- `node_log` — append-only per-node operation log (§node-log)
- `replicated_row` — inbound P2P row apply (A3+A4): `DbCommand::ApplyReplicatedRow`
  → **admission** (`admit_inbound_row`; role propagation RP-4 — see the gate
  paragraph below) → **per-table mode dispatch**
  (`dispatch_by_mode`: `ReplicationMode::Row` → whole-row merge; `Timeseries`
  → append-only union for `iot_reading`, §replication-mode) →
  geometry-safe, tombstone-honoring durable write (`merge.rs`) + petal-scoped
  `SceneChange`;
  loop-free by construction (no replication sender in scope). **The gate
  (RP-4), in this order:** (1) a verse row whose payload `verse_id` ≠ the
  replica's verse is denied (scope-injection guard); (2) `canon_device_enrolment`
  is verified first (signature + self-authorship, Denied on failure even before
  the manifest), then waits for the manifest and for membership
  (canon_log/AGENTS.md §device-enrolment); (3) **first-manifest rule** — before
  a manifest exists ONLY the verse row is admitted (trust on first use — and only
  when its `created_by` is a `did:key:` identity, §reconcile OF-4); every
  other row is `Deferred Manifest` and retried when it lands (the old bootstrap
  window that admitted everything is gone — deferral now retries, so nothing is
  lost); (4) **referential confinement** (P2P-REF-CONFINE): one parent-chain
  resolver (`locate_fractal`/`locate_petal`/`locate_node_in_petal`) confines
  every row — `fractal.verse_id`, `petal.fractal_id` (required), `node.petal_id`
  (a tombstone or partial row uses the durable row's petal; an unknown node with
  no petal waits on `Parent{node}`), `iot_reading.{petal_id, node_id,
  payload_id}`, and the §device-payload tables — AND the durable row's CURRENT
  parent (a cross-verse re-parent or tombstone is Denied). Three-valued:
  known-here → its scope; known-foreign → `Denied`; unknown → `Deferred
  Parent{table, id}`. An id carrying `#` cannot form a scope and is Denied;
  (5) **role at the row's deepest scope** (`resolve_role` over the
  materialized cache = `role_at`): an Editor appointed for one petal passes
  there and is `Deferred AuthorRole` elsewhere; a row that MOVES a durable row
  needs Editor+ at both the old and the new scope. **Verse manifests:** the
  identity fields `verse_id`/`created_by`/`created_at`/`namespace_id` are
  immutable (a change is Denied for everyone, the owner included — T9, the
  MERGE used to let an Editor rewrite `created_by`; a missing `namespace_id` may
  be filled once, and that fill is a governance change — it picks the replica
  this node opens next, so an Editor's fill is deferred, refine R2); a change to
  a governance field (`default_access`, `ts_mode`,
  `ts_replication_factor`, `ts_bucket_width_ms`) needs **Manager+ at the verse
  scope** (a petal or fractal Manager is deferred — verse settings lie above
  their subtree); `name`/`view_hidden` follow the Editor+ gate. **Invite owner
  pin (T10):** `VerseInvite` signs `owner_did` + `verse_created_at` (the
  inviter's `verse.created_by`/`created_at`, a did:key — a non-did owner is
  refused at generate and at join); the joiner writes them as its local
  manifest, so the owner's rows resolve to Owner and a manifest naming another
  owner meets the immutable-identity check (Denied — the pin IS the local
  manifest, so it is never a "first" manifest; relays that open a replica
  without an invite stay trust-on-first-use, D-23). **API role cap (AC-46,
  T26):** `lifecycle_auth_context` caps an `Identified` caller at
  `min(token role, role_at(local_did, scope))` (`node_role_at`, the one
  helper), which also covers `authorize_edit` (`SetPetalTerrain` — now gated,
  RP-4 step 7 — `SetPetalRenderDistance`, visibility groups, node lifecycle);
  the role-less doors (`InsertIotReadings`, `InsertDevicePayloads` when they
  replicate, `RegisterDeviceFormat`, and `CreateAsset` for an anchored upload —
  W6 TC-7, refine R2: its row replicates with the device payloads that list it;
  an anchor petal that does not resolve is refused) require the NODE itself to
  clear Editor+ at the petal / Manager+ at the verse (`require_node_role`), so a token minted
  before a remote demotion cannot write rows every peer defers. The wire **tombstone form is the empty entry**
  (`Doc::del`): the handler synthesizes the node payload and resolves the
  scene change's petal from the durable row; an empty entry on a static
  hierarchy table is `NotApplicable` (no tombstone semantics there). A
  third rule (F20, 2026-10-07): an inbound **verse manifest's
  `default_access` must be `"viewer"` or `"none"`** — anything else is
  denied in `apply_static_row`. The schema column carries only a
  `DEFAULT 'viewer'` (no ASSERT), so without this a replicated manifest
  could set `"editor"` and make every unknown peer resolve to a writer for
  that verse, defeating A3 deny-by-default on the one path that writes it.

## §asset-resolution

**The blob store is the only asset source** (retired 2026-10-13, RUNBOOK §3
R2). `load_hierarchy_handler`'s `resolve_asset_path` and
`load_nodes_by_petal_handler` turn a node's `asset_id` into
`blob://{content_hash}.glb` only when the asset row has a `content_hash`
whose blob `has_blob`; anything else is `None` (no mesh) with a warn in the
hierarchy path ("no content_hash" / "Blob missing" / "Invalid content_hash").
There is **no disk fallback** any more: the old resolver probed
`fractalengine/assets/imported/<asset_id>.{glb,gltf}` and returned the
asset-server-relative `imported/<file>` path. Nothing has written that
directory since import moved to the blob store (`import_gltf_handler`,
`seed_assets`, API `create_asset_reply` all `add_blob` then record the hash);
it is now gitignored and its local copies are kept.

**One-time backfill** (`handlers/asset_backfill.rs`,
`backfill_imported_assets`) closes the gap for rows that only ever rendered
via that fallback:

- **Where:** the DB thread's startup block, right after the Phase C
  base64→blob migration and before `DbResult::Started`. That block is the one
  place that already owns both the `Db` and the `BlobStoreHandle`, runs once
  per process, and finishes before the UI can send the first `LoadHierarchy`
  — so the first hierarchy load already sees the backfilled hashes. Doing it
  lazily inside `load_hierarchy_handler` would put disk I/O and writes on a
  read path that runs on every navigation reload. Failure is logged and never
  blocks startup (same contract as Phase C).
- **What:** if `imported_assets_dir()` holds no `.glb`/`.gltf` (the normal
  case on a fresh install) it returns before touching the DB. Otherwise it
  scans every `asset` row (`asset_id, content_hash`); a row whose
  hash parses and whose blob exists is skipped (this is the idempotency — a
  second run finds nothing). For the rest it ingests
  `<asset_id>.glb`, else `.gltf` (the old probe order) with `add_blob` and
  `UPDATE asset SET content_hash, size_bytes` (`size_bytes` is always the
  ingested file's length, so it matches the blob the row now names, even
  when the hash was re-pointed). Summary log:
  `backfilled` / `missing` (no blob and no legacy file — unrecoverable here;
  the node renders without a mesh and the resolver warns) / `failed`.
- **Full scan, not dir-driven:** the `missing` count needs every row; the
  cost (one SELECT + one `has_blob` stat per row) is paid only on machines
  that still have legacy files.
- **Hash replacement:** a row whose recorded `content_hash` blob is gone but
  whose legacy file hashes differently is re-pointed at the file's hash (warn
  logged) — that file is exactly what the old fallback was rendering.
- **Local-only, never deletes:** like Phase C it is a plain `UPDATE` (no
  `replicate_row` call site emits the `asset` table) and it never removes files from the legacy
  directory. `IMPORTED_ASSETS_SUBDIR` / `imported_assets_dir()` survive only
  as the backfill's source path (`examples/dump_db.rs` lists the directory).

## §role-propagation (role_propagation_20261013 RP-2)

Roles replicate as **per-grantor role statements**, not as `role` rows
(R7; plan §2). The pure model — validation, the least-fixpoint
materializer, `admissible`, the cache projection — is fe-policy
`role_grants` (fe-policy/AGENTS.md §role-grants); `src/role_grants.rs` is
the DB plane around it.

- **Storage (§2.1).** `role_grant` holds ADMITTED statements,
  `role_statement_pending` holds DEFERRED ones (grantor not yet Manager+ at
  the statement's scope, or no manifest yet). A key
  `(scope, peer_did, grantor_did)` — the doc record id
  `{scope}/{peer_did}/{grantor_did}`, also stored as `statement_key` — has
  exactly ONE live version across both tables, the newest by fe-policy
  `latest_per_grantor` (HLC, then canonical payload bytes). Every move
  between the tables is one transaction (`write_live_version`). `hlc` is a
  decimal string: the wire range is u64, SurrealDB's int is i64.
- **Placement (`place_statement`).** Older-or-equal than the live version →
  `Superseded` (idempotent, nothing written). Admissible under the current
  materialization → `Admitted`. Otherwise `Held`, unless it would create a
  NEW held key beyond `ROLE_PENDING_PER_GRANTOR` (1024/verse) for its
  grantor → `RefusedOverBudget` (counted, `role_pending_refused_count()`;
  the replica still holds it and the sync-plane re-read recovers it, §2.5).
  There is no shared or per-verse cap — that is the eviction vector D-RP-1
  removed — and the budget is keyed by the attested grantor, so no author
  can occupy or evict another's keys. `submit_statement` = snapshot the prior
  materialization + place + repartition; the RP-3 inbound arm calls it.
- **Partition (`repartition`, §2.4).** Recompute M over `role_grant`;
  **demote** every admitted statement M no longer admits (never refused,
  never dropped, and it cannot move M — the fixpoint already ignores it);
  **promote** held statements of each DID whose authority ROSE, looked up by
  the `(verse_id, grantor_did)` index only — a flooder's held rows are never
  scanned unless it gains authority; re-validate in full at promotion (a
  pre-manifest hold that targets the owner is deleted as Denied); repeat
  with the newly raised DIDs; then diff-write the cache. "Rose" is computed
  per mentioned cell (`changed_between`): any rise of authority at a scope
  implies a rise at some statement cell on its walk, so it is a superset.
  fe-policy's `EffectiveRoles::diff` is deliberately not used here — its
  per-DID "best role" can miss a rise at a narrower scope when the DID
  already holds the same level elsewhere (the RP-1 hand-off note).
  `prior = None` (startup, migration, manifest landing) seeds the risen set
  with the owner and every admitted peer. Invariant afterwards:
  `role_grant` = exactly the latest statements admissible under M,
  `role_statement_pending` = the rest.
- **Cache write (`write_cache`).** One transaction per verse, touching only
  `role` rows whose scope `parse_scope`s to THAT verse — `VERSE#v` or
  `VERSE#v-FRACTAL#…`, re-checked with `parse_scope` — and never a verse id
  shaped like the canonical-log interim authority key (64 lowercase hex,
  `is_interim_authority_verse_id`), nor a `-RESOURCE#` key (parse fails).
  The interim `VERSE#<hex>[-PETAL#<hex>]` rows that
  `DurableAuthorizationView::load` reads are therefore structurally out of
  reach (AC-45, T18).
- **Local commands (§2.3, `role_manager`).** `assign_role_checked` /
  `revoke_role_checked` refuse, before the op-log append and with a typed
  `RoleCommandError`: `owner` as a role (T3), the owner as target (T5), a
  caller below Manager+ at the scope (subtree authority, T19), and a
  Manager assigning `none` (Managers only raise; the owner's `none` pins).
  Then: op-log (§log-first-commit) → the statement (hlc = the op's HLC) into
  `role_grant` → `repartition` (which may promote statements the grant just
  empowered) → publish `role/{scope}/{peer}/{grantor}`. A revoke is the
  caller's own `revoked` statement: it retracts only the caller's grant (an
  owner revoke blocks the cell), and `RevokeReport::still_held` says when
  the target keeps its role through another grantor or inheritance (logged;
  the UI surface is D-32 FR-4). A local command never defers.
- **Durable re-emit (AC-42, T23).** `replicate_row` only reaches an OPEN
  replica and fe-sync's pending writes are capped, drop-oldest and
  process-lifetime (fe-sync AGENTS.md §pending-writes). So
  `reemit_local_statements` republishes every live statement this node
  authored (admitted or held — one per key, always the newest, so it can
  never replay a superseded grant past a later revocation) at DB startup and
  on `DbCommand::ReemitLocalRoleStatements`, which the sync thread sends
  after every successful replica open. iroh dedupes identical bytes.
  **Paced, revocations first (refine R2, the RP-2 watch item):** the re-emit is
  the only second chance, so unlike live publishes it does not drop on a full
  bridge — `publish_paced` retries every 5 ms within a 2 s budget per call
  (async sleep; the deadline keeps the DB loop live even if the bridge and the
  sync thread stall each other), revocations take the first slots, and only
  past the budget are the rest counted as `replication_drop_count` drops (a
  warn names how many were revocations; the next replica open retries them).
  Before this a node with more live statements than the 256-slot bridge
  dropped the same deterministic tail on every re-emit.
- **Startup (`role_grants::startup`).** After `reconcile` (which may change
  an owner) and before `Started`: the migration below, `repartition_all`
  (crash repair between a statement write and its partition), then the
  re-emit.
- **Migration (`migration/role_grants.rs`, plan step 7).** Unlike §reconcile
  this is **marker-guarded** (`local_migration_marker` key
  `role_grants_v1`): rerunning it after the cache exists would misread cache
  rows as legacy grants. It is local-only data (role rows never
  replicated), so the P2P "peers can't coordinate a migration" argument does
  not apply. For each `role` row whose scope parses to an existing,
  non-interim local verse: `owner` rows are dropped (every implicit
  petal-creator row, R7/T8); other rows in a verse this node owns become
  owner statements, each op-logged so its HLC survives a restart (a fresh
  local statement must always outrank it); everything else is dropped with a
  warning (A2). Statement first, row delete second, marker last — a crash
  reruns safely. Rows of unknown verses are left untouched.
- **Petal creators are not owners (R7).** `create_petal_handler` and the
  seeder no longer write a `role: "owner"` row at petal scope; the creator
  keeps the role they resolve to from the enclosing scope.
- **Owner resolution needs no cache invalidation (F6 diagnosis).** The F6
  in-app lockout investigation considered, and ruled out, two hypotheses
  before landing on the real root cause (an unpersisted ephemeral identity,
  `fractalengine/src/AGENTS.md` §identity-tiers): (b) that the viewer badge
  and the actual write gate read different role sources — they don't, both
  the UI badge and `rbac::require_manage_role`'s gate resolve through the
  same `resolve_local_role_handler` → `role_manager::resolve_role`; and (c)
  that a stale materialized `role` row with `role: "owner"` for the old
  identity was shadowing the new one — it isn't read at all, because
  `resolve_role`'s owner check (`peer_did == verse.owner_did`) compares
  `verse.created_by` LIVE, never the cache. So `ClaimOrphanedVerse` only
  has to fix `verse.created_by`; the owner-level UI/gate both see it on the
  very next resolve, same frame, no repartition or re-emit needed. A legacy
  `role: "owner"` row for the old identity is dead weight, cleaned up only
  by the ordinary `migrate_legacy_role_rows` pass (unconditionally drops
  every `role = "owner"` row, R7) — never a functional requirement.
- **Admin (`admin.rs`).** `KnownTable` includes `role_grant` and
  `role_statement_pending` (reset/dump/load cover them). `clear_table` on
  `role` or either statement table rebuilds the cache from the remaining
  statements — to remove roles, clear `role_grant`. `load_from_file` skips a
  statement-bearing dump's derived `role` rows and repartitions; a legacy
  dump (no statements) re-arms the migration marker instead.
- **Out of reach:** neither statement table is in `ELEVATED_WRITABLE_TABLES`
  (pinned by `elevated_writable_tables_never_include_role_tables`), nor in
  fe-api `/query`'s `ALLOWED_TABLES` (plan §7 A7, RP-9 records it).

## §inbound-admissions (role_propagation_20261013 RP-3)

Inbound rows (`DbCommand::ApplyReplicatedRow`) have three verdicts:
`Applied`-family, `Denied` (structural — permanent), and
`Deferred(DeferralReason)` (not converged yet — retried). Plan §2.4.

- **Role statements (`role/{scope}/{peer}/{grantor}`).** Routed by
  `apply_replicated_row_reporting` BEFORE the Editor+ gate to
  `apply_role_statement`: parse the key, parse the payload, run
  `fe_policy::validate_statement` against the ATTESTED author (owner `None`
  pre-manifest → only the owner-independent checks); any failure is `Denied`.
  An interim `VERSE#<hex>` scope is `Denied` (T18). Then
  `role_grants::submit_statement`: `Admitted` → `Applied` and the partition's
  risen DIDs become retry triggers; `Superseded` → `Applied` (idempotent) unless
  the key's live version is held, then `Deferred`; `Held` and
  `RefusedOverBudget` → `Deferred(Manifest)` pre-manifest, else
  `Deferred(AuthorRole{grantor})`. A `Doc::del` (empty entry) is
  `NotApplicable` — revocation is a non-empty `revoked: true` statement.
  Role statements are durable in `role_statement_pending` (§role-propagation),
  never in the in-memory queue, and never wait on `Parent`: the cache is keyed
  by scope strings, so a grant for a petal not known yet takes effect when the
  petal row lands.
- **Manifest landing.** A `verse` apply that changes the owner (first
  manifest, or a `created_by` change) runs `repartition(verse, None)`, which
  promotes every pre-manifest hold the owner empowers; its risen DIDs and a
  `Manifest` trigger feed the content retries.
- **A3 gate.** An author below the threshold (Editor+ write gate; Manager+
  for `device_format`) is `Deferred(AuthorRole{author})`, not `Denied`.
  Structural checks (scope injection, a known-foreign reference, immutable
  manifest identity, the petal terrain/render-distance shape checks) stay
  `Denied`; since RP-4 an unknown parent is `Deferred(Parent{table, id})` and
  any non-verse row before the manifest `Deferred(Manifest)` (§handlers
  "replicated_row").
- **Enrolment (AC-44, T27).** `canon_device_enrolment` stays first: signature
  and self-authorship failures are `Denied`. An ENROLMENT then waits for the
  manifest (`Deferred(Manifest)`) and for membership — the principal resolves
  to a role at the verse scope (owner / explicit row / `default_access`) OR
  `EffectiveRoles::has_any_role` (a petal-scoped member) — else
  `Deferred(AuthorRole{principal})`. Revocations stay unfloored.
- **`InboundAdmissions` (content rows).** The ONE entry point both the
  production loop (`lib.rs`) and the test harness (`fe-test-harness`
  `peer.rs`) call. A deferred content row is queued by
  `(verse, table, record_id, author)` with its latest bytes and indexed by
  wait reason; caps `DEFERRED_PER_VERSE` 4096 and `DEFERRED_PER_AUTHOR` 512 per
  verse, drop-oldest, counted (`deferred_admission_drop_count()`). Retries are
  a worklist, not recursion: triggers are a `Manifest` landing, an applied row
  (`Parent{table, record_id}`), and each risen DID (`AuthorRole`); a local
  `AssignRole`/`RevokeRole` retries every `AuthorRole` waiter of its verse.
  Every settled retry is echoed as its own `ReplicatedRowApplied`. It
  terminates: each retry consumes a queued row, a row re-queues only if still
  deferred, and triggers come only from applies (a bound guards bugs). No
  channel is awaited (self-drain rule). A restart or overflow loses queued
  content rows; the snapshot replay on the next replica open re-feeds them
  (plan §7 A4). A content-row flood can delay content, never change roles.
- **One-shot callers.** `apply_replicated_row_handler` (tests, earlier
  phases) returns the outcome alone and never retries a `Deferred` row.

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

**Governance gate (role_propagation RP-2 step 5).** Both
`SetVerseTimeseriesSettings` and `SetVerseDefaultAccess`
(`set_verse_default_access_handler`) require the local caller to hold Manager+
at the **verse** scope (`rbac::require_manage_role`): a petal or fractal
Manager is refused, because verse governance lies above their subtree.
`SetVerseDefaultAccess` now also re-emits the read-back manifest, the same way
settings do, so the change propagates deliberately (plan §7 A3).

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
- **Cost bound (QA-0138/QA-0139, 2026-11-07).** `LatestPerAnchor` is one
  indexed petal scan reduced in Rust (`newest_per_anchor_metric`: max
  `recorded_at_ms`, ties by `hlc_timestamp`) — the fe-query correlated
  `math::max` predicate is quadratic in `iot_reading` and measured 4-5 s per
  partial in a debug build at a few hundred readings, back to back on every
  peer, which starved transforms, readings and Serve admissions. `AllReadings`
  carries `LIMIT row_cap + 1`. `DbCommand::ExecuteTsPartial.deadline` is the
  waiter's give-up time: a partial still queued past it is skipped, never run
  for nobody. Every DB command slower than 250 ms logs `slow DB command` with
  its variant and the queue depth behind it (`lib.rs`, `variant_name`).

## §node-log

The `node_log` table is append-only: each row is an immutable fact recording
a single mutation on a node, INSERT-only — no UPDATE or DELETE is ever
issued against it. `row_version` is monotonically increasing per `node_id`
and serves as hidden metadata for "most recent state" queries;
`hlc_timestamp` is the HLC-packed `u64` (§hlc) for time-series ordering and
distributed merge.

`handlers::node_log::append_node_log` derives `row_version` as
`SELECT math::max(row_version)` + INSERT, which is NOT atomic. It is safe
only because it has exactly ONE caller context: the DB thread's command loop,
which `block_on` drives on the DB thread itself and which awaits each handler
to completion before receiving the next command, so no two calls execute
concurrently. The runtime is multi-thread since OF-2 (§shared-handle), and
that is fine: its workers only run SurrealDB's router and query tasks, never
handlers. **This invariant must be maintained.** Never call `append_node_log`
off the DB thread (e.g. on fe-api's shared `db_reader`), never spawn handlers
onto the runtime, and never run two node-log writes under `join!`/`join_all`.
If any of that becomes necessary, first replace the derivation with an atomic
subquery or a DB-level sequence.

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

**Id lookup indexes.** `apply_all` also defines `idx_petal_id`,
`idx_fractal_id`, `idx_node_id`, `idx_verse_id` on each hierarchy table's OWN
id column, backing the scope resolvers' `WHERE <id> = $x LIMIT 1` point reads
(crud.rs `resolve_*_scope_handler`, fe-api `rest.rs` resolvers — fe-api
AGENTS §existence-oracle). They are plain, not UNIQUE: a UNIQUE definition
validates existing rows eagerly and would fail startup on a store holding
duplicate ids written by raw/elevated SQL (cf. the `reading_id` fallback);
the goal is lookup cost, not enforcement. `IF NOT EXISTS` keeps them
idempotent on populated stores (`schema_idempotent.rs::id_lookup_indexes_exist_after_apply_all`).

**`array<object>` FLEXIBLE (VIS-PH4 `petal.visibility_groups`).** SurrealDB 3
rejects `FLEXIBLE` on a bare `array` ("FLEXIBLE can only be used with types
containing object"), and `TYPE option<array<object>> FLEXIBLE` still rejects
the ELEMENT objects' keys in a SCHEMAFULL table ("Found field
'visibility_groups[0].id', but no such field exists"). Defining the column
auto-creates its element field `visibility_groups[*]`, so `IF NOT EXISTS`
silently skips a re-declaration; `apply_all` therefore runs
`DEFINE FIELD OVERWRITE visibility_groups[*] ON TABLE petal TYPE object FLEXIBLE`
after the petal table (OVERWRITE is idempotent). The new eyes
(`verse`/`fractal`/`petal.view_hidden`, `option<bool>`) need no backfill:
`NONE` = visible. Pinned by
`schema_idempotent.rs::visibility_fields_apply_to_a_populated_pre_phase4_store`.

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

**Stamp position lock (D-A5, T5-LOCK).** `update_node_transform_handler`
(handlers/transform.rs) reads `properties.node_kind` + the stored position in
one SELECT after `require_node_exists`; for a `node_kind = 'stamp'` row whose
requested position differs from the stored one by more than 1e-4 on any axis
it bails with `position-locked: stamp …` BEFORE `commit_operation`, so no
op-log row is appended and the row is unchanged. Rotation/scale-only updates
(same position) proceed. The dispatcher's existing error arm turns the Err into
a `SceneChange::TransformFailed` rollback, which covers the UI/WS path too.
fe-api's `stamp_translate_denied` is only the fail-fast pre-broadcast twin.

**Cascade = one materialized subtree update (FR-2).**
`cascade_tombstone_node_handler` BFS-collects the N-level descendant subtree
over the durable parent edges (`properties.parent_id` / `gpx_track_id` /
`owning_path_id`, de-duplicated, depth-capped), appends its operation through
§log-first-commit, then stamps the whole set in one UPDATE statement. The
operation append and the row update are ONE transaction
(`commit_tombstone_atomic` → `op_log::commit_operation_atomic_with`, whose
closure builds the `$ts` marker from the stamped lamport): a failing update
rolls the `NodeTombstoned` row back too, so there is never a
logged-but-unapplied cascade (`tombstone_atomicity_tests` injects a THROWing
node EVENT). The single-node `tombstone_node_handler` uses the same seam.
The dispatcher emits one petal-scoped `SceneChange::NodeRemoved` per id in the
materialized subtree, so `EntityStore` and WebSocket mirrors discard every
affected descendant rather than only the cascade root.
An already-tombstoned root is an idempotent no-op: the handler returns an empty
affected set before the atomic commit, so it appends no second operation,
updates no rows, and emits no duplicate scene or lifecycle event.

**Merge non-resurrection (`merge.rs`, N-4).** `apply_replicated_node` is the
durable counterpart of `EntityStore::upsert`'s tombstone guard: an incoming
*live* row for a locally-tombstoned node is skipped (`SkippedTombstoned`), an
incoming tombstone converges the local row to deleted. fe-sync's `reconcile_petal`
drives this for inbound peer rows.

**Order-independent node rows (QA-0156, 2026-11-07).** Two authors' concurrent writes
to one node reach peers in different orders, so every inbound node field has a named
tiebreak, never "last to arrive":
- **Complete snapshots** (every current publisher, `_fe_scene` marker) are a whole-row
  register ordered by (original HLC, publisher): scene_publication/AGENTS.md "Wire and ordering".
- **Markerless (legacy, clockless) live rows** go through `merge::apply_replicated_node_by`
  with the **legacy writer precedence**: the verse owner first, then the larger DID
  (`LegacyAuthor`'s `Ord`). `node_legacy_writer` (local, never replicated, in the dev
  reset list) records the node's highest-ranked markerless author; a lower-ranked one is
  `SkippedLegacyPrecedence` (`NotApplicable`). One author's own rows keep iroh-docs'
  per-slot order. Rows without a known author (`reconcile_petal`) keep the blind merge.
- **Tombstone markers** from two deleters settle on the larger marker (sorted-key JSON).

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

**Node view (`SetNodeView` / `LoadPetalViewState`, hierarchy_visibility_groups
Phase 3, `handlers/node_view.rs`).** A node's visibility override persists as
`properties['view.state']` (`"show"`/`"hide"`; ABSENT = Auto) and its Group
membership as `properties['view.groups']` (ABSENT = none). D-15 forbids raw
`SetNodeProperty` for these, so the arm follows the lifecycle order: validate
(state ∈ auto|show|hide, ≤ 64 group ids of 1-64 chars) → tombstone-aware
resolve (`resolve_live_node_petal`: missing/tombstoned = failure) → petal scope
→ `lifecycle_auth_context` (`Local` resolves the real local role) →
`authorize_node_edit` (Editor+) → writes. Each key goes through the same
`PropertySet`/`PropertyDeleted` op-log commit `SetNodeProperty` uses (reuses
`entity_property`'s handlers, so HLC + log-first are inherited); `"auto"` and
an empty group list DELETE their key rather than store a default. Success
emits one `SceneChange::PropertyChanged` per touched key (owning petal) and
replies `NodeViewSet` with the read-back values; failures go through
`correlated_failure` (`RequestFailed` when correlated — the GUI always
correlates (`fe-ui:view:{node_id}`) so a refusal never lands on an API waiter
as a wildcard `Error`). `LoadPetalViewState` is one read-only SELECT that
projects only the two keys (a track's bag also holds its whole `gpx_points`)
over live nodes of the petal carrying either key. `SetNodeView` is registered
`UnmappedDeferred` in the SPEC-8 mapping inventory; the load is a read and is
excluded per §inventory-scope.

**Visibility groups + scope eyes (`SetPetalVisibilityGroups` /
`GetPetalVisibilityGroups` / `SetScopeHidden`, hierarchy_visibility_groups
Phase 4, `handlers/visibility_groups.rs`).** The petal's Group registry is the
`petal.visibility_groups` column (`[{id, name, visible, order}]`, D-3) and the
verse/fractal/petal eyes are `view_hidden` (`true` or `NONE`, D-1 — the petal
eye is persisted too, so the chain has no session-only gap). Set follows the
lifecycle order: validate (≤ 256 groups, unique ids of 1-64 chars — the same
cap as a node's `view.groups` ids — names 1-80 chars trimmed, finite `order`)
→ resolve the petal/fractal/verse scope (missing row = failure, never an
empty answer) → `lifecycle_auth_context` → `fe_policy::authorize_node_edit`
(`Write` at ANY scope string, so it is reused rather than adding an
`authorize_scope_edit`) → `UPDATE` → reply. The registry write is a
WHOLE-registry replace: O(groups), and membership (`view.groups` on nodes) is
never touched — a deleted Group leaves dangling ids that hide nothing.
Set and Get both reply `PetalVisibilityGroupsLoaded` (Set echoes the stored
value); `SetScopeHidden` replies `ScopeHiddenSet`; failures are
`correlated_failure`. `load_hierarchy` reads `view_hidden` off its
`SELECT *` rows into the hierarchy DTOs so the sidebar hydrates on
`HierarchyLoaded`. The `*_reply` builders are `pub` so fe-api's tests serve
them on a simulated DB thread. Both writes are `UnmappedDeferred` in the
mapping inventory. Durable tests: `tests/visibility_groups_test.rs` (Viewer
denied + nothing stored, Editor round-trip, survives a DB-thread restart on
the same store, eyes reach `HierarchyLoaded`, missing rows fail).

**Store release on DB-thread exit.** After the command loop the thread drops
its `Surreal` handle and yields/sleeps briefly INSIDE its runtime so the
SurrealKV engine finishes closing; without that the file lock outlived the
joined thread (os error 33 on Windows) and an in-process reopen of the same
path — the restart tests above and `node_view_test.rs` — failed. Since OF-2
the router runs on the runtime's worker threads (§shared-handle), so the
yield/sleep gives that worker time to finish; the join after `Stopped` stays
the reopen barrier. A live shared clone defeats this close (§shared-handle).

**Promotion write-side contract.** `promote_instance_handler` writes
`properties: { node_kind: 'stamp', path_id, owning_path_id, instance_index }` —
`node_kind`/`path_id`/`instance_index` are fe-query's read contract
(fe-query/src/spatial_nodes.rs `KIND_KEY`/`STAMP_PATH_ID_KEY`/
`STAMP_INSTANCE_INDEX_KEY`); `owning_path_id` stays because the cascade BFS and
the reflow hook traverse it as the durable parent edge.


### Guard snapshots never project NONE columns (QA-0099, 2026-11-07)

A use-time fence `IF (SELECT a,b,c ...) != $snapshot` compares against a JSON
rebinding of an earlier read. An explicitly projected NONE column reads back as
JSON `null` and rebinds as NULL, and `NULL != NONE`, so the fence always THROWs.
Filter it in WHERE (`tombstone = NONE`) instead of projecting it — the tombstone
member fence (`commit_tombstone_atomic`) and the idempotent stamp-promotion fence
both broke every delete / re-promotion this way. A `SELECT *` fence binds `[row]`
(the SELECT is an array; `plugin_host` once bound the bare object).

### Transform register for legacy rows (QA-0100, merge.rs)

Unversioned node rows (no `_fe_scene` marker) are merged by `merge.rs`. Their
transform (position/elevation/rotation/scale) is an LWW register ordered by
`edit_seq` (local transform writes bump it) and then the value vector under
`f64::total_cmp`; non-transform fields keep the blind merge. Versioned scene rows
are ordered by the `(hlc, publisher)` scene clock in `scene_publication/inbound.rs`.
The replica snapshot feeds every author's `node/` entry (oldest first) so a
rejoining peer sees the register winner, not just the newest entry.

## §command-lane (QA-0143, 2026-11-07)

The DB thread is one serial queue, and under a multi-author placement burst it held 20-46
`ApplyReplicatedRow` commands at 300-750 ms each. A remote peer's scene-asset request waits on
`InspectSceneAssetTransfer { purpose: Serve }`, whose caller gives up after 2 s, so every such
admission timed out behind the backlog and the provider refused verse Editors as if they held no
role (run 4: refusals exactly 2 s apart on the owner, the other authors' GLBs 63-80 s late at
host CPU < 50 %). `command_lane::CommandLane` keeps the queue FIFO except for that one class:
it drains up to 128 commands into a local lookahead and hands a Serve admission out ahead of
the transparent commands queued before it.

- **Why only Serve.** It is a bounded read that answers a remote request. Fetch and Publish
  admissions depend on rows queued BEFORE them (the prefetch is triggered by the row that is
  still being applied), so they stay FIFO.
- **Security barrier (QA-0154, 2026-11-07).** A Serve admission at lookahead index i is taken
  early only when every command before it is *transparent* (`is_transparent`: reads, transforms,
  readings, device payloads). A `RevokeRole`, a role/ACL or replicated-row apply, an asset or
  node write, and any variant added later (the allowlist fails closed) is a barrier: the lane is
  strict FIFO up to it, so a blob is never served on pre-revocation or pre-retire state. The cost:
  an admission behind an `ApplyReplicatedRow` burst waits it out again; a timed-out admission is
  `RateLimited` (busy, wire code 2), never a role denial, and the requester retries.
- **No starvation.** After a command jumps the queue the next one is the FIFO head, so writes
  still get every other slot under a flood of requests.
- **Backpressure kept.** The lookahead is bounded (128), so senders still see a full channel;
  the slow-command log reports `queued` as channel + lookahead.
- **Deferred transform publication (QA-0161, 2026-11-08).** Measured on the run-7 stage: an MCP
  `update_transform` spent ~23 ms reaching the DB thread (HTTP + one reactive frame) and ~63-80 ms
  in it, replying the moment the command ended; four writes sent together queued serially, so the
  fourth answered after ~300 ms and the 0.5 s drone tick overran. A probe split the DB time into
  ~32 ms commit (with `authorized_node_scope` run twice) and ~27 ms `publish_after_commit`
  (re-read, fence transaction, blob, bridge). Now the transform write authorizes once
  (`node_history::commit_mutations_in_scope`; the lane is serial, so the role cannot move between)
  and `DeferredPublications` holds the publication: the arm commits, sends its `SceneChange` ack
  and queues the node id; the loop publishes every queued node (each once, its current committed
  state) when the lane idles, when the oldest waited 250 ms, or before any command outside
  `runs_before_publication` (transforms, IoT readings, device payloads and a few plain reads).
  Every other command, `Ping` and shutdown included, publishes first, so a replicated-row apply, a
  role/ACL change, a Serve admission or a scene-clock read never sees a committed transform
  without its publication. A burst of writes to one node publishes one row version.
  Since QA-0161 (2026-11-08) an idle lane publishes the waiting nodes one at a time and
  re-checks for commands between them (all at once only past the 250 ms bound), so a drone
  write that arrives during the previous tick's publications waits ~27 ms, not ~110 ms.
- **Superseded inbound versions (QA-0164, 2026-11-08).** Receivers applied one
  `ApplyReplicatedRow` per inbound row version (a node row ~88 ms, a reading ~16 ms), so the
  2 Hz four-drone flight (8 node versions + 8 readings a second) kept the editor's lane above
  100 % busy and the remote drone lag grew linearly to ~22 s with the DB channel full. While a
  command is drained into the lookahead, a peer's live complete node snapshot (`_fe_scene`
  marker, publisher = author, no tombstone) replaces the bytes of the same author's older queued
  snapshot of the same node, if one is reachable scanning back across only other node, reading
  and payload applies: it keeps the OLDER slot (so a reading or payload queued after the first
  version still finds the node) and the newer bytes. Any other command, a role/verse/fractal/
  petal/asset row, another author's version of the node, a tombstone, a partial (markerless)
  row, an older clock or a petal change stops the scan, and the version queues as before. This
  is the replica's own model: iroh-docs keeps only the latest entry per key and author, so a peer
  that syncs late never sees the intermediate versions either, and applying the latest complete
  snapshot converges to the same row. A backlog therefore collapses to one apply per drone, and
  `ApplyReplicatedRow:<table>` in the slow-command log names the table. The inbound rows queued
  right behind the one being handled apply in the same command (at most 64, `+N` in the log):
  each is judged exactly as before (`InboundAdmissions::apply`, role-at-write per row), and the
  batch echoes once, so a node's current projection is re-read once per batch instead of once
  per version. The rows are not merged into one SurrealDB transaction: each complete-snapshot
  apply is a fenced transaction whose guards read what the previous row committed, so the batch
  amortizes the intake and the projection, not the commits. The sender side stops
  rewriting unchanged rows (fe-sync `src/AGENTS.md` section "unchanged-rewrite").

## §shared-handle (OF-2, 2026-10-13)

**One SurrealKV open per process.** SurrealKV takes a file lock per open
handle; a second `Surreal::new::<SurrealKv>` on the same path in the same
process is refused while the first lives (Windows: `os error 33`). Both host
binaries used to open a second "read-only" handle for the API gateway after the
DB thread had opened the store, so on Windows the analytics/BI surface always
started without its reader. `spawn_db_thread_shared` fixes this at the source:
the DB thread sends a `db.clone()` (taken after `use_ns`/`use_db`, so the clone's
session inherits the namespace/database) through its existing startup channel,
and the caller gets it back as `DbThread::shared_db`. The handoff rides the
synchronous startup barrier — there is no separate wait or timeout, so a
started DB thread always yields a reader. `spawn_db_thread_with_sync_and_lifecycle`
(and the older shims) delegate and drop the clone.

**Runtime ownership (read from surrealdb 3.0.5 `engine/local/native.rs`).**
`Surreal<Db>` is a client: `connect` `tokio::spawn`s `run_router` on the
runtime that created the handle, and the router `tokio::spawn`s every query on
that same runtime; clients talk to it over `async_channel`s, which are
runtime-agnostic, so awaiting a clone from another runtime (the API thread's,
or a throwaway `block_on` for hydration) is sound. `Surreal::clone` registers a
new session through an unbounded channel that the router drains `biased` ahead
of routes, copying the source session's ns/db/vars. Two consequences:

- The DB thread's runtime is **multi-thread (2 workers)**. Its command loop
  blocks in crossbeam `rx.recv()` inside `block_on`; on a current-thread runtime
  that parks the router too, so every API read would stall until the next
  `DbCommand` arrived. With workers the router and query tasks run while the
  DB thread idles. Handlers still run one at a time on the DB thread itself
  (`block_on` runs the loop future on the calling thread), so the single-writer
  discipline is unchanged.
- The DB runtime must outlive every reader. It lives until the DB thread exits
  (app lifetime). After `DbCommand::Shutdown` the runtime drops at thread exit,
  dropping the router; a lingering clone then gets an error, not a hang.
- A live clone defeats the graceful close. The router only runs
  `kvs.shutdown()` when the last handle drops (its route channel closes), and
  `Surreal` exposes no close method. With the API still holding its clone, the
  engine is torn down by the runtime drop instead, and SurrealKV's file lock
  stays held until the process exits (verified: an in-process reopen then fails
  with os error 33). Both binaries exit right after shutdown, so this only
  matters in-process; durability rests on the per-commit fsync
  (`SURREAL_DATASTORE_SYNC_DATA=every`, set by both mains). Tests that reopen a
  store must not keep a shared clone alive. The pre-OF-2 binaries built the reader on a throwaway
  current-thread runtime that was dropped right after hydration, so even where
  the second open succeeded its router was already dead.

**Read-only is a convention, not a mode.** SurrealDB has no read-only handle;
the clone can write. The API keeps its existing write rules: mutations go
through `DbCommand`s to the DB thread, except the sanctioned append-only IoT
ingest (§iot-readings), which writes on `db_reader` when present.
Audit of the OF-2 review (2026-10-13), covering every `state.db_reader` site
in fe-api/src:
- Every site issues SELECT, `RETURN true` (readiness), or the guarded read-only
  query (`query_guard::validate_select_sql` rejects CREATE, UPDATE, DELETE,
  DEFINE, REMOVE, RELATE, INSERT and the rest).
- Elevated DML goes through `DbCommand::ElevatedQuery` and never touches the reader.
- The only write is `iot.rs` → `insert_readings_with_replication`, which is
  append-only with HLC stamping behind a Mutex and has no read-then-write.

A new write on the reader, especially anything that touches `node_log`
(§node-log invariant), must go through a `DbCommand` instead.

Tests: `tests/shared_handle_test.rs` (DB-thread write read back through the
clone on a foreign runtime while the DB thread idles; after shutdown the clone
errors promptly instead of hanging).

## §canon-log-materialization

The Canonical Fractal Data Log's SurrealDB half lives in `canon_log/` and is documented in
`canon_log/AGENTS.md`. That covers the verified log, admission order, rebuild and checkpoints,
epoch state, the W4-7 durable authority sources and production verifier, and the W4-8 device
enrolment registry. The legacy-to-canonical migration harness lives in `migration/` and is
documented in `migration/AGENTS.md`. §log-first-commit above still describes the only live write
path. `canon_log` is parallel and dormant, with one exception: `canon_device_enrolment` rows.
They are owner-decided (R2) ordinary replicated rows, written by
`DbCommand::EnrolLocalDevice` / `RevokeLocalDevice`. Inbound, they are admitted by signed
self-authorship, not role, in `handlers::replicated_row` (`canon_log/AGENTS.md`
§device-enrolment).

## Node property acknowledgment correlation

The three node property commands echo optional correlation IDs on their typed replies. Failures reuse handlers::correlated_failure: API requests get RequestFailed with the exact ID, GUI requests retain Error. Persistence, scene-change broadcasts and node-log append behavior remain shared. A persisted property must acknowledge independently of which node the GUI currently selects.

## §property-schema

`field_def.enforced_schema` is optional: absent means legacy metadata, present
means the SDK bounded property-schema contract. SetFieldSchema requires
consecutive revisions and retains them in `schema_revisions`. It validates the
stored default and every present value in the resolved descendant scope before
changing anything (10000-node dry-run cap). It runs on the sole serialized DB
command writer; no shared-reader or concurrent handler use. No implicit migration
or backfill. Every applicable ancestor rule is enforced conjunctively by ordinary
SetNodeProperty; descendant metadata cannot weaken it. Outer properties remain
optional: DELETE removes them, nested required checks object members, explicit
null needs permission. Legacy UpdateFieldDef cannot change an enforced type and
must validate replacement defaults; DeleteFieldDef refuses enforced definitions.
The default column now accepts arbitrary JSON types; defaults remain suggestions.
This contract covers ordinary property commands, not arbitrary operator SQL or
replication. Those paths must not be represented as validated ordinary writes.

## §node-history

Ordinary empty/asset creation now atomically writes a complete `baseline` v1.
Native GLTF import uses that same sealed creation boundary, including its new
Asset metadata in the transaction and recording the actual local actor and scope;
see handlers/AGENTS.md Ordinary creation and imported assets.
Property set/delete and full transforms atomically commit the materialized node,
legacy operation intention, and INSERT-only node_log facts through
commit_operation_atomic_with. Their previous dispatcher best-effort append is
removed. A unique (node_id,row_version) index protects per-object commit order;
edit_seq remains transform confirmation metadata, independent of this order.
This remains the ordinary local journal: canonical migration/signature machinery
is unchanged and dormant. The sole serialized DB command writer remains required.

Covered payloads carry journal_version1, retained resolved scope, typed operation
data, full after_state and applicable field-definition revisions. Baselines store
complete node-table state, with XYZ position normalized from the stored XZ point
and elevation; external model/blob contents are not versioned by this journal.
Before continuing coverage, the handler compares the current normalized row with
the last covered after_state. A legacy fact or changed row starts a new baseline
in the same mutation transaction. Legacy facts are retained as audit evidence;
unsupported rows/gaps refuse replay, and later baselines do not fabricate the
missing interval. Snapshot payloads are capped at65536 encoded bytes. Replay reads
at most10000 facts in200-row batches; page limit1..200. API response byte caps also
apply. Versions are per object, never a global time cut. created_at is server
record time; HLC is ingest metadata, not measurement/effective time. This first
contract accepts no client mutation effective-time override. Numeric readings
retain their separate existing recorded_at event-time contract.

ReadNodeHistory returns coverage metadata, immutable cursor pages, or a pure
baseline+property/full-transform fold for one exact version. Availability is
available/before_coverage/incomplete_interval/unsupported_operation/version_unavailable;
unavailable replies omit state. Reads never mutate state or activate replay.
CommitNodeMutation/CreateObject carry actor from the validated API principal;
ordinary GUI dispatch uses local identity. New object GUI birth replies carry
fresh unknown correlation IDs, so they cannot consume unrelated API create
waiters; the committed QueryResult receipt has its own matching request ID.

All ordinary property/default/schema writes reject unsigned integers above i64MAX
before append, preventing silent storage rounding. Signed integers beyond2^53
remain exact JSON integers. SDK numeric bound comparison is exact for integer
pairs; mixed float/integer comparisons outside exact f64 precision refuse.
Arbitrary elevated/replicated/import/clear/rename paths are not claimed as covered
operations. Comparing row state detects an unjournaled interval before the next
covered write; current_state_verified reports whether the latest fact matches the
current row. Current state can differ while older committed versions remain valid.

### Node-history grant binding and historical scope

The dedicated CreateObject/CommitNodeMutation/ReadNodeHistory commands carry a
trusted NodeAccessGrant: Local for internal desktop calls, or Scoped with the
validated token scope. The enum is not deserialized from request bodies. Grant
containment compares parsed verse/fractal/petal identifiers, never an identifier
prefix. Dedicated writes check the current stored ancestry before preparing an
operation and again before committing; fixed bound SQL fences verify the exact
node/petal/fractal/verse chain inside the same transaction. A queued reparent or
ancestry change refuses without durable state, intention, history, or sequence
advance. A scope change starts a fresh coverage baseline even when the node row
itself otherwise stayed equal.

Read admission is rechecked on the DB writer using current ancestry or the latest
retained covered scope after deletion. Every returned fact/baseline is separately
authorized by its own retained scope and matching node/petal snapshot identity.
Foreign facts and legacy facts without verified scope become static
unavailable_interval markers: only requested node identity and cursor version are
retained, with no original payload, log id, actor, HLC or timestamp. Replay refuses
an unauthorized baseline or interval with unavailable_scope and no state. Coverage
start metadata is omitted when the initial baseline is outside the caller grant.
The local desktop must currently hold Viewer at each retained fact scope;
unknown-scope and no-longer-readable payloads are redacted. Historical reads never infer ownership from
only the object's present location.


### Local identity and role floors

Local is an internal routing mode, not an authority grant. The DB command loop
resolves Local actors from its actual local_did and ignores the command actor_did
label. The committed core and ordinary local compatibility write helpers require
Editor through the existing require_node_role/node_role_at path at the actual
stored scope. This includes create, property set/delete, and full transform.
Node-view writes carry their already-authorized real subject and scope through
the same core, without a literal local placeholder. Scoped API requests retain
their validated upstream token-role behavior; opaque API subjects are not treated
as local users or looked up as local DIDs.

Local history admission, each retained fact/baseline, and version replay require
current Viewer permission for the DB-resolved local identity. Revoking access to
an earlier scope redacts those facts even while the local user can read the
object's current scope. Local SetFieldSchema requires Manager at the stored
definition scope; GetFieldSchema requires Viewer. Both dedicated commands carry
NodeAccessGrant, and API constructs Scoped from validated claims. Ordinary
metadata-only legacy definition constructors stay compatible; active enforced
definitions keep their earlier update/delete protections.


## §hierarchy-mutations

Hierarchy rename, description update and physical subtree deletion require
Manager at the target's stored scope, using the DB thread's resolved local DID.
CreateFractal and CreatePetal use the same preparation at the stored parent.
`handlers/entity.rs::ParentMutation` snapshots the owner's/default-access inputs
and the actor's exact ancestor role rows. Permission is derived solely from
those captured inputs through the shared pure role resolver, never a separate
live role read. Its transaction fences those inputs and every stored ancestry edge
before any write. Missing, moved, ambiguous or no-longer-authorized targets fail
without partial changes. SQL identifiers come only from the EntityType match;
caller names and identifiers are bound parameters.

The cascade executes all node, petal, fractal and existing verse cleanup in one
SurrealDB BEGIN/COMMIT transaction. Membership links use exact IDs and SELECT
VALUE subqueries. Verse role cleanup compares parsed verse IDs, so a similarly
prefixed verse's roles survive. The old physical-delete boundary is retained:
it does not publish P2P tombstones, emit per-node lifecycle events, delete retained
node history, or claim ordinary replay coverage for structural deletion. Peers
can retain old structure; subscribers must reload after structural changes.
Existing role-grant statements and non-verse role cleanup are unchanged.

EntitySettings' navigation-scoped cached role is not authoritative for another
tree target. The DB check is authoritative; target-specific dialog role UX is a
separate UI task. Rename, description and delete channel failures are logged and
shown through the shared notice surface; failed sends retain the delete dialog.

## Fenced hierarchy details reads

GetEntityDetails uses the same prepared ancestry/role snapshot as hierarchy
mutations with a Viewer floor; Manager remains the floor for mutations and
child creation. The shared pure snapshot resolver returns the captured role.
Every ancestry/authority guard runs before the selected-field SELECT in one
explicit transaction. SQL identifiers/projections come only from EntityType,
and caller values stay bound. The DB thread supplies local_did; callers cannot
assert an actor. The read retains missing default access as absent and returns
only valid viewer/none values, optional description, actual name and scope.
Fields are bounded at 64 KiB. Failure returns a correlated static unavailable
message; detailed database errors remain in local logs. The response index is
the number of preceding guard statements plus one: pinned SurrealDB emits a
NONE result for BEGIN at index zero, then each guard, SELECT and COMMIT. The
SDK retains those indexes. Existing production-dispatcher settings journeys
assert the selected fields, so an empty guard result cannot count as readback.
## Scene materials

`handlers::scene_materials` is a bounded local Viewer read, not RawQuery. It checks expected scope against stored
Petal ancestry, rejects globally ambiguous hierarchy/node IDs, authorizes Viewer, selects only material/primitive
presence under exact live ancestors, then rechecks ancestry and role. Unknown/moved/foreign uniformly fail.
At most64 IDs and4096 UTF-8 JSON bytes per preview return; oversize remains explicitly truncated/unavailable.
This read changes no object, operation journal or history. Full editing uses the existing typed property flow.

## Plugin host

PluginRead and PluginCommit carry host-minted PluginAdmission (no Deserialize)
and embedded typed reply senders, so plugin results never share API waiters.
The initial scope handshake pins a full stored petal ancestry; subsequent requests
refuse reparented scopes. Every use checks admitted capability/property/scope
patterns plus the actual DB local DID's current Viewer/Editor floor. The shared
hierarchy permission snapshot now accepts a role floor; parent mutations retain
Manager and identical snapshot guards. Reads fence that exact authority and
ancestry inside their read transaction and recheck permission after materializing.

A plugin batch stages its complete ordered changes before writing. Property
validation uses the exact captured field definitions, then fences those rows,
initial node fields/geometry, journal cursors and captured authority inside the
same existing atomic operation transaction. Repeated changes on a node use
node_history::prepare_change, sharing the ordinary baseline, reducer, size and
monotonic-version contract. A PluginTransaction intention records the actual
ordered batch; schema, state, intention and journal either all commit or roll back.
The host never uses ElevatedQuery or caller SQL for this path.

V1 DeleteNode is a real soft tombstone for plain nodes. Path/stamp nodes or nodes
with dependent children refuse before effects, including dependencies introduced
by another staged operation. Its plugin_tombstone journal fact is explicitly
unsupported by historical replay; the earlier supported baseline stays readable.
No tombstone replay coverage is invented. Post-commit publishing uses existing
scene, lifecycle, GUI materialization/cache and P2P row seams. PluginNodeRemoved
is GUI-only and cannot claim an API NodeDeleted waiter. Replication remains the
existing best-effort transport, not a guarantee of peer delivery or distributed
transaction atomicity.

Plugin schema capture is restricted to exact ancestor scopes, capped at 256
enforced definitions per key. Expanded staged states/facts are capped at 2 MiB
before effects, in addition to the 64 KiB input and per-snapshot bounds.


### Schema revision array migration

Field-definition revision history is an optional array of descriptor objects. The pinned database parser requires an explicit object-containing type for FLEXIBLE; a bare array or any is insufficient. Reapply the outer array<object> declaration and its wildcard object element with OVERWRITE so existing roots retain arbitrary nested descriptor fields. Default values use the explicit optional JSON union to preserve scalar and compound representations. Boot and persisted roundtrip evidence are required in addition to source review.


## §plugin-host-flush

The DB thread's `PluginCommit { admission, ops, readings, reply }` arm calls
`handlers::plugin_host::flush` and nothing else (I-DB 2026-11-09, closing G5-C5 and G6-C3;
the same slice's `handlers/mod.rs` lines close G6-C2 and G2-A7). `flush` takes `FlushSinks`: the GUI result channel, the scene feed, the lifecycle
seam, the blob store, the replication sender and the loop's `DeferredPublications`. The
order is the R6 whole-flush rule:

1. **Refuse up front.** A revoked lease, an invalid batch, or a reading anchored on a node the
   same batch deletes is refused before anything is written. The door then validates the
   readings against the bound petal.
2. **Commit the scene batch atomically.** A readings-only flush still re-checks the Editor
   floor and the pinned ancestry.
3. **Publish what survived.** A created, deleted or property-changed node is published at
   once. A transform-only node goes on the deferred lane, exactly like `UpdateNodeTransform`
   (QA-0161), so a tick that moves 64 nodes does not hold the serial queue for 64
   publications.
4. **Land the readings** through `insert_readings_with_provenance` as the admitted source
   class, with the package stamp (R24).

Every journal fact the batch writes carries `payload.ext = {package_id, content_hash}`
(`stamp_provenance`), the created node's `baseline` fact included: a plain create writes no
other fact, so without it that node had no provenance at all (R-BND). Decoder stamping
(`payload_id`, `source_class: decoder`, bindings.md §6 `ingest.decode`) has no carrier on
`PluginCommit` yet: `StagedReading` holds neither, so decoded readings land as the grant's class
with no payload link until the seam gains one.

If the scene batch committed but its readings failed, the reply is `unknown-outcome`, never
a retry instruction. The old arm destructured `BatchOutcome.nodes` as a 4-tuple (E0308 after
E4) and never called `flush`, so no staged reading ever reached the DB. Both plugin arms now
answer the typed `PluginResult`: `PluginRead` maps its failure through
`plugin_host::refusal`, which keeps the WIT class.

## §extension-governance-commands

The DB arms for the governance commands (fe-runtime `src/AGENTS.md` §extension-db-commands)
are thin. Each one parses the JSON payload strictly, calls `handlers::extension_policy`, and
answers with the handler error's `Display` (`invalid-policy: …`, `role-denied: …`,
`confirmation-mismatch: …`, `invalid-input: …`). They act as the DB thread's own `local_did`:
the policy row's author and the override's Owner check both use it.

- **`AppendExtensionAudit` refuses `kill-list-override` entries.** `load_kill_list_overrides`
  rebuilds overrides from ok audit entries at startup, so an appended one would hydrate as an
  override without `record_kill_list_override`'s Owner and confirmation checks. The arm logs
  a refused append at warn, because a lost audit entry matters. It ignores a closed reply:
  the bus audit forwarder drops its receiver at once.
- **`ListExtensionAudit` reads by installation, then by verse, then the whole trail.** With
  both filters it narrows the installation's rows to that verse. With neither, it reads the
  whole trail newest first, capped at `AUDIT_TRAIL_READ_MAX` (10,000; per-subject reads keep
  the handler's 500), for the binding worker's acceptance replay. The cap fails closed: an
  acceptance older than the window reads as not accepted.
- **`LoadExtensionGovernance` answers `{policies, consents, overrides, acceptances}`.**
  Consents are sorted by installation. `acceptances` is the lossless `load_acceptances` replay,
  so the host can hydrate "Allow in verse" without the whole-trail scan.

## §extension-binding

`handlers/extension_binding.rs` is the DB plane of scoped extension bindings (docs/extensions/bindings.md
§3-§4; slice BND-1; declared in `handlers/mod.rs` and dispatched by I-DB 2026-11-09).

- **Storage.** Table `extension_binding` holds one live version per doc key
  `binding/{scope}/{hook}/{key}/{author_did}`. The index columns (`binding_key`, `verse_id`, `scope`,
  `hook`, `author_did`, `mode`, `package_id`) exist only for lookups. The canonical `payload` JSON is
  the truth on read, and a row whose payload does not parse is skipped with a warning.
- **Local write** (`PutBinding` → `put_binding`) runs in this order:
  1. Shape the row: canonical scope, a v1 hook, a key that fits the hook, a manifest v2 package id,
     a SemVer `version_req`, and a DID for `runs_on`.
  2. Check that the scope is a known verse, fractal or petal.
  3. Judge the remote caller (`caller_did` other than the local DID): it needs Viewer+ there, or it
     gets the uniform `not-found`, and Manager+ for the write.
  4. Check that the local DID itself holds Manager+, because it authors the row.
  5. For an `extension` row, check the replicated verse policy at package level.
  6. Commit ONE `ExtensionAudit` op and the row atomically. `written_at` is that op's HLC, and
     `author_did` is host-stamped.
  7. Publish the row.

  A Manager+ refusal is audited `refused`. A local write never defers.
- **Inbound rows.** `confine_inbound` checks the structure: the key and payload agree, the author is
  the attested author, and the scope is in this verse. Then the Manager+-now gate runs.
  `apply_replicated_binding_row` bounds the author-stamped clock (`admit_scene_hlc`), then keeps the
  newest `(written_at, payload bytes)` per key.
- **Judging happens on read, and demotion deletes nothing.** `judge_versions` marks a version
  admissible when its author held Manager+ at the scope when it wrote AND holds it now. The newest
  admissible version per point is effective. A demoted author's rows stay stored for audit, but a
  re-grant does NOT revive them (bindings.md §4.5): the write-time half reads
  `role_grants::authorized_at_write` over `fe_policy::materialize_at`, and a statement's continuity
  (`manager_since`) restarts at the re-grant, so only versions written inside the new window are
  admissible. The same rule withdraws a version backdated into the author's old window. The
  current-role half is what stops a demoted author from backdating while still demoted (an author
  stamps its own HLC). Keep binding (`keep_binding`, `kept_template`) re-authors a withdrawn version
  as the clicking Manager with its effective `runs_on` pinned; it has no DB command yet (R-BND).
- **Re-emit.** The DB thread re-publishes this node's own live versions at startup (with every verse)
  and on `ReemitLocalRoleStatements` (per verse), beside the role-statement re-emit. That is the
  second chance for a publish dropped on a full bridge.


## scoped-query

DbCommand::ScopedQuery is the desktop SQL input path. Query tab, generated GIS
filters and generated Paths listing send required correlation IDs and requested
petal IDs. Every target is resolved and authorized from stored ancestry/roles
using the DB thread's actual local identity at Viewer floor. Requested IDs and
LocalUserRole are not authority. Existing ParentMutation stored-policy snapshots
are reused; trusted fence bindings are renamed by target index so every member
is checked in one read transaction. Changes to owner, default access, role rows,
ancestry or identity cardinality invalidate the plan. Unknown, denied and
ambiguous targets uniformly return not found or not permitted. One authorized
target never authorizes another member; no special Owner bypass is added.

Shared prepare_scoped_sql confines every allowed source before user predicates.
Desktop queries refuse API-global metadata sources (verse/fractal/field_def/
crate_entry) and permit node, iot_reading, device_payload, petal, room, model and
crate_registry only. Native surrealdb-core 3.0.5 parses the exact executable
SELECT with its 5s TIMEOUT and checks one statement. It also checks generated
transaction boundaries. IndexedResults is checked and decoded at the single
known SELECT slot. Engine/decode failure, unexpected statement count, partial
data, row cap and byte cap produce correlated QueryFailed, never empty success.

Bounds: 16 distinct conservative ASCII petal IDs, 128-byte IDs, 64 KiB SQL,
64 declared bindings with 64-byte names, 16 nesting levels/8192 values/64 KiB
JSON, 10000 result rows and 8 MiB result bytes (the existing /query ceilings).
Binding depth/count/string limits are checked iteratively before serialization.
Every parameter reference must be declared; server controls and protected session
names cannot be bound. Shared comments/record-constructor/source restrictions
remain. Desktop additionally refuses caller TIMEOUT, FETCH, VERSION, WITH,
PARALLEL, EXPLAIN, TEMPFILES, FUNCTION and FUTURE clauses. A quote-aware lexical
pass permits count, array::len and the named math aggregate/scalar functions
listed in the handler. Custom/external/session calls and allocation/range/repeat
generators are refused. Function-looking text in strings is not code. This is
the initial conservative desktop dialect; expansions belong in the campaign
decision ledger and require security review, not automatic fallback.

Preparation/execution have a 7s async backstop; execution also has a 6s backstop
around the 5s DB statement limit. Row/byte limits are checked after strict
decoding, consistent with the existing API path. These are not pre-allocation
memory isolation or a claim that async timeout preempts arbitrary synchronous
allocation; generator functions are refused for that reason. RawQuery retains
trusted/server-built API fallback, internal scope/GIS reads and laboratory
contracts; the desktop table whitelist is not imposed on those callers.

The existing real-store journey covers both data planes and nested properties,
foreign predicates/OR/subqueries, dialect/parser/binding refusals, no-read denial,
Viewer success, all-target denial and revocation between plan capture and
transaction execution. It also captures a request before revocation and
dispatches it afterward to exercise point-of-use authorization. Tests, exact
timeout outcomes, fresh GUI proof and independent security review remain pending.

The token gate also refuses unquoted slash/backslash, range syntax, closure
pipes, non-ASCII tokens and dynamic expression/parameter calls. This avoids
quote confusion inside regex literals hiding later calls from a quote-only
scanner; native parser success/count alone cannot establish function policy.
Quoted Unicode/string values remain usable. Slash refusal also excludes desktop
division syntax in this initial dialect. Regex/division/lambda expansion is a
decision-ledger follow-up requiring a reviewed lexer/engine contract; trusted
programmatic API/report queries retain their existing behavior.

The desktop gate also refuses graph arrows and global metadata identifiers
(verse, fractal, field_def, crate_entry) anywhere in SQL, including projection
record literals. FROM-only source rewriting cannot bound those record/graph
reads. Quoted metadata words are conservatively refused too; any wider desktop
dialect belongs in the reviewed decision ledger. API guard semantics and trusted
programmatic callers remain unchanged. The existing refusal journey covers these
projection and incoming/outgoing/bidirectional graph forms.

Security follow-up: desktop lexical admission refuses every unquoted <,
including all native casts, versioned ML calls and less-than operators. Quoted
angle text remains data subject to shared forbidden-word restrictions. This closes cast-to-Table iterator sources and
ml::name<version>(args), which does not use the usual name-followed-by-parenthesis
function spelling. Less-than operators/casts are explicit decision-ledger
restrictions pending a reviewed lexer; callers can currently use reversed >
comparisons. The shared API guard separately refuses whole-word TABLE/RECORD
(including data/field uses) to close nested capability casts without removing
ordinary API comparisons. Existing real-store and pure refusal journeys were
extended; source-only validation is not runtime proof.

Replica catalog restart repair lives in handlers/local_catalog.rs. CreatePetal
now publishes only after its authoritative commit; replica open requests a
stored Manager-authorized, ancestry-fenced catalog re-emit through the same
replication seam. See handlers/AGENTS.md local-catalog for limits and fields.
Existing durable role-restart journey also checks committed Petal publication,
Verse/Fractal/Petal recovery ordering, exact identities, private-field omission
and Viewer no-publish. No readings, history, schema, blobs or journal facts are
re-ingested by this command. Execution and live restart proof remain pending.

Petal declaration persistence lives in handlers/petal_requirements.rs. It reuses
the ordinary atomic op-log seam, exact stored Manager/ancestry authority, raw
document CAS and existing catalog allowlist. There is no DDL change or implicit
residency/install policy. See handlers/AGENTS.md section Petal requirements.

## Token mint reply correlation

MintApiToken and ApiTokenMinted echo an optional correlation ID. Inspector
requests carry a process-unique nonce; legacy dialog/scenario callers use None.
The DB door uses its existing correlated_failure helper, including a missing
keypair, without changing authorization. PendingApiRequests has a dedicated
mint reply family and exact-ID routing; a late/unknown mint never falls back
to another waiter. GUI request IDs are consumed only by their owning form.

## Local role admission and legacy token revoke

Local role statements commit their operation and admitted row atomically under captured Manager/ancestry, exact verse-owner and prior statement-row fences. Ordering still uses latest_per_grantor. Ordinary repartition/materialized-role update and publication follow commit; startup repair retains its existing responsibility. This is not distributed transaction atomicity or synchronous peer delivery.

The RevokeApiToken constructor census found GUI EntitySettings/Inspector and test peer/scenarios only; REST/MCP/WS has no producer or pending family. Legacy local self-revoke derives authority from the DB-thread DID and treats supplied sub only as target data; a mismatched sub refuses. The test peer mirrors this rule. Explicit target Manager-revoke manages a descendant token under its separate stored-authority fence. Remote API principals are not substituted with host identity.

## seed-petal

`queries::create_petal` is the seed-only constructor used by `handlers/seed.rs` to create the Genesis Petal before attaching its stored Fractal ancestry. It retains the caller argument for compatibility but does not infer ordinary interactive creation authority from that argument. Normal hierarchy creation uses the checked target handler; do not expose this bootstrap helper as an alternate authorized mutation route. The unused explicit-only `get_scope_members` helper has been removed; current Access reads retain their stored target, inherited and converged-role readback contract.

## Terminal development reset barrier

Legacy reset commands retain their live-host refusal. TerminalDevelopmentReset is a host-only, nonserialized command carrying single-use quiescence proof and exact RESET confirmation. The DB worker retains its startup developer capability and repeats the existing actual global Owner, allowlisted inventory and transaction snapshot checks. It emits only TerminalDevelopmentReset, then exits the command loop on either refusal or completion. It never emits legacy DatabaseReset/LoadHierarchy and never executes a queued post-barrier mutation. Clear-plus-seed failure remains distinguishable from refusal before clear.

The existing shared-reader exit journey now drives a real bounded worker through prior writes, a full result channel, final reset/seed, an already-queued late write, actual join, retained-reader failure, same-path reopen and consumed-proof refusal. Named fake service owners exercise the proof boundary in that isolated test; real desktop service shutdown remains a separate live acceptance gate.

## api-auth-lifecycle

api_auth_state:current holds one private local credential generation. Schema startup creates legacy compatibility only when adding the table to an existing or fresh store; an existing table with a missing/invalid singleton refuses startup. A valid generation is literal legacy or a canonical uppercase ULID. No identity/signing key is rotated. Ordinary token persistence and protected dev-file issuance use fe1.<generation>.<canonical ULID> JTIs. The ordinary mint transaction fences the generation captured before signing. The dev writer verifies its generation again before protected-file publication and remains part of the reset stop/join proof.

Terminal reset recognizes this private metadata table separately from DEVELOPMENT_RESET_TABLE_NAMES. It fences the captured generation and global ownership, atomically replaces the generation with a fresh unpredictable ULID, clears/checks every content table, and verifies the replacement before commit. Failure rolls all of that back; ordinary schema reapplication/restart never resets the generation. The table is absent from public Query/elevated table allowlists, projection/admin dumps, and replicated-row admission. It is not P2P authority or scene data.

A pre-reset legacy JTI remains compatible until the first reset, with its stored revoked flag still enforced. After reset, old generationless and earlier-generation JWTs are refused, including unpersisted Owner/dev credentials. Current-generation unknown JTIs remain allowed because dev credentials intentionally have no api_token record. Existing string storage, exact tuple comparisons and the Access 256-byte bound already accept the longer JTI; there is no workspace JTI-specific 26-character parser. Other object/ULID grammar is unchanged. Low-level identity example and harness mints are legacy-compatible signers, not production generation authorities.

Existing real-store shared-handle and schema reset journeys now assert persisted ordinary revocation after reopen, generation preservation on ordinary restart/refusal/schema reapplication, terminal rotation, old Owner retirement under the same verifying key, and a fresh DB-thread mint. These are authored source assertions until the integrated sweep.


## Join invitations

`join_invite` defines the versioned Owner-issued, recipient-DID-bound envelope.
Fixed-order, domain-separated signed bytes include the exact owner manifest,
scope, role, times, nonce and opaque native document ticket. Parsing is bounded
and rejects unknown fields, noncanonical identities/encoding and unsupported
versions without legacy fallback. Signature verification does not import a
document or prove a live peer: native ticket capability, addressing and expiry
are independently rechecked by sync admission before effects.

The first producer supports Owner issuers; Manager issuers require a separately
verified owner-rooted delegation chain. An addressed envelope does not encrypt or
make its enclosed bearer document ticket nontransferable. Envelope expiry bounds
acceptance, not the lifetime of a copied transport capability. Whole-Verse read
access is not subtree confidentiality, and role revocation does not erase bytes
or capabilities already received. Cross-store admission is explicitly staged;
parsing or local row creation alone must never produce joined readiness.

The canonical pure native ticket parser now lives beside the envelope in
`join_ticket`, with existing-workspace iroh/iroh-docs dependencies. It has no
network, engine or service effects. fe-sync re-exports this one parser rather
than providing a second policy. VerifiedJoinInvitation retains a parsed ticket;
invalid namespace/capability/issuer/address data therefore refuses before issuer
grant or recipient metadata writes, without trusting a GUI boolean.

The envelope also signs the actual ordinary issuer grant HLC. Readiness requires
the owner's normally replicated, nonrevoked grant at least that new clock plus
actual effective role and ancestry. A public default Viewer or older cached grant
cannot fabricate convergence. Manager issuance still requires its owner-rooted
delegation proof and is not enabled by this Owner-only slice.


## Join-delegation

join_delegation is a pure, transport-free certificate module for legacy hierarchy Manager
authority. It does not use the canonical-log interim authority adapter, which lacks portable
legacy scope history. Certificates contain no invitation envelope or DocTicket. Each signed
certificate binds a canonical audience DID, exact Manager grant HLC, canonical full scope,
immutable Verse id/Owner/creation/namespace, issue/expiry times and its parent proof content
hash. Domain-separated fixed-order signing and content hashing are distinct. Display names
and mutable manifest settings are not identity anchors. Existing Owner-v1 admission/readiness
already ignores name changes while pinning those immutable fields; a real-store journey now
asserts rename preservation. Prepare-to-issue still intentionally fences the captured source.

A verified chain has an Owner root, matching successive issuer/audience identities, no cycles
or Owner recipients, maximum depth eight and 16 KiB serialized proof budget. Scope and time
can only attenuate; expiration is bounded by the existing thirty-day invitation ceiling. An
issuer at the depth bound may not create another Manager proof. These limits govern portable
invitation authority and do not shorten or extend ordinary role access or erase capabilities.

VerifiedDelegationChain seals signature verification only. Its SupportingGrant records are
requirements for current ordinary role statements, never fabricated grants or DB authority.
The DB integration must require exact nonrevoked Manager statement HLCs, current effective
authority and ancestry, and fence the proof rows and grant inputs at use in the same issuance
transaction. Received proofs remain quarantined until actual Join readiness. Existing Managers
without a portable proof require explicit Owner reauthorization (DQA016); cached role rows or
unsigned local op records are not portable evidence. At this checkpoint the module is dormant:
Owner-v1 remains the active envelope/producer and no Manager issuer is enabled.


### Dormant addressed-v2 envelope adapter

The pure issue_addressed/parse APIs support an Owner or delegated Manager issuer. V2 signs
the full existing envelope plus its exact issuer chain and optional recipient Manager chain
under a distinct domain and URI version. V1 signed bytes and parse_owner remain compatible;
the active DB/UI producer and consumer still use v1 at this checkpoint. Prefix/envelope
version mismatches and delegated fields on v1 are refused without fallback.

RecipientManagerProof can only be obtained from a fully verified v2 Manager-recipient branch.
The recipient certificate must extend the exact signed issuer chain and match the envelope
issuer, recipient, scope, grant HLC and issue/expiry times. Viewer/Editor envelopes cannot carry
a recipient Manager proof. Manager issuer validity is bounded by its current proof interval.
This sealed transport-free branch is the intended DB quarantine/finalization input; it avoids
a host boolean claiming authority or an unrelated separately supplied certificate. It proves
no current stored grant and no native readiness by itself. Production activation is deferred
until the separate DB/current-authority and coordinator slices receive independent review.


## Join-authority

join_authority owns local detached-proof quarantine, current Manager authority snapshots and
recipient proof finalization. It is a dormant DB adapter at this checkpoint: active DB/UI
commands still use Owner-v1. The private module and v2 adapter entry points temporarily allow
dead code until the independently reviewed coordinator integration. No native-ready boolean
is accepted and no DB function claims transport readiness.

join_delegation is a separate local table, not a role or replicated projection. Admission of
a verified v2 recipient-Manager branch places its exact signed proof, acceptance nonce and
envelope signature into quarantine in the same metadata transaction. The proof is not active
authority. The prepend adapter places its immutable-identity/capacity/expiry guards before
Verse/member effects; all steps still commit or roll back together in that one store.

ManagerAuthority reads raw proof strings only after a 16 KiB byte check, strictly decodes the
canonical proof, pins its content identity and validates signatures against immutable Verse
identity. It captures bounded, ordered admitted/held role statements (8192 each), rejects
malformed or duplicate live keys, derives the ordinary fe-policy grantor fixpoint, and requires
every SupportingGrant to be the exact nonrevoked Manager statement HLC. An unchanged role cache
cannot revive an older certificate after revoke/regrant. Every certificate holder scope and
the requested target also use normal stored ancestry/cache Manager fences. Up to 32 active
candidate proofs and 64 unexpired stored proofs per holder/Verse bound local cache work.

The same proof row, all admitted/held inputs, immutable identity, expiry, cached authority and
full ancestry snapshots are appended to the ordinary checked grant transaction. The real
Manager remains the grantor and only the actual committed HLC is returned. The eventual issue
handler must preserve the existing explicit partial-outcome boundary after a stored grant.
Preparing a snapshot is not a claim that it remains current before those fences run at use.

The bounded production finalize_before can only obtain RecipientManagerProof from a verified v2 Manager-recipient branch.
It rechecks the current supporting grants/effective Manager authority, exact pending proof and
acceptance membership in the activation transaction. The coordinator must call it only after
actual native/role/hierarchy readiness, correlate cancellation/revision/deadline, then inspect
native readiness again before navigation. That coordinator phase is not wired at this checkpoint.
Cancellation after accepted activation is a truthful stored partial outcome; a proof alone is
never sufficient for later issuance, which rechecks all current local store inputs. This is
not a promise of instantaneous global revocation. Certificate expiry does not expire roles.

Schema reset/admin clear include the local proof table. The legacy replicated table-name set
excludes it. Dumps may preserve public certificates for inspection, but import deliberately
skips local proof activation state; imported Managers require Owner reauthorization. Existing
real-store journeys now cover quarantine without a grant, ordinary grant convergence, stale
proof/grant rollback, exact-HLC reauthorization, actual Manager-authored grant and import refusal.
These are authored assertions only until the parent integrated sweep.


The current dormant v2 handler separates signed issuer-chain readiness from local active-proof
authority for issuing new invitations. CurrentChainAuthority derives ordinary effective roles
and carries read fences; ManagerAuthority additionally requires a current active stored proof.
Independent chain snapshots rebind both delegation and holder-ancestry parameters before they
share a readiness read transaction. Prepared/issued InviteSource values now carry the actual
Owner or delegated Manager proof identity, and fresh QA source comparison includes that type
separately from mutable display metadata. Production dispatch remains Owner-v1 for this review.

## Committed node replica recovery

handlers/scene_publication/AGENTS.md is the authority for the versioned full-node publisher/receiver,
node_scene_clock local materialization watermark, receive-to-local HLC causality and bounded startup
repair. Original local journal/op provenance is required; full node state is not NodeDto or historical
replication. Explicit development reset clears the local watermark; admin projection lists omit it.


## Native Petal navigation read foundation

`handlers/native_navigation.rs` reuses prepare_parent_access at the Viewer floor
and its guarded read transaction. Only the actual DB worker's local DID is used;
no issuer token, client actor or synthetic role is accepted. Stored Verse/Fractal/
Petal ancestry and the exact rendered name must agree. The response contains only
that bounded target and a BLAKE3 fence of its sampled row/scope/effective-role/
local-actor binding; no grants, token metadata, properties or descriptions leave
this read. The fence is opaque sampled metadata, not a capability or atomic
revocation guarantee. The shared transaction fences the sampled authority and
ancestry at use. Missing, duplicate, moved, renamed and denied targets refuse.

The target SELECT is limited to two rows with a 500ms statement timeout. A one-
second outer await bounds the dispatcher's wait, not engine cancellation. Native
proof lifetime starts earlier at original UI-owner admission and cannot be
extended by DB execution or reply arrival. This slice adds no second DB opener,
write, public endpoint or automatic navigation. Existing hierarchy-thread
journey coverage is extended; it remains authored rather than executed here.

## §write-time-role (QA-0131, critic 2026-11-07)

A scene row is admitted only if its author holds Editor at its scope NOW (the existing gate)
AND held it when the row was WRITTEN (`role_grants::written_while_authorized`). Without the
second check a revoked editor's rows are admitted by any later re-grant: on the Deferred
retry, on a restart replay and on the empowerment re-read.

- **Write time is the snapshot's `_fe_scene` HLC.** `fe_policy::materialize_at` rebuilds
  the role view from the admitted statements as of that clock. Each statement counts from
  its continuity (`editor_since`/`manager_since`, fe-policy `AGENTS.md` §role-grants), not
  its own `hlc`, so promoting an editor never un-admits their earlier rows.
- **Only statement-derived authority is time-checked.** If the author's CURRENT Editor+ does
  not come from `role_grant` statements (the owner; legacy cache rows), the current gate
  decides alone. The check can only refuse, never admit.
- **Rows written before a break stay out.** A revoke (or a fall below Editor) ends a key's
  continuity, and the replica keeps one version per key, so a later re-grant cannot show the
  old window. Rows from before the break were removed by the demotion re-judge on every
  peer, and a fresh peer refuses them the same way: convergent and fail-closed. The author
  can publish them again.
- **Clock causality.** Every validated inbound role statement's HLC is observed
  (`op_log::observe_role_statement_hlc`, 5-minute future bound), so a newly granted editor's
  next write is stamped after the grant even when its wall clock lags the grantor's.
- **Markerless node rows** (legacy writers) carry no write clock: an author with
  statement-derived authority is denied (`legacy_node_row_provable`, judged as of clock 0).
  Current publishers always send complete versioned snapshots.
- **Re-judge (`scene_publication::rejudge`).** One `RejudgeReplicatedNodes` batch per verse.
  A version is admissible when both checks hold. A demotion replaces an inadmissible
  current version with the newest admissible candidate, including an inadmissible
  TOMBSTONE (peers that refused it hold the live row), or removes the node. A rise
  (`restore_only`) skips nodes present locally and restores an absent node from the newest
  admissible candidate. Journeys: the QA-0121/QA-0131 block in `op_log.rs` and fe-policy's
  lapse/re-grant test.

## §p2p-fixes-swarm-run1 (2026-11-07: QA-0121/0122/0123/0125)

- **Object doors are capped by the node's stored role (QA-0122, AC-46).** `CreateObject` and
  `CommitNodeMutation` with a `NodeAccessGrant::Scoped` (REST/MCP token) first pass
  `require_granted_node_role` in the DB dispatch: THIS node must hold Editor at the target
  scope, exactly like a Local command. The token subject is attribution only; it never
  carries authority. `SetFieldSchema` applies the same cap (Manager) in `field_def`. Journey:
  `tests/role_propagation_api_cap_test.rs`.
- **The Group registry replicates on commit (QA-0123).** `SetPetalVisibilityGroups`,
  `ComparePetalVisibilityGroups` and an editor batch whose statements include
  `REGISTRY_WRITE` (`PreparedBatch::writes_registry`) publish the petal row's catalog columns
  through `visibility_groups::publish_registry`, the same full-row shape `petal_terrain` uses.
  Inbound petal rows carrying a malformed `visibility_groups` are denied
  (`is_valid_inbound_registry`, strictest reader). Reopened (fix:2): the registry row
  is an explicit-column projection, so the petal's absent `hexon_manifest` rides as
  `null`; `apply_static_row` validated requirements BEFORE stripping nulls and silently
  denied every registry row of a petal with no requirements. Nulls are now stripped
  first and a requirements denial warns. Journey: `tests/visibility_groups_test.rs`
  replays the owner's published rows into a second peer store.
- **Scene rows are re-judged on demotion (QA-0121).** `DbCommand::RejudgeReplicatedNode`
  carries every author's replica entry for one node key (sent by fe-sync for DIDs whose role
  fell). `scene_publication::rejudge` acts only when the stored clock's publisher no longer
  holds Editor at the node scope: it restores the newest complete snapshot whose author still
  does (`apply_complete(.., rejudge = true)` bypasses the newer-clock and local-fact
  shortcuts), or deletes the node row and clock locally when no author qualifies
  (`DbResult::NodeDeleted`). Semantics are current-role: a demoted author's version is
  replaced even if written before the revoke, so every peer reaches the same state from the
  same entries and role rows whatever order it saw them in. Superseded by §write-time-role:
  a re-grant no longer replays `node/` entries.
- **API hierarchy reads never rebuild the GUI tree (QA-0125).** `DbCommand::ReadHierarchy`
  answers `DbResult::HierarchyRead`, which only pending API waiters consume;
  `LoadHierarchy`/`HierarchyLoaded` stay the GUI's own reload.
- `load_hierarchy_handler` warns once per missing scene blob until it lands (QA-0118).

## §device-payload-files (QA-0161, 2026-11-08)

`DbCommand::InsertDevicePayloads` carries `files: Vec<PayloadFile>`, blobs the API stored but did
not register. `handlers::device_payload::insert_device_payloads_with_files` registers each as an
`asset` anchored at the payload's petal (`create_asset_handler`, then `publish_uploaded_asset`,
which publishes only a scene MIME, exactly as the separate `CreateAsset` did) and appends its id
to `asset_ids` before `insert_device_payloads` validates and writes the payload. One payload is
one DB command and one reply instead of one `CreateAsset` round trip per file first. With files
the DB arm requires the anchor petal to resolve and the node to hold Editor there (the
`CreateAsset` rule); a file-registering payload writes asset rows, so the lane treats it as a
barrier for Serve admissions (§command-lane). It runs ahead of deferred transform publications
unless a file has a scene MIME (`SCENE_ASSET_MIMES`): opaque payload files never touch scene
publication, while a scene-MIME file is published like an upload and so publishes them first. As
before, assets registered for a payload that is then rejected or is an idempotent retry stay
unreferenced until asset GC reclaims them.
