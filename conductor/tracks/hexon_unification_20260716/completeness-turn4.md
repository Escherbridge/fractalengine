---
type: Review
track: hexon_unification_20260716
critic: C-HEXON
round: 4
date: 2026-11-13
---

# C-HEXON completeness: hexon_unification_20260716 (round 4)

Every spec requirement and plan task checked against code at HEAD (`b817b97e` plus uncommitted round-4 work in the tree). No cargo was run. "Complete" means the code and its test exist; tests have not been run (user-gated).

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 single manifest/type system | complete | `fe-hexon/src/manifest.rs:5` re-exports fe-format types, no local manifest type; `HexonType::{Script, PetalSnapshot}` `fe-format/src/manifest.rs:34,36`, exhaustive `as_str` `:74`, legacy `FromStr` `:106`, table test `:599`; licence fields ported `fe-format/src/license.rs:21-27`; `EntryKind::{Sprite, Script}` `fe-format/src/entries.rs:9,20`; fe-format is a normal dep `fe-hexon/Cargo.toml:19`; no `crate_id` in fe-hexon/fe-api/fe-database/fe-hexon-registry src |
| FR-2 single container + signing | partial | `HexonPackage::build/open` on `HexonArchive` `fe-hexon/src/package.rs:105,145`; `signature_valid` via `verify_manifest` `:204`; raw-bytes scheme gone (`fe-hexon/src/signature.rs:3` re-exports fe-format). AC "no zip entry in fe-hexon/Cargo.toml" not met: `zip = "2"` stays a direct dep (`fe-hexon/Cargo.toml:24`, used by `package.rs:147,216`, `publisher.rs:445`, `partial/p2p.rs:402`). One zip version for hexon code holds; zip 0.6.6 remains only through fe-ext |
| FR-3 runtime re-typed | complete | `InstalledHexon` `fe-hexon/src/manifest.rs:9`; registry keyed `(petal_id, hexon_uri)` `fe-hexon/src/registry.rs:188`; content-install refusal `:251`; builder output publishes, searches and downloads on `hexon_id` `fe-hexon/tests/remote_registry_test.rs:165-188` |
| FR-4 authz survives; snapshot Manager+ | complete | `fe-hexon/src/authz.rs:89-182` (Install/Write Editor+, Read Viewer+, published-binding refusal `:115`); snapshot Manager+ `fe-api/src/petal_snapshot.rs:49-56`; Editor and out-of-fractal denial plus Manager export/import `fe-api/tests/export_share_test.rs:1784`, route auth legs `fe-api/tests/api_integration.rs:992-1012` |
| FR-5 /crates re-pointed, dead code gone | complete | handlers on unified types `fe-api/src/hexon.rs:249-661`, `HexonType` parse `:424,661`; `install_crate` goes through `install_published_as` (authz + petal record) `fe-api/src/hexon.rs:334`, `fe-hexon/src/authz.rs:108`; `uninstall_as` `fe-api/src/hexon.rs:381`; e2e journey `fe-api/tests/api_integration.rs:878`; `RemoteRegistryClient::publish` `fe-hexon/src/remote.rs:128`; `fecrate` only in `docs/hexon-format-spec.md` (removal note) |
| FR-6 PetalSnapshot archive format | complete | `fe-format/src/snapshot.rs:18-60` (SnapshotMeta, OpLogRange, PetalSnapshotData incl. iot_readings), writer `:152` (canonical entries.json, oplog.jsonl `:192-216`), reader `:238` verifies a sealed archive first; member list test `:531-540` |
| FR-7 export | complete | `DbCommand::ExportPetalSnapshot` `fe-runtime/src/messages.rs:1186`, `DbResult::PetalSnapshotExported` `:1591`; one read transaction, op-log `ORDER BY lamport_clock` `fe-database/src/handlers/petal_snapshot.rs:19-33`; `OpType::ExportPetal` `:56`; blob attach off the DB thread `fe-hexon/src/snapshot.rs:131`; route `GET /api/v1/petals/{petal_id}/snapshot.hexon` `fe-api/src/server.rs:285`, `application/x-hexon+zip` + disposition `fe-api/src/petal_snapshot.rs:159-161` |
| FR-8 import/restore | partial | refuse existing `fe-database/src/handlers/petal_snapshot.rs:227,247`, collision guards `:250`, one restore transaction `:327-539`, HLC advance `:260`, `OpType::ImportPetal` `:266`; blob verify-then-write `fe-hexon/src/snapshot.rs:243`; route `fe-api/src/petal_snapshot.rs:207`. Open: REST import never passes the scene-hook door (SEC-INBOX 9 API side, R-INT-4 F7), so a bound scene hook never judges imported nodes; snapshot replies are not in `fe-runtime/src/app.rs` correlation maps (R-INT-4 F8: a failed import answers 504); refused imports leave verified blobs (R-INT-4 F11 / R2-13 KEPT) |
| FR-9 round-trip determinism | complete | `fe-database/tests/petal_snapshot_test.rs:285` (second import refused `:348`, byte-identical re-export `:353,376`, far-future lamport leg `:389`); batch R10 filter `conductor/test-batches.md:363` |
| FR-10 registry serves snapshots | complete | `fe-hexon-registry/tests/registry_test.rs:268-322` (publish, `?type=petal_snapshot` only hit, byte-equal download); type filter `fe-hexon-registry/src/routes.rs:324,361` |
| NFR-1 layering | complete | fe-format deps `fe-format/Cargo.toml:19-33` (no tokio/crossbeam/DB/policy); fe-runtime carries serde rows `fe-runtime/src/petal_snapshot.rs` |
| NFR-2 thread topology | complete | DB work only in `fe-database/src/handlers/petal_snapshot.rs`; API uses `db_round_trip` `fe-api/src/petal_snapshot.rs:61` |
| NFR-3 security | partial | verify before install/import (`fe-hexon/src/package.rs:204`, `fe-format/src/snapshot.rs:238`); but `verify_manifest` calls `verify`, not `verify_strict` (`fe-format/src/signature.rs:100`; R1-17 DEFERRED) |
| NFR-4 build/test health | blocked-user | tests not run this round |
| NFR-5 size posture documented | missing | no memory-profile note in `fe-hexon/src/AGENTS.md` (sections end at §unified-format `:270-272`) |
| P1 tasks 1-4 (variants, licence, entries, legacy parser) | complete | as FR-1 |
| P1 "Decide the fe-format/fe-hexon merge gate" | partial | de facto decided by the code (fe-format owns the shape, fe-hexon re-exports: `fe-hexon/src/manifest.rs:5`, `signature.rs:3`) but not recorded in spec/plan or crate_consolidation_r2 |
| P2 tasks (promote dep, signing, builder, registry, handlers, p2p, authz, publish) | complete | as FR-1..FR-5; p2p kind filter `fe-hexon/src/p2p/discover.rs:51` |
| P2 "Rebuild HexonPackage on HexonArchive; remove zip 0.6" | complete (code) | zip 0.6 gone from fe-hexon (`fe-hexon/Cargo.toml:24` is zip 2, R1-09 applied); task box still unticked |
| P3 "Make install_crate real" | complete (code) | `fe-api/src/hexon.rs:309-358`; plan note "routing not verified" is stale |
| P3 "End-to-end publish flow test" | complete (code) | `fe-api/tests/api_integration.rs:878` (HexonBuilder -> publish -> install -> installed/search -> entries -> asset, Viewer denied install); plan note stale |
| P4/P5 tasks | complete | as FR-6..FR-9 |
| P5 "fe-hexon-registry PetalSnapshot" | complete (code) | `fe-hexon-registry/tests/registry_test.rs:318`; plan note "no PetalSnapshot reference" is stale (the plumbing needed none in src) |
| P6 "Create fe-format/AGENTS.md + fe-hexon/AGENTS.md ..." | partial | `fe-format/src/AGENTS.md` has §signature/§integrity but no §petal-snapshot, which `fe-hexon/src/AGENTS.md:272` and `fe-database/src/handlers/AGENTS.md:204` point at; `fe-api/AGENTS.md:2454` §petal-snapshot exists; `fe-hexon-registry/AGENTS.md` does not mention petal_snapshot; NFR-5 note missing |
| P6 "Update docs/hexon-format-spec.md" | complete | `docs/hexon-format-spec.md:187,456-463,515,565`; the `hexon_path_asset_20260713` cross-mark has no target (no such track directory) |

