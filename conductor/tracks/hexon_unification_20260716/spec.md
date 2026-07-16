---
type: Track Spec
title: Hexon Unification — One Canonical Format + Portable Petal Snapshot
description: Collapse the fe-hexon parallel format stack onto fe-format v1.0.0 as the
  single canonical .hexon implementation, and add a PetalSnapshot hexon type that
  packages the full state and op-log of a petal's SurrealDB instance.
tags: [feature, hexon_unification_20260716, pending]
timestamp: 2026-07-16T00:00:00Z
resource: ./metadata.json
---

# Specification: Hexon Unification + Portable Petal Snapshot

**Track ID:** `hexon_unification_20260716`
**Crates:** `fe-format`, `fe-hexon`, `fe-api`, `fe-hexon-registry`, `fe-database`,
`fe-runtime`
**Directives:** ratified by user 2026-07-16 — binding constraints, restated in
§Constraints below.

## Overview

Two parallel hexon stacks exist today:

- **fe-format** — the canonical `.hexon` v1.0.0 format: `HexonManifest`
  (`schema_version: "1.0.0"`, `hexon_id`, `HexonType` enum), `HexonArchive`
  ZIP export/import (`manifest.json` + `entries.json` + `assets/{blake3}` +
  type-specific sections), canonical-JSON ed25519 signing
  (`fe-format/src/signature.rs`). Lean deps (serde, zip 2, chrono,
  ed25519-dalek — no tokio/DB/network). Already consumed by `fe-terrain`,
  `fe-hexon-registry`, and fe-api's petal Scene export.
- **fe-hexon** — a *second, incompatible* format (`HexonManifest` with
  `schema_version: u32` + `crate_id` + `HexonKind`, `.fecrate` ZIP via zip 0.6,
  raw-bytes manifest signing) plus the runtime machinery: `HexonRegistry`
  (install/uninstall, `crate_registry`/`crate_entry` DB rows, `FsBlobStore`),
  `HexonBuilder` publisher, type install handlers, P2P announce/discover,
  `RemoteRegistryClient`, and real deny-by-default authz (`authz.rs`,
  fe-policy).

Concrete cost of the split: the hosted registry (`fe-hexon-registry`) indexes
on `hexon_id` (`index.rs:55` — "manifest missing hexon_id"), so a `.fecrate`
built by `HexonBuilder` (which has `crate_id`, no `hexon_id`) **cannot be
published to the project's own registry service**. Two signing schemes coexist
(delta-format spec amendment A3 flagged this). Two zip crate versions ship in
one workspace.

This track does two things:

1. **Unification** — fe-format stays the pure format layer and its v1.0.0
   schema is upheld as the only `.hexon` format; fe-hexon is rebuilt as the
   runtime layer *on* fe-format types; fe-hexon's duplicate
   manifest/package/signature code and the `.fecrate` format are deleted.
2. **Portable petal snapshot** — a new `HexonType::PetalSnapshot` whose
   archive carries the full state and op-log of a petal's SurrealDB instance:
   all petal-scoped rows, referenced asset rows + blobs, and the `op_log`
   entries with HLC lamport clocks and `sig` fields carried verbatim.

## Background

- **Direction decision (fe-format absorbs the format role, fe-hexon keeps the
  runtime role).** fe-format's dependency footprint is intentionally lean
  (`fe-format/Cargo.toml`: fe-runtime, serde, serde_json, zip 2, anyhow,
  chrono, ed25519-dalek, base64, bs58). fe-hexon carries tokio, crossbeam,
  fe-policy, fe-identity, image, dirs, and optional reqwest. Absorbing
  fe-hexon into fe-format would drag runtime deps into every format consumer
  (fe-terrain, fe-hexon-registry, fe-api, and the sibling gis-tile-etl repo).
  The reverse — fe-hexon gains a real (currently dev-only) dependency on
  fe-format and deletes its parallel types — keeps the layering clean. The
  opposite direction is rejected on this dep-footprint ground.
