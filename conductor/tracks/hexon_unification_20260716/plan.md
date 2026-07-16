---
type: Implementation Plan
title: Implementation Plan — Hexon Unification + Portable Petal Snapshot
tags: [hexon_unification_20260716]
resource: ./spec.md
---

# Implementation Plan: Hexon Unification + Portable Petal Snapshot

## Overview

Six phases, strictly ordered: consolidate the format layer first (Phase 1),
rebuild the fe-hexon runtime on it (Phase 2), re-point fe-api and delete the
dead `.fecrate` stack (Phase 3), then build the PetalSnapshot capability on
the now-single format — export (Phase 4), import + round-trip proof
(Phase 5) — and close with docs + the single full workspace sweep (Phase 6).

Crates touched: `fe-format`, `fe-hexon`, `fe-api`, `fe-runtime` (DbCommand),
`fe-database`, `fe-hexon-registry` (minimal). `fe-terrain` should require no
change — if it does, stop and re-check the layering.

TDD per task (red → green → refactor), tests scoped per crate
(`cargo test -p <crate>`). Per repo convention, the **full** workspace
fmt/clippy/test sweep runs once, in Phase 6 — not after every task.

---

## Phase 1: Format-Layer Consolidation (fe-format)

Goal: fe-format v1.0.0 can express everything fe-hexon's parallel types can,
additively, while keeping its lean dependency footprint.

Tasks:

- [ ] Task: Add `HexonType::Script` and `HexonType::PetalSnapshot` variants
      (TDD: serde snake_case round-trip tests red first; assert every legacy
      `HexonKind` value has a `HexonType` counterpart)
- [ ] Task: Audit and reconcile `License`/`LicenseType` — port fe-hexon-only
      fields (`payment_provider`, `payment_verification_url`, `free_entries`,
      `encrypted_key`) into fe-format additively (TDD: a fe-hexon-shaped
      license JSON fixture parses under fe-format and round-trips)
- [ ] Task: Audit and reconcile `AssetEntry`/`EntryKind` parity (e.g.
      `EntryKind::Script`/`Sprite`) so every fe-hexon entry is expressible in
      fe-format; additive fields only (TDD: fixture round-trips)
- [ ] Task: Add a `HexonKind`-string → `HexonType` compatibility parser
      (FromStr accepting legacy lowercase kind names) for API query params
      (TDD: parse table test)
- [ ] Verification: fe-format tests green; `fe-format/Cargo.toml` diff shows
      zero new dependencies; manual review of enum/serde output against
      `docs/hexon-format-spec.md` [checkpoint marker]

---

## Phase 2: Runtime Rebuild — fe-hexon on fe-format

Goal: fe-hexon becomes the runtime layer (registry, publisher, handlers, p2p,
authz, remote client) built on fe-format types; duplicate format code deleted.

Tasks:

- [ ] Task: Promote fe-format from dev-dependency to real dependency of
      fe-hexon; delete duplicate types in `fe-hexon/src/manifest.rs`,
      re-exporting `fe_format::{HexonManifest, HexonType, AssetEntry, License,
      ...}` for a smaller diff (TDD: port `manifest.rs` round-trip tests to
      fe-format types first — red until the re-export lands)
- [ ] Task: Replace `fe-hexon/src/signature.rs` raw-bytes signing with
      fe-format canonical-JSON `sign_manifest`/`verify_manifest`; keep the
      blake3 `asset_hash` helper (TDD: tampered-manifest rejection test red
      under the new scheme first)
- [ ] Task: Rebuild `HexonPackage::{build, open}` on
      `HexonArchive::{export, import}`; output extension is `.hexon`; remove
      the zip 0.6 dependency from fe-hexon/Cargo.toml (TDD: package
      round-trip + `signature_valid()` tests)
- [ ] Task: Rebuild `HexonBuilder` (publisher.rs) to emit fe-format manifests
      (`hexon_id`, `schema_version: "1.0.0"`) with canonical signing (TDD:
      built package opens via `HexonArchive::import`)
