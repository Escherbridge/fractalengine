---
type: Worker Report
title: Path/Node Two-Way Binding Hardening — W1 report
tags: [worker-report, path_node_binding_hardening_20260712]
timestamp: 2026-07-12T00:00:00Z
resource: ./WORKER_REPORT.md
---

# WORKER_REPORT — W1 (path_node_binding_hardening_20260712)

Edit-only pass, no cargo commands run (per hard rules — coordinator owns the
serialized build sweep). All six FRs implemented within exclusive file
ownership; two deviations from the literal spec text, both forced by files
outside ownership (see "Deviations" below).

## Files changed

### `fe-runtime/src/messages.rs`
- Added `DbCommand::DeleteNode { node_id: String }`.
- Added `DbResult::NodeDeleted { node_id: String, petal_id: String }`.
- `SceneChange::NodeRemoved` **left unchanged** (`{ node_id: String }`, no
  `petal_id`) — see Deviations.

### `fe-database/src/handlers/crud.rs`
- Added `pub(crate) async fn delete_node_handler(db: &Db, node_id: &str) -> anyhow::Result<String>`
  (returns the deleted node's `petal_id`). Cascades: looks up `petal_id`,
  deletes all `node` rows where `properties.gpx_track_id == node_id`
  (waypoints — **not** a separate `gpx_waypoint` table; confirmed via
  `fe-database/tests/gpx_pipeline_test.rs`), then `DELETE node WHERE node_id
  = $node_id RETURN BEFORE`, bailing with `anyhow::bail!` if zero rows
  matched (mirrors `entity_property.rs`'s matched-rows-assertion pattern —
  no silent no-op).
- Added `#[cfg(test)] mod delete_node_tests` at end of file: two tests —
  `delete_node_removes_row_and_cascades_waypoints` (creates a track node +
  a waypoint tagged `gpx_track_id`, deletes the track, asserts both rows
  gone) and `delete_node_bails_on_missing_node` (asserts the bail-on-empty
  behavior). Uses an in-memory `Surreal::<Mem>` DB, schemaless (matches
  production `CREATE node CONTENT {}` usage — no DDL needed). **Not run**
  per hard rules; needs the coordinator's cargo sweep to confirm compile +
  pass.

### `fe-database/src/lib.rs` (ONLY the allowed emit-site zone, new arm added
adjacent to the `DeleteNodeProperty`/`SetPetalTerrain` arms — no other lines
touched)
- Added `Ok(DbCommand::DeleteNode { node_id }) => { ... }` dispatch arm:
  calls `delete_node_handler`, on success emits
  `SceneChange::NodeRemoved { node_id }` (via `entity_change_tx`, mirroring
  the existing `NodeAdded` emit style), appends a `node_log` entry
  (`"deleted"`), then sends `DbResult::NodeDeleted { node_id, petal_id }`.
  On failure sends `DbResult::Error("Delete node failed: {e}")`.

### `fractalengine/src/gpx_bridge.rs`
- **FR-2**: `PathOp::DeleteTrack` now sends one `DbCommand::DeleteNode`
  instead of two `DeleteNodeProperty` calls. Still optimistically clears
  the render-side `TrackRouteMap` entry + despawns the `GpxTrackLine`
  entity (kept for immediate UI feedback); DB-side cascade + left-panel/
  Paths-tab sync now flow through FR-1/FR-3.
- **FR-4**: added `PendingPathEdits.track_property_confirms: HashMap<String,
  HashSet<&'static str>>`. On `CreateTrack`'s `NodeCreated`, seeds this with
  the 3 keys about to be set (`gpx_type`, `gpx_points`, `gis.track.name`)
  instead of immediately setting `PathEditStatus` success. New
  `DbResult::NodePropertySet` arm in `advance_path_edits` pops confirmed
  keys and only flips `PathEditStatus` to `"Track created"` once the set is
  empty. New `DbResult::Error(msg) if msg.starts_with("Set node property
  failed")` arm surfaces the failure against whichever track is still
  pending (best-effort correlation — see Deviations, `Error` carries no
  `node_id`). **Also fixed a latent bug while doing this**: the early-return
  guard at the top of `advance_path_edits` didn't check
  `track_property_confirms`, so once `pending.creates` drained (on
  `NodeCreated`) but before `NodePropertySet` results arrived, the system
  would early-return and never process the confirms — added the 4th
  condition to the guard.