- **Existing petal export is a strict subset.** fe-api already exports a
  Scene-type hexon per petal (`fe-api/src/format.rs::export_petal` — nodes +
  field_defs, assets deliberately empty, no properties in the channel
  fallback, no op_log). PetalSnapshot is the superset: portable *instance*
  (full table state + history + blobs) vs portable *content* (Scene). Both
  remain; the Scene export is unchanged by this track.
- **The op-log primitives exist.** `fe-database/src/types.rs::OpLogEntry
  { lamport_clock: u64, hlc_timestamp: String, node_id, op_type, payload,
  sig }`; HLC is a packed u64 (`wall_ms << 16 | counter`,
  `fe-database/src/op_log.rs`) whose `init_hlc(max_persisted)` already knows
  how to advance past a persisted maximum — exactly what import needs.
  `OpType::ExportPetal` / `ImportPetal` variants already exist
  (`fe-database/src/types.rs:22-23`).
- **The DB thread owns SurrealDB.** All writes go through the
  `DbCommand`/handler seam (`fe-runtime/src/messages.rs:116`); fe-api
  additionally holds an optional read-only `db_reader`
  (`fe-api/src/server.rs:33`) that bypasses the channel for reads.

### Relationship to `hexon_delta_format_20260710` (foundry-candidate)

That track is **spec-only and foundry-candidate** (closed side of the D-12
line): replayable delta/WAL hexons, per-entry signature chains, hash-chained
sequence tamper-detection, HashSeq/manifest-of-blobs streaming containers, and
the §D4 log-first write-path inversion. **This track delivers the open-core
FULL-snapshot capability and must be forward-compatible with — but not
implement — delta hexons:**

- `snapshot/oplog.jsonl` entries are exactly `OpLogEntry`-shaped, the same
  record shape a future delta hexon's entries would use.
- `snapshot/meta.json` records the covered op-log range
  (`min_lamport`/`max_lamport`/`count`), so a snapshot can serve as the
  checkpoint base a future delta range anchors on.
- No replay function, no hash chain, no per-op signing (sigs are placeholders
  per decision D5-1 and are carried verbatim), no zstd/HashSeq container. ZIP
  is retained — amendment A1 keeps ZIP for whole-archive hexon types, and a
  snapshot is by definition a whole-archive type.

### Relationship to `hexon_path_asset_20260713` FR-5 (PetalHexon bake)

FR-5 ("serialize a petal's node write-ops into a fe-hexon package; reload
rehydrates nodes") remains open on that track. **PetalSnapshot subsumes it:**
snapshot export/import is the platform capability FR-5 described, delivered on
the unified format instead of the deleted `.fecrate` stack. On completion,
that track's FR-5 should be marked satisfied-by-reference to this track (its
FR-6 multi-asset seam is unaffected).

## Functional Requirements

### FR-1: Single manifest and type system (Priority: P0)

fe-hexon's duplicate `HexonManifest`, `HexonKind`, `AssetEntry`, `EntryKind`,
`License`, `LicenseType`, and `CrateDep` (`fe-hexon/src/manifest.rs`) are
deleted. All fe-hexon runtime code uses `fe_format` types. The fe-format
schema (`schema_version: "1.0.0"`, `hexon_id`) is canonical. Additive
extension only: `HexonType` gains `Script` (parity with `HexonKind::Script`)
and `PetalSnapshot`; any `License`/`AssetEntry` fields that exist only in
fe-hexon (e.g. payment fields, `encrypted_key`) are ported additively into
fe-format so no capability is lost.

**Acceptance criteria:**
- `fe-hexon/src/manifest.rs` no longer defines a manifest type; grep for
  `crate_id` in fe-hexon/fe-api returns no production hits.
- `HexonType::{Script, PetalSnapshot}` serde-round-trip (snake_case) tests
  pass; every pre-existing `HexonKind` value maps to a `HexonType` value.
- fe-hexon depends on fe-format as a normal dependency (today dev-only).

### FR-2: Single container and signing scheme (Priority: P0)