- [ ] Task: Re-type `HexonRegistry` + rename `InstalledCrate` →
      `InstalledHexon` keyed by `hexon_id[@version]` URI (adopt
      fe-hexon-registry `split_uri` parsing); redefine `crate_registry`/
      `crate_entry` row shapes around `hexon_id` — delete/replace, no
      migration (TDD: install/uninstall/list/search tests re-typed)
- [ ] Task: Re-type install handlers (model, material, skybox, sound,
      terrain, gpx_collection) on `fe_format::AssetEntry` (TDD: existing
      handler tests re-typed and green)
- [ ] Task: Re-type p2p announce/discover (`from_manifest`, `SearchQuery`
      kind filter → `HexonType`) (TDD: announcement round-trip + search tests)
- [ ] Task: Re-type authz seam — `install_as`/`uninstall_as` Editor+,
      `list_installed_as`/`search_local_as` Viewer+ preserved exactly (TDD:
      existing deny-by-default tests must pass unmodified in intent)
- [ ] Task: Add `RemoteRegistryClient::publish()` (multipart POST to the
      registry publish route) (TDD: in-process fe-hexon-registry router test —
      publish → search → download round-trip; this also proves FR-3's
      headline claim that a builder-produced hexon indexes on `hexon_id`)
- [ ] Verification: `cargo test -p fe-hexon` and
      `cargo test -p fe-hexon-registry` green; grep shows no production
      `crate_id`/`.fecrate` references in fe-hexon [checkpoint marker]

---

## Phase 3: fe-api Re-point + Dead Code Deletion

Goal: `/api/v1/crates/*` runs on the unified stack; `.fecrate` code is gone.

Tasks:

- [ ] Task: Rebuild `fe-api/src/hexon.rs` DTOs and handlers on unified types
      (`crate_id` → `hexon_id`; kind parsing via the Phase-1 compat parser)
      (TDD: handler unit tests with an in-memory registry)
- [ ] Task: Make `install_crate` real — route through
      `HexonRegistry::install_as` so the petal association is recorded and
      authz-enforced; same for `uninstall_crate` (TDD: install-then-list
      shows the petal association; Editor allowed, Viewer denied)
- [ ] Task: Delete dead `.fecrate` paths and stale comments across fe-api;
      workspace grep-clean `fecrate` (production code) (TDD: compile +
      existing fe-api tests green)
- [ ] Task: End-to-end publish flow test: `HexonBuilder` → POST
      `/api/v1/crates/publish` → GET `search`/`installed`/`{uri}/entries`
      return fe-format fields (TDD: axum integration test)
- [ ] Verification: `cargo test -p fe-api` green; manual: publish a sample
      hexon via curl against a dev node, confirm search + entries + asset
      responses [checkpoint marker]

---

## Phase 4: PetalSnapshot Format + Export

Goal: `petal_id -> .hexon` full-state export through the DB seam.

Tasks:

- [ ] Task: fe-format — `SnapshotMeta` + `PetalSnapshotData` types and
      `HexonArchive` snapshot section writer/reader
      (`snapshot/meta.json`, `snapshot/state/*.json`, `snapshot/oplog.jsonl`,
      lamport-ascending line order) (TDD: format-level round-trip with
      synthetic rows; empty-tables archive is valid)
- [ ] Task: fe-runtime — `DbCommand::ExportPetalSnapshot` +
      `DbResult::PetalSnapshotExported` carrying serde-plain row arrays +
      ordered op-log entries (keep fe-format out of fe-runtime per NFR-1)
      (TDD: message serialization test)
- [ ] Task: fe-database — DB-thread handler: collect petal row, nodes,
      node_log, field_defs, rooms, referenced models/assets, petal-scoped
      roles, iot_readings; op_log filtered by the versioned petal predicate
      (payload `petal_id` OR scope contains `PETAL#<id>`), ordered by
      `lamport_clock` (TDD: embedded-DB test seeding a petal and asserting
      row counts + strict lamport ordering; audit every `OpType` against the
      predicate — spec Open Question 1)
- [ ] Task: fe-hexon — blob attachment stage: resolve `assets.json` rows to
      blob bytes via `FsBlobStore`/asset paths, BLAKE3-verify, populate
      `entries.json` (TDD: missing-blob and hash-mismatch produce errors, not
      silent gaps)