## Excluded by the scoper (not gaps)

- blocked-user: Phase 1, 2, 3, 4, 5 and 6 Verification lines (test runs, manual curl/export/import, Open Question 2 with real data).
- obsolete: Phase 6 "THE single full workspace sweep" (replaced by `tools/qa/test_batches.ps1`; no clippy).

## Gate hand-offs (did each reach its owner?)

- R1-08 reached: FIXED by R2-09 (`fe-hexon/src/registry.rs:251`).
- R1-09 reached: I-LIBS-1 replaced zip 0.6 with zip 2 (`fe-hexon/Cargo.toml:24`).
- R1-10 NOT honoured: I-LIBS-1 (`ba48aad5`, after R-HEXON-1) added `"multipart"` to reqwest (`fe-hexon/Cargo.toml:41`) although `publish` posts raw bytes (`fe-hexon/src/remote.rs:128`). Harmless, unused feature.
- R1-11 reached: no `HexonKind` left in fe-api (`fe-api/src/hexon.rs:424,661` parse `HexonType`).
- R1-12 closed in effect: FR-9 asserts byte-identical archives (`fe-database/tests/petal_snapshot_test.rs:353,376`).
- R2-11 / SEC-INBOX 9 door side reached (R-INT-1: `fe-runtime/src/scene_write.rs:35,533,719`); API side still open (R-INT-4 F7).
- R2-12 NOT landed: no `DbCommand::UninstallCrateInPetal`; `fe-hexon/src/registry.rs:398-411` still keeps DB rows in memory-only uninstall; R-INT-5 re-routed it.
- R2-18 reached: `conductor/test-batches.md:363` (`--test petal_snapshot_test`).
- SEC-INBOX 5, 6, 8: closed by R-HEXON-2 (CONFIRMED-FIXED lines). SEC-INBOX 9: door CONFIRMED-FIXED (R-INT-1), API side open (below).