- **FR-5**: added `PendingPathEdits.in_flight_points: HashMap<String,
  Vec<TimestampedRoutePoint>>`. First `AppendPoint` for a track still goes
  through `GetNodeProperties` (seed read) but that read handler now also
  inserts into `in_flight_points`. Every subsequent `AppendPoint` for the
  same `track_node_id` checks `in_flight_points` first and, if present,
  mutates it directly + flushes one `SetNodeProperty` — no second
  `GetNodeProperties` round trip, killing the race where two overlapping
  reads both start from the same stale base and the later write clobbers
  the earlier append. `RemovePoint`'s existing read-path also now
  re-syncs `in_flight_points` after mutating, so a later `AppendPoint`
  doesn't resurrect a point removed by an earlier `RemovePoint`.
  `DeleteTrack` clears the entry.
- **FR-6**: `advance_path_materialization` (previously only consumed
  `NodePropertiesLoaded`) now also consumes `DbResult::NodeDeleted`:
  removes the `TrackRouteMap` entry and despawns the matching
  `GpxTrackLine` entity. Function doc comment rewritten to describe it as
  the one projection path from DB events to render state.
- Module doc comment for `drain_path_ops` updated to drop the stale "no
  DbCommand::DeleteNode exists" note.

### `fe-ui/src/verse_manager/db_results.rs`
- Added `DbResult::NodeDeleted { node_id, petal_id }` arm: `petal.nodes.retain(|n| n.id
  != *node_id)` (generic left-panel fix for **all** node deletes, not just
  tracks — matches FR-3's literal ask), closes the path editor if the
  deleted node was being edited, and — since `petal_id` is on hand — calls
  `crate::actions::path::query_tracks(&db_sender, &mut path_state,
  petal_id.clone())` when the delete is in the active petal.
- `DbResult::NodeCreated` arm extended to also call `query_tracks` for the
  active petal (covers `CreateTrack`'s `NodeCreated`, so a freshly created
  track shows up in the Paths tab without a manual refresh).
- `DbResult::NodePropertySet` arm extended: when `key == "gis.track.name"`,
  unconditionally (independent of the existing `is_for_selected_node`
  guard used for the Inspector panel) re-runs `query_tracks` for the active
  petal — covers the "any track-property change" clause of FR-3.
- No new "tracks dirty" flag was added — see Deviations.

### `fe-ui/src/panels/path_editor_card.rs`
- Doc comment added above the Refresh button noting it's now a manual
  override, not the only sync path.

### `fe-ui/src/gis/query.rs`
- **Untouched.** Read for reference (`track_query`/`parse_gis_row` shape)
  but no change was needed there — the FR-3 sync fix lives entirely in
  `db_results.rs`'s result-handling, not in the query builder.

## New message/command/SceneChange signatures (integration surface for other workers)

```rust
// fe-runtime/src/messages.rs
DbCommand::DeleteNode { node_id: String }
DbResult::NodeDeleted { node_id: String, petal_id: String }
// SceneChange::NodeRemoved is UNCHANGED: { node_id: String } — no petal_id.
```

```rust
// fe-database/src/handlers/crud.rs
pub(crate) async fn delete_node_handler(db: &Db, node_id: &str) -> anyhow::Result<String>
// returns petal_id on success; anyhow::bail! if node_id matched no row.
```

Any other worker consuming `DbCommand`/`DbResult` (exhaustive matches) will
need to handle the two new variants — non-exhaustive `_ => {}` catch-alls
(used in most consumers I saw, e.g. `fe-api/src/ws.rs`, `fractalengine/src/main.rs`)
are unaffected.

## Deviations from spec (and why)

1. **`SceneChange::NodeRemoved` does NOT carry `petal_id`**, contrary to the
   spec's literal `SceneChange::NodeRemoved { node_id, petal_id }`. This
   variant is a pre-existing type (it already existed before this track,
   contrary to the spec's root-cause claim that "no `SceneChange::NodeRemoved`
   exists" — it does, just unused by any DB emit site). It has consumers in
   `fractalengine/src/main.rs:407-408` and `fe-api/src/ws.rs:526` (test),
   **both explicitly out of my file ownership / `fe-api/*` is hard-banned**.
   Adding a field would have broken their compilation, and I cannot fix
   compile errors in forbidden files. Mitigation: `DbResult::NodeDeleted`
   (which I *am* allowed to shape, and which I do emit alongside
   `NodeRemoved` on every delete) carries `petal_id` instead — consumers
   needing petal-scoped node-removal should read that result, not
   `SceneChange::NodeRemoved`. **Flagging for the coordinator**: if a later
   wave needs `SceneChange::NodeRemoved.petal_id`, `fractalengine/src/main.rs`
   and `fe-api/src/ws.rs` will need a matching one-line fix at the same time.

2. **FR-4's "request/correlation id threaded through CreateNode/NodeCreated"
   was NOT implemented as a schema change** to `DbCommand::CreateNode` /
   `DbResult::NodeCreated`. That's shared infrastructure used by ~20 files
   including `fe-api/src/rest.rs`, `fe-api/src/gpx.rs`, `fe-test-harness`,
   `fe-ui/src/dialogs/create_entity.rs` — all out of ownership or hard-banned
   (`fe-api/*`). Instead, implemented the achievable subset entirely inside
   `gpx_bridge.rs`:
   - Gated `PathEditStatus` success on actual `NodePropertySet` confirms
     (not optimistic-on-NodeCreated) — this **is** the literal ask of FR-4's
     second sentence and is fully done.
   - Did **not** eliminate the `(petal_id, name)` correlation between
     `advance_path_edits` and `advance_gpx_imports` — both still key on that
     tuple and could theoretically still steal each other's `NodeCreated`
     result on an exact `(petal_id, name)` collision between an authored
     track and a same-named GPX import happening in the same window. This
     residual risk is unchanged from before this track. **A full fix
     requires either the forbidden schema change or a `fe-database/src/lib.rs`
     change outside the 3 named emit sites** (e.g. having the DB thread
     itself assign and echo back a caller-supplied token) — out of reach
     under the current file-ownership grant. Recommend a follow-up track
     scoped to touch `fe-database/src/lib.rs` broadly + `fe-api/*` +
     `fe-ui/src/dialogs/create_entity.rs` + `fe-test-harness` together, or
     a narrower fix scoped only to `gpx_bridge.rs` + `messages.rs` if the
     other consumers can be confirmed to tolerate a new optional field via
     `..Default::default()`-style destructuring (they currently pattern-match
     without `..`, so even an optional field addition would need those
     match sites touched).

## Other notes

- `fe-database/src/lib.rs` is otherwise **untouched** outside the new
  `DeleteNode` arm — verified by re-reading the diff region; no stray edits
  near the quarantined WIP.
- Waypoint cascade uses `properties.gpx_track_id`, not a `gpx_waypoint`
  table — confirmed against `fe-database/tests/gpx_pipeline_test.rs`'s
  `WAYPOINTS_SQL` constant and round-trip test before writing the handler;
  the spec's phrasing ("child waypoints (`WHERE gpx_track_id = $node_id`)")
  reads naturally as a column condition, which is what was implemented,
  just against the `node` table.

## TODOs left for the coordinator's build sweep

- Run `cargo test -p fe-database delete_node_tests` (or full sweep) —
  unverified by me per hard rules.
- Run `cargo build`/`cargo check` across `fe-runtime`, `fe-database`,
  `fractalengine`, `fe-ui` — unverified by me per hard rules.
- Confirm no other exhaustive `match` on `DbCommand`/`DbResult` (outside
  files I read) breaks on the two new variants.

WORKER_COMPLETE W1

## Coordinator fix: delete_node bail (CORRECTED — real bug, fix applied)

The first triage of this failure concluded "no bug / stale rmeta" — that
was WRONG. Running the single test with `--nocapture` on a clean build
gave the actual error:

```
unexpected error message: DeleteNode lookup statement failed: The table 'node' does not exist
```

**Root cause (real):** `setup_mem_db()` (crud.rs:828) builds a bare
schemaless `Surreal::<Mem>` with NO `node` table. The sibling test
`delete_node_removes_row_and_cascades_waypoints` passes because it calls
`create_node_handler` first (which creates the table). The bail test
never creates a node, so the handler's first statement
`SELECT petal_id FROM node WHERE node_id = $node_id` hits `.check()` and
errors with *"The table 'node' does not exist"* — which is mapped to
`"DeleteNode lookup statement failed: ..."` and returned BEFORE reaching
the `"matched no node"` bail. That message lacks the required substring,
so the test's `assert!(err.contains("matched no node"))` panicked. Not
rmeta — a genuine empty-table edge case in the handler.

**Fix (applied, crud.rs lookup block):** treat a table-absent `.check()`
error as the empty/not-found case — an absent `node` table means no
nodes exist, which is semantically "matched no node", not a hard DB
error. The lookup now matches on `.check()`: `Ok` → take rows as before;
`Err` whose message contains `"does not exist"` → empty rows (falls
through to the `"matched no node"` bail); any other `Err` → still a real
`"DeleteNode lookup statement failed"`. Preserves FR-1's contract
(missing node → error containing `"matched no node"`) on both an empty
DB and a populated-but-missing-id DB. Cascade + success return of
`petal_id` unchanged.

## Coordinator fix batch: FR-5 concurrency + cascade atomicity

Opus-review defects in the FR-5 lost-update fix, plus a non-atomic delete
cascade. Touched only `fractalengine/src/gpx_bridge.rs` and
`fe-database/src/handlers/crud.rs`.

### FIX 1 (HIGH-1) — seed handler honors already-seeded `in_flight_points`
Two `AppendPoint`s issued before the first `GetNodeProperties` seed reply each
fired their own read against the same stale DB base; the second
`NodePropertiesLoaded` unconditionally rebuilt from its snapshot and clobbered
the first append. Fixed with both defenses:
- **Dedup seed reads (drain):** added `PendingPathEdits.seed_pending: HashSet<String>`.
  In `drain_path_ops`, an `AppendPoint` with no in-flight buffer only issues a
  `GetNodeProperties` if `seed_pending.insert(track)` is a first insert;
  otherwise it just enqueues onto `reads`. Overlapping appends serialize
  against one read instead of racing two.
- **Honor the buffer (advance):** `NodePropertiesLoaded` now bases `points` on
  the existing `in_flight_points` entry when present (else the read snapshot),
  clears `seed_pending`, and after the first append drains all further queued
  `AppendPoint`s from the `reads` queue into the same buffer — so no confirmed
  append is lost when several overlap one seed read. Single-append behavior is
  unchanged (empty queue → same path as before).

### FIX 2 (HIGH-2) — `RemovePoint` mutates `in_flight_points` when present
`PathOp::RemovePoint` unconditionally re-read the DB, so an overlapping
fast-path `AppendPoint` (not yet committed) was clobbered. Now `drain_path_ops`
checks `in_flight_points` first: if a buffer exists it removes the index there
and flushes via `persist_and_render_points` (with the same out-of-range error
handling); only falls back to the `GetNodeProperties` read path when no
in-flight buffer exists. No-in-flight behavior is byte-for-byte the old path.

### FIX 3 (MEDIUM-2) — clear `in_flight_points` on petal switch / materialization
`in_flight_points` was only dropped on `DeleteTrack`, so it persisted across
petal switches and reloads — a later append could fast-path off a buffer that
no longer matched the DB (e.g. after a P2P-sync remote write), and it leaked.
`request_petal_gpx_materialization` now takes `ResMut<PendingPathEdits>` and
clears both `in_flight_points` and `seed_pending` at the petal-load entry point,
forcing the next append to re-seed fresh. `DeleteTrack` also now clears
`seed_pending` alongside its existing `in_flight_points.remove`.

### FIX 4 (LOW-2) — atomic waypoint cascade in `crud.rs`
`delete_node_handler` ran two separate `DELETE`s (waypoints, then parent)
non-atomically — a crash between them left a parent row with its waypoints gone.
Replaced with a single statement
`DELETE node WHERE node_id = $node_id OR properties.gpx_track_id = $node_id
RETURN BEFORE`. The pre-lookup of `petal_id` (return value) and the
matched-no-node bail are preserved: the bail now checks that the `RETURN BEFORE`
set contains a row whose `node_id` equals the target (the parent), so a parent
that vanished between lookup and delete still errors with `"matched no node"`.

No `unwrap()`/`expect()` in production paths; terse `///` doc-comments; rationale
kept inline-terse per styleguide. cargo NOT run (coordinator owns the build).

## Coordinator fix: FR-4 correlation-id (full)

FR-4 was previously PARTIAL: the optimistic-success bug was fixed, but
`advance_path_edits` (authored `CreateTrack`) and `advance_gpx_imports` (GPX
import) both consumed the same broadcast `DbResult::NodeCreated` stream keyed by
the NON-UNIQUE `(petal_id, name)` tuple. An authored track and a same-named GPX
import created in the same frame window could steal each other's `NodeCreated`.
This is the FULL fix: an explicit correlation id now disambiguates the authored
path, so the `(petal_id, name)` tuple is no longer the disambiguator for
authored tracks.

### The new field

Optional, semantically additive, threaded end-to-end:

- `fe_runtime::messages::DbCommand::CreateNode` gained `correlation_id: Option<String>`.
- `fe_runtime::messages::DbResult::NodeCreated` gained `correlation_id: Option<String>`.

`None` preserves the legacy content-correlated behavior for every existing
sender; only the racing authored-track path sets `Some(_)`.

### How the authored-track path now disambiguates

- `fractalengine/src/gpx_bridge.rs`:
  - New `next_authored_track_correlation_id()` — a process-unique id from a
    `static AtomicU64` counter (`"authored-track:{n}"`). Chosen over ulid/uuid
    to avoid adding a crate dependency to the `fractalengine` binary (would have
    touched `Cargo.toml` + Cargo.lock and forced a resolve in the coordinator's
    build). A monotonic counter is sufficient: uniqueness is only needed within
    one process's in-flight window.
  - `PendingPathEdits::creates` changed from
    `HashMap<(String, String), VecDeque<PendingTrackCreate>>` to
    `HashMap<String, PendingTrackCreate>` keyed by that unique id (unique key ⇒
    no FIFO queue needed).
  - `drain_path_ops` `PathOp::CreateTrack` sends
    `CreateNode { ..., correlation_id: Some(id) }` and stores the waiter under
    that id.
  - `advance_path_edits` `NodeCreated` arm now matches authored creates by
    `correlation_id` (`pending.creates.remove(cid)`), NOT by `(petal_id, name)`.
  - `advance_gpx_imports` skips any `NodeCreated` with `correlation_id.is_some()`
    (`if correlation_id.is_some() { continue; }`) — the import path only owns
    `None` results.
  - The annotate-waypoint branch inside `advance_path_edits` keeps its
    `(petal_id, name)` correlation but is only reached for `None` results
    (guarded by the authored-`Some(_)` early branch above it), and its own
    `CreateNode` sends `correlation_id: None`.

Net effect: the two `NodeCreated` consumer streams are partitioned by presence
of a correlation id — an authored track's `Some(id)` result can only be matched
by `advance_path_edits`'s id lookup, and an import's `None` result can only be
matched by the import/annotate `(petal_id, name)` paths. They can never
cross-consume, closing the race.

### The DB echo (dispatch layer, minimal churn)

Echoed at the dispatch layer (`fe-database/src/lib.rs`) per plan preference, so
`create_node_handler` (`fe-database/src/handlers/crud.rs`) needed NO signature
change — its `Ok(id)` return is unchanged; the id is echoed alongside in the
emitted `NodeCreated`.

### Every file touched

Core:
- `fe-runtime/src/messages.rs` — added the optional field to both variants.
- `fractalengine/src/gpx_bridge.rs` — the actual FR-4 fix (see above).
- `fractalengine/src/AGENTS.md` — replaced the documented "residual risk"
  note with the FR-4 resolution; updated the `CreateNode` shape reference.

Construction/match sites updated to name the new field (all `correlation_id: None`
except the authored path):
- `fe-ui/src/dialogs/create_entity.rs` — `CreateNode` construction (`None`).
- `fe-ui/src/verse_manager/db_results.rs` — `NodeCreated` destructure gained `..`
  (it bound all fields explicitly; does not need the id).
- `fe-test-harness/src/peer.rs` — `CreateNode` destructure + `NodeCreated`
  construction echo the id (test harness mirrors the real DB dispatch).
- `fe-database/tests/db_test.rs` (2 sites), `gis_data_test.rs`,
  `gpx_pipeline_test.rs`, `gpx_path_persistence_test.rs` — `CreateNode`
  constructions (`None`). Their `NodeCreated` matches already used `{ id, .. }`.

QUARANTINED files — PURELY ADDITIVE edits only (confirmed below):
- `fe-database/src/lib.rs` (quarantined) — dispatch arm: added `correlation_id`
  to the `CreateNode` destructure and passed it into the emitted `NodeCreated`.
  Two existing lines each gained only `, correlation_id`; no WIP line rewritten.
- `fe-api/src/rest.rs` (quarantined) — 3 `CreateNode` constructions gained
  `correlation_id: None`. Its `NodeCreated` matches already used `{ id, name, .. }`.
- `fe-api/src/mcp.rs` (quarantined) — 1 `CreateNode` (`None`).
- `fe-api/src/gpx.rs` (quarantined) — 1 `CreateNode` (`None`).
- `fe-api/src/format.rs` (quarantined) — 1 `CreateNode` (`None`).

### Quarantined-file additive confirmation

I made ONLY additive edits to the quarantined files. I did not reformat, revert,
or rewrite any pre-existing uncommitted (Antigravity) WIP line. Verified via
`git diff`: the sole additions in `fe-api/*` are `correlation_id: None,` lines;
the sole additions in `fe-database/src/lib.rs` are the two `, correlation_id`
tokens. `fe-api/src/assets.rs` and `fe-api/Cargo.toml` were NOT touched (0
correlation_id additions). Pre-existing WIP such as rest.rs's
`response.num_statements()` lines was left exactly as found.

### Sites I was unsure about / notes

- `fe-plugin`'s `PluginCommand::CreateNode` and `PendingOp::CreateNode`
  (`fe-plugin/src/lib.rs`, `transaction.rs`, `rhai/host_api.rs`,
  `wasm/host_imports.rs`) are a DIFFERENT enum from `DbCommand::CreateNode` and
  were correctly NOT touched.
- `fe-api/src/types.rs` has only a `CreateNodeRequest` REST DTO struct (not the
  enum variant) — not touched.
- `fe-api/tests/gis_test.rs:119` mention is a doc comment, no construction.
- Docs/diagrams referencing the old 3-field shape
  (`docs/diagrams/05-database-schema.md`, `research/...`, archived specs) were
  left as historical prose (not compiled).

### Verification

Edit-only; cargo NOT run (coordinator owns the serialized build — concurrent
cargo crashes rustc here). Exhaustiveness verified statically: grepped every
`DbCommand::CreateNode {` and `DbResult::NodeCreated {` across the workspace and
confirmed each construction names `correlation_id` and each exhaustive
destructure either binds it or uses `..`. No `unwrap()`/`expect()` added; terse
`///` doc-comments; substantial WHY recorded in `fractalengine/src/AGENTS.md`
§gpx.