`.fecrate` is removed. `HexonPackage`/`HexonBuilder` produce and open
fe-format `.hexon` ZIP archives (via `HexonArchive`), eliminating fe-hexon's
direct zip 0.6 dependency. Manifest signing uses fe-format's canonical-JSON
scheme (`sign_manifest`/`verify_manifest`, sorted-keys-no-whitespace) as the
one scheme — resolving delta-spec amendment A3 in fe-format's favor.
fe-hexon/src/signature.rs's raw-bytes scheme is deleted (its blake3
`asset_hash` helper survives, relocated or re-exported).

**Acceptance criteria:**
- `HexonBuilder::build` output opens with `HexonArchive::import` and vice
  versa; `signature_valid()` verifies via fe-format `verify_manifest`.
- Tampered-manifest rejection test passes under the unified scheme (security
  testing requirement in workflow.md).
- No `zip` entry remains in fe-hexon/Cargo.toml; workspace has one zip version
  for hexon code.

### FR-3: Runtime layer rebuilt on fe-format (Priority: P0)

`HexonRegistry` (install/uninstall/list/search + `FsBlobStore`), the type
install handlers (model, material, skybox, sound, terrain, gpx_collection),
P2P announce/discover, and the publisher builder are re-typed on fe-format.
`InstalledCrate` becomes `InstalledHexon` keyed by hexon URI
(`hexon_id[@version]`, adopting `fe-hexon-registry/src/index.rs::split_uri`
parsing as canonical). The `crate_registry`/`crate_entry` DB row shapes are
redefined around `hexon_id` — per directive 2, delete/replace with **no
migration path** for existing rows.

**Acceptance criteria:**
- All existing fe-hexon handler and registry unit tests pass re-typed.
- A hexon built by the unified `HexonBuilder` resolves through
  `fe-hexon-registry`'s `index_one`/`resolve` (the `hexon_id` index mismatch
  is dissolved — this is the track's headline integration proof).

### FR-4: Authorization survives the merge (Priority: P0)

The deny-by-default fe-policy engine in `fe-hexon/src/authz.rs`
(`install_as`/`uninstall_as` Editor+, `list_installed_as`/`search_local_as`
Viewer+) is preserved verbatim in behavior across the re-typing, with its
tests green. New snapshot operations are gated through the same engine:
**export Manager+** (a snapshot contains role assignments and full history —
strictly more sensitive than the Viewer+ Scene export) and **import Manager+**
(it writes petal, node, and role rows) at the resolved petal/fractal scope.

**Acceptance criteria:**
- Existing authz tests pass unchanged in intent (deny-by-default, role
  thresholds).
- New tests: Editor is denied snapshot export and import; Manager+ is allowed.

### FR-5: fe-api /crates/* re-pointed; dead code deleted (Priority: P1)