## HANDED-OFF (owner = next round)

- HANDED-OFF (round 5, owner of `fe-api/src/petal_snapshot.rs`): call `scene_gate::admit(Door { claims, kind: SceneCallerKind::Rest }, &snapshot_petal_id, DbCommand::ImportPetalSnapshot{..})` before `db_round_trip` (R-INT-4 F7, SEC-INBOX 9 API side).
- HANDED-OFF (round 5, owner of `fe-runtime/src/app.rs`): add the snapshot commands/results to `correlation_of_command` / `correlation_of_result` and a reply kind (R-INT-4 F8).
- HANDED-OFF (round 5, owners of `fe-runtime/src/messages.rs`, `fe-database/src/lib.rs`, `fe-hexon/src/registry.rs`): `DbCommand::UninstallCrateInPetal` + dispatch to `crate_registry::uninstall_crate_in_petal` + send from the held-elsewhere branch (R2-12).
- HANDED-OFF (round 5, owner of `fe-format/src/signature.rs`): use `verify_strict` at `:100` (NFR-3, R1-17).
- HANDED-OFF (round 5, I-DOC-CORE): write `fe-format/src/AGENTS.md` §petal-snapshot (layout, op-log predicate v1, sealed-check-on-import), the NFR-5 memory note in `fe-hexon/src/AGENTS.md`, and a PetalSnapshot line in `fe-hexon-registry/AGENTS.md`.
- HANDED-OFF (round 5, T-HEXON): record the merge-gate decision (fe-format owns the shape) in spec/plan; tick P2 "Rebuild HexonPackage", P3 "install_crate real", P3 "E2E publish", P5 "registry PetalSnapshot"; remove the stale "not verified" notes.
- HANDED-OFF (round 5, I-LIBS owner of `fe-hexon/Cargo.toml`): drop reqwest `"multipart"` (R1-10) and decide whether FR-2's "no zip entry" AC is relaxed to "one zip version" (fe-hexon still edits archives in place).
- HANDED-OFF (round 5, owner of `fe-api/src/petal_snapshot.rs` / `fe_hexon::snapshot`): write blobs after `PetalSnapshotImported`, or sweep on refusal (R-INT-4 F11).

## User-question defaults (turn4-plan.md §9) vs code

- Q15 (import scope): matches. Import requires the parent fractal to exist locally and Manager+ there (`fe-api/src/petal_snapshot.rs:250`; DB refuses a missing fractal `fe-database/src/handlers/petal_snapshot.rs:230-238`).
- Q16 (OQ2/OQ3): matches. iot_reading rows exported (`fe-database/src/handlers/petal_snapshot.rs:31`); roles only `WHERE scope = $scope` (`:30`).
- Q17 (crate rows): matches in effect, not by composite record ids. Rows keep generated ids; `(hexon_uri, petal_id)` / `(hexon_uri, entry_id)` are UNIQUE indexes that log instead of failing on old duplicates (`fe-database/src/schema.rs:1011-1026`), and the handlers delete then create in one transaction (`fe-database/src/handlers/crate_registry.rs:24-43`). No migration.

Verdict: PARTIAL. FR-1/3-7/9/10 complete in code. FR-2, FR-8 and NFR-3 are partial, NFR-5 docs are missing, and 8 hand-offs go to round 5.