- [ ] Task: fe-api — `GET /api/v1/petals/{petal_id}/snapshot.hexon`
      (Manager+, scope-checked like `export_petal`); writes an
      `OpType::ExportPetal` op-log entry; streams `application/x-hexon+zip`
      (TDD: authz denial test for Editor; happy-path returns valid archive)
- [ ] Verification: `cargo test -p fe-database -p fe-hexon -p fe-api` green;
      manual: export a real dev petal, unzip, eyeball `snapshot/` layout and
      `meta.json` counts; decide Open Question 2 (iot volume) with real data
      [checkpoint marker]

---

## Phase 5: PetalSnapshot Import/Restore + Round-Trip Determinism

Goal: `.hexon -> fresh DB` restore, provably lossless.

Tasks:

- [ ] Task: fe-runtime + fe-database — `DbCommand::ImportPetalSnapshot`:
      refuse-on-existing-petal_id, verbatim row restore (IDs, timestamps,
      op-log `lamport_clock`/`hlc_timestamp`/`sig` untouched) in one handler
      invocation; on failure, cascade-delete the partial petal (TDD:
      duplicate-import fails with distinct error; rows byte-equal after
      restore)
- [ ] Task: HLC advance after import — bump the HLC past imported
      `max_lamport` via the `init_hlc` advance-past-persisted path (TDD:
      synthetic far-future lamport in the snapshot; first post-import write
      exceeds it — mirrors `survives_restart_past_persisted`)
- [ ] Task: Blob restore — write `assets/{blake3}` into the blob store with
      re-hash verification BEFORE the DB restore begins; upsert-or-skip
      already-present asset/model rows (same hash ⇒ same content) (TDD:
      corrupted blob aborts import, no partial petal visible)
- [ ] Task: fe-api — `POST /api/v1/petals/import-snapshot` (multipart,
      Manager+); writes an `OpType::ImportPetal` op-log entry (TDD: authz
      denial for Editor; happy path returns the new petal id)
- [ ] Task: Round-trip determinism integration test (FR-9): seed petal with
      nodes/properties/roles/assets/history → export → import into a fresh
      embedded DB → re-export → snapshots match (table row sets equal, op-log
      count + order equal, BLAKE3 hashes verify; volatile manifest fields
      excluded) (TDD: this test IS the acceptance criterion — write it first
      against the Phase-4 export, watch it fail on import, drive import
      green)
- [ ] Task: fe-hexon-registry — publish/index/search/download a PetalSnapshot
      hexon through the existing routes; add type-filter plumbing only if
      search needs it (TDD: in-process router test)
- [ ] Verification: all Phase-5 tests green; manual: export from dev node A's
      petal, import on a wiped dev node, open the petal in-app and confirm
      nodes + history present; confirm second import is refused
      [checkpoint marker]

---

## Phase 6: Sweep, Docs, Close-out

Goal: single integrated quality gate; rationale captured per repo convention.

Tasks:

- [ ] Task: Create `fe-format/AGENTS.md` (why one format; snapshot layout;
      canonical-JSON signing decision resolving delta-spec A3; dep-footprint
      rule) and `fe-hexon/AGENTS.md` (runtime-layer role; authz seam;
      snapshot export/import flow + memory posture per NFR-5); update
      `fe-api/AGENTS.md` and `fe-hexon-registry/AGENTS.md` sections touched
      by the re-point; keep in-code comments terse one-liners pointing here
- [ ] Task: Update `docs/hexon-format-spec.md` — PetalSnapshot section,
      unified signing scheme, `.fecrate` removal note; cross-reference the
      forward-compat contract with `hexon_delta_format_20260710`; mark
      `hexon_path_asset_20260713` FR-5 satisfied-by-reference in that track's
      metadata/spec
- [ ] Task: THE single full workspace sweep — `cargo fmt --check`,
      `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
      (RUST_MIN_STACK per the surrealdb ICE workaround if needed); fix all
      fallout in one pass
- [ ] Verification: sweep green; workspace grep for `fecrate`/`crate_id`
      returns only docs/archive hits; conductor retro + tracks.md/metadata
      close-out per the track-per-feature rule [checkpoint marker]