`fe-api/src/hexon.rs` handlers (`publish`, `install`, `uninstall`, `search`,
`installed`, `{uri}`, `{uri}/entries`, `{uri}/entries/{id}/asset`,
`available` — routes at `fe-api/src/server.rs:321-351`) are rebuilt on the
unified types. `install_crate` — today essentially a stub that only echoes an
association — goes through `HexonRegistry::install_as` so the petal
association is actually recorded and authz-checked. All `.fecrate`-specific
code is deleted. Route paths stay as-is (`/api/v1/crates/*`) — renaming is out
of scope. `RemoteRegistryClient` gains the missing `publish()` (multipart POST
to the registry's existing publish route), closing the client/server gap.

**Acceptance criteria:**
- End-to-end test: build → publish via `/api/v1/crates/publish` → appears in
  `search`/`installed` → `entries` and `{entry_id}/asset` serve fe-format
  fields.
- `RemoteRegistryClient::publish()` round-trips against the
  `fe-hexon-registry` router in-process (publish → search → download).
- `rg -i fecrate` across the workspace returns only historical docs/specs.

### FR-6: PetalSnapshot archive format (Priority: P0)

A `PetalSnapshot` hexon is a standard v1.0.0 archive with an additive
`snapshot/` section:

```
manifest.json                    # hexon_type: "petal_snapshot"
entries.json                     # AssetEntry per carried blob (blake3-keyed)
license.json                     # optional, as usual
snapshot/meta.json               # SnapshotMeta (below)
snapshot/state/petal.json        # the petal row, verbatim
snapshot/state/nodes.json        # node rows (petal-scoped)
snapshot/state/node_log.json     # node_log rows (petal-scoped)
snapshot/state/field_defs.json   # field_def rows in the petal's scope
snapshot/state/rooms.json        # room rows (petal-scoped)
snapshot/state/models.json       # model rows referenced by the petal's nodes
snapshot/state/assets.json       # asset rows referenced by the petal's nodes
snapshot/state/roles.json        # role rows whose scope is the petal
snapshot/state/iot_readings.json # iot_reading rows (petal-scoped)
snapshot/oplog.jsonl             # one OpLogEntry per line, lamport ascending
assets/{blake3}                  # blob bytes for every entry in entries.json
```

`SnapshotMeta`: `{ snapshot_format_version, petal_id, scope,
source_node_id, exported_at, table_row_counts: {table: n},
op_log_range: { min_lamport, max_lamport, count } }`.

Op-log entries carry `lamport_clock`, `hlc_timestamp`, `node_id`, `op_type`,
`payload`, and `sig` **verbatim** — `sig` values are today's placeholders
(decision D5-1); this track must not invent signing or re-sign entries.
Op-log scope filter: entries whose payload references the petal (payload
`petal_id` field equal to the petal, or scope string containing
`PETAL#<id>`); the predicate is versioned in `SnapshotMeta` (see Open
Question 1).

**Acceptance criteria:**
- Format-level round-trip test with synthetic rows: export → import →
  structural equality on every `snapshot/state/*.json` array and byte-order
  equality on `oplog.jsonl`.
- A `PetalSnapshot` archive with zero assets and empty optional tables is
  valid (all `snapshot/state/*` files present, possibly `[]`).

### FR-7: Petal snapshot export (Priority: P0)

Export path `petal_id -> .hexon` through the DB seam: new
`DbCommand::ExportPetalSnapshot { petal_id, reply-correlation }` +
`DbResult` variant. The DB-thread handler collects all FR-6 table rows plus
the filtered, lamport-ordered op-log and returns them as a
`PetalSnapshotData` payload; blob bytes are then attached **outside the DB
thread** (fe-hexon runtime layer, from `FsBlobStore`/asset paths, keyed and
verified by BLAKE3) and the archive is built by fe-format. fe-api exposes
`GET /api/v1/petals/{petal_id}/snapshot.hexon` (Manager+, scope-checked,
mirroring `export_petal`'s scope resolution). The export itself is recorded
as an `OpType::ExportPetal` op-log entry.

**Acceptance criteria:**
- Exporting a seeded petal yields an archive whose `entries.json` lists every
  referenced blob, whose blob bytes hash to their `assets/{blake3}` names,
  and whose `oplog.jsonl` is strictly ascending in `lamport_clock`.
- The endpoint streams `application/x-hexon+zip` with a content-disposition
  filename, consistent with the existing Scene export.

### FR-8: Petal snapshot import/restore (Priority: P0)

Import path `.hexon -> live DB` strictly through the DB thread: new
`DbCommand::ImportPetalSnapshot`. Semantics:

- **Refuse-on-conflict:** if the `petal_id` already exists, import fails with
  a distinct error. No merge — merging concurrent histories is delta/CRDT
  territory (out of scope).
- **Verbatim restore:** rows are recreated with original IDs, timestamps, and
  op-log `lamport_clock`/`hlc_timestamp`/`sig` values untouched.
- **HLC safety:** after restoring op-log rows, the HLC is advanced past the
  imported `max_lamport` (reusing `init_hlc`'s advance-past-persisted logic)
  so post-import writes stay monotonic.
- **Blob restore:** `assets/{blake3}` blobs land in the blob store; each is
  re-hashed and verified against its name before the DB rows are committed.
- fe-api exposes `POST /api/v1/petals/import-snapshot` (multipart, Manager+);
  the import is recorded as an `OpType::ImportPetal` op-log entry.

**Acceptance criteria:**
- Import into a fresh DB reproduces every table row and every op-log entry;
  importing the same snapshot twice fails the second time with the conflict
  error.
- A corrupted blob (hash mismatch) aborts the import with no partial petal
  visible.
- First write after import receives a `lamport_clock` greater than the
  imported maximum (test with a synthetic far-future lamport, mirroring
  `survives_restart_past_persisted`).

### FR-9: Round-trip determinism (Priority: P0)

The track's definition-of-working: **export petal → import into a fresh DB →
re-export → the two snapshots match** — structural equality of every
`snapshot/state/*` table (row sets), identical op-log count and order, and
all BLAKE3 asset hashes verifying. Volatile manifest fields
(`created_at`/`updated_at`/`exported_at`, `build_id`) are excluded from the
comparison; everything else must be equal.

**Acceptance criteria:**
- An integration test (fresh embedded SurrealDB instances) implements exactly
  this loop and passes.

### FR-10: Registry service serves snapshots (Priority: P1)

`fe-hexon-registry` (already fe-format-based) indexes, searches, and serves
`PetalSnapshot` hexons through its existing routes
(`/hexons/publish|search|{uri}|entries|download`) with no format special-casing
— only whatever additive type-filter plumbing search needs.

**Acceptance criteria:**
- Publish a PetalSnapshot hexon to an in-process registry router; find it via
  search filtered by type; download bytes equal published bytes.

## Non-Functional Requirements

### NFR-1: Layering / dependency hygiene
fe-format keeps its lean footprint: no tokio, crossbeam, fe-policy,
fe-identity, network, or DB dependencies may be added. All runtime concerns
live in fe-hexon. fe-database/fe-runtime gain no fe-format dependency beyond
what the seam payloads require (prefer plain serde types in
`DbCommand`/`DbResult`).

### NFR-2: Thread topology respected
All DB writes via `DbCommand` on the DB thread; export reads may use the
`db_reader` pattern only if the DB-thread handler path proves insufficient
(default: DB-thread handler, matching the guidance seam). No `block_on` in
Bevy systems (none are touched).

### NFR-3: Security
Signature verification before any install/import (fe-format
`verify_manifest`, ed25519 `verify_strict` underneath); tampered-manifest and
tampered-blob rejection tests. Op-log `sig` fields are opaque payload — never
validated, never regenerated (D5-1). Snapshot export/import authz per FR-4.

### NFR-4: Build/test health
Workspace builds; fe-hexon, fe-hexon-registry, fe-api, fe-database test suites
green. Per repo convention, TDD tests run scoped per task
(`cargo test -p <crate> ...`); the **full** workspace sweep (fmt, clippy
`-D warnings`, test) runs **once** at the end of the track, not per fix.

### NFR-5: Snapshot size posture (documented, not engineered)
v1 builds snapshots in memory via ZIP; acceptable for dev-scale petals.
Document the memory profile in fe-hexon/AGENTS.md; streaming/HashSeq
containers for very large petals are explicitly deferred to the delta track
(amendment A1 territory).

## User Stories

- **As a node operator**, I want to export a petal — everything: nodes,
  properties, roles, history, and asset blobs — into a single `.hexon` file,
  so that I can back it up, move it to another machine, or hand it to another
  operator. *Given* a live petal with nodes, assets, and edit history, *when*
  I call the snapshot export endpoint as Manager+, *then* I receive one
  `.hexon` file containing the full state and op-log.
- **As a node operator**, I want to restore that file on a fresh node, *given*
  a node that has never seen the petal, *when* I import the snapshot, *then*
  the petal appears with identical content and its history intact, and new
  edits sequence correctly after the imported history.
- **As a hexon publisher**, I want one format and one toolchain, *given* a
  hexon built with `HexonBuilder`, *when* I publish it to the hosted registry
  or through the node API, *then* both accept it — no more `.fecrate` vs
  `.hexon` split.
- **As a platform developer**, I want the format crate dependency-light,
  *given* a tool that only reads hexons (e.g. gis-tile-etl), *when* it depends
  on fe-format, *then* it does not pull tokio/policy/network crates.

## Technical Considerations

- **Order of operations matters:** unification (FR-1..FR-5) lands before the
  snapshot (FR-6..FR-10), so PetalSnapshot is only ever implemented once, on
  the canonical types.
- **`DbCommand` payload shape:** `PetalSnapshotData` (table row arrays +
  ordered op-log entries) should live where both fe-database and fe-api can
  see it without violating NFR-1 — candidate: serde-plain structs in
  fe-runtime messages, converted to fe-format archive types at the fe-api/
  fe-hexon layer.
- **Import atomicity:** SurrealDB embedded has transactions but the restore
  spans many tables + the blob store. Strategy: verify blobs first (cheap,
  outside DB), then restore rows in one DB-thread handler invocation;
  on any row failure, delete the partial petal (cascade delete already exists
  via `DeleteEntity`) before returning the error.
- **`model`/`asset` row scoping:** these tables are asset-keyed, not
  petal-keyed; the snapshot carries only rows referenced by the petal's nodes.
  Import must upsert-or-skip asset/model rows that already exist (same
  BLAKE3 identity ⇒ same content), which does not violate refuse-on-conflict
  (that applies to the petal identity).
- **`fe-hexon-registry` and `fe-terrain` should need minimal change** — they
  are already fe-format-based; treat any required change there as a smell to
  investigate before proceeding.
- **Docs convention:** terse one-line doc comments in code; rationale in
  directory AGENTS.md (`fe-format/AGENTS.md` and `fe-hexon/AGENTS.md` are
  created by this track; `fe-api/AGENTS.md`, `fe-hexon-registry/AGENTS.md`
  updated).

## Constraints (binding, ratified 2026-07-16)

1. **One shared implementation.** fe-format `.hexon` v1.0.0 core is canonical
   (manifest.json + entries.json + assets/{blake3} + type-specific sections).
   Additive extension fine; a parallel format is not.
2. **Dev-phase breakage allowed.** Registry compatibility may break freely;
   `.fecrate`, its `crate_id` manifest, and installed-crate DB rows get **no
   migration path** — delete/replace.
3. **Snapshot completeness.** The PetalSnapshot must carry the FULL state and
   ops log of the petal's SurrealDB instance: all petal-scoped rows, asset
   rows + blobs, petal-scoped roles, and op_log entries with HLC lamport
   clocks and `sig` fields verbatim (sigs are placeholders per D5-1 — carry
   as-is, do not invent signing).

## Out of Scope (Non-Goals)

- Delta/replay hexons, op replay/materialization, hash-chained sequences,
  time-travel checkpoints — all `hexon_delta_format_20260710`
  (foundry-candidate).
- Real op-log signing (D5-1 prerequisite work) or any signature-chain design.
- The §D4 log-first write-path inversion.
- Merge-on-import / CRDT conflict resolution — import is refuse-on-conflict.
- HashSeq/streaming containers, zstd compression — ZIP retained per A1.
- Marketplace, payment enforcement, or hexon-foundry concerns.
- Renaming `/api/v1/crates/*` routes or the `crate_registry`/`crate_entry`
  table names beyond what re-typing requires.
- Mobile/WASM, UI work beyond keeping existing screens compiling.

## Open Questions

1. **Op-log petal-filter completeness.** Some `OpType`s (e.g. `AssignRole`,
   `RevokeSession`) may reference the petal only via scope strings, or not at
   all. The filter predicate (payload `petal_id` OR scope contains
   `PETAL#<id>`) is versioned in `SnapshotMeta`; entries not attributable to
   any petal are excluded. Confirm during Phase 4 that no petal-mutating
   `OpType` escapes the predicate; extend it (and bump its version) if one
   does.
2. **`iot_reading` volume.** Included by default (it is petal-scoped state);
   if a real petal's readings dominate snapshot size, add an
   `?include_iot=false` query flag — decide at Phase 4 verification, don't
   pre-build.
3. **Inherited roles.** Only roles scoped *at* the petal are exported;
   verse/fractal-level roles that affect the petal are intentionally not
   portable (they belong to the destination's hierarchy). Flag in docs;
   revisit if operator feedback disagrees.
