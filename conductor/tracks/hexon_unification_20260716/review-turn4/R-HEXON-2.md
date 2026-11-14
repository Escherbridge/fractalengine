---
type: Review
track: hexon_unification_20260716
gate: R-HEXON-2
reviews: [HEXON-04, HEXON-06, HEXON-09, HEXON-10, HEXON-11, HEXON-12, HEXON-13]
security_review: true
date: 2026-11-13
---

# R-HEXON-2 — adversarial review (turn 4)

Diffs read against `7af0a144`, prompted to refute. Security pass covered the snapshot import trust boundary (API authz, DB restore, op-log replay, blob restore), registry install gates and the crate registry rows. No cargo run.

R2-01 FIXED - fe-database/src/handlers/petal_snapshot.rs:521 - restore never checked that a snapshot's node_id, reading_id or op-log marker ids were new here, so a Manager of one fractal could plant a second row under another petal's node_id. In-transaction collision guards (node_id incl. referenced-but-absent ids, reading_id, op-log request/operation markers, SceneAssetAdopted asset_id) THROW before any write -> `ImportError::Collision`.
R2-02 FIXED - fe-database/src/handlers/petal_snapshot.rs:470 - op-log replay accepted any entry matching one clause of the OR predicate; now every payload `petal_id`, `scope` and `node_id` must stay inside the petal (scope under the locally derived petal scope), and ExtensionAudit/ElevatedQuery entries (never exported) refuse the snapshot.
R2-03 FIXED - fe-database/src/handlers/petal_snapshot.rs:395 - field_def rows were accepted when their scope named `PETAL#<id>` under ANY verse/fractal; now the scope must equal or sit under `<local parent fractal scope>-PETAL#<id>`.
R2-04 FIXED - fe-database/src/handlers/petal_snapshot.rs:230 - the DB import relied on the API alone for the parent fractal; it now refuses (Invalid) when `fractal_id` is missing or not a local fractal, and derives the petal scope from it.
R2-05 FIXED - fe-database/src/handlers/petal_snapshot.rs:335 - the failure cascade deleted every node/room/model/reading keyed to the petal id and field_defs ending in `-PETAL#<id>` in any verse, including rows the import never wrote; pre-existing rows keyed to the petal id (or its scope) now refuse the import as PetalExists, and the cascade deletes field_defs by exact petal scope.
R2-06 FIXED - fe-database/src/handlers/petal_snapshot.rs:367 - duplicate node_id / reading_id inside one snapshot were accepted; refused.
R2-07 FIXED - fe-database/src/handlers/petal_snapshot.rs:289 + fe-api/src/petal_snapshot.rs:298 - an invalid snapshot surfaced as a 500 RequestFailed; Invalid and Collision now reply PetalSnapshotImportRefused with the reason, the route answers 400 for invalid and 409 for exists/collision.
R2-08 FIXED - fe-hexon/src/snapshot.rs:250 - restore stored every shipped blob, including ones no asset row references (orphan storage per upload); such an archive is now refused before any blob is written.
R2-09 FIXED (R1-08) - fe-hexon/src/registry.rs:189 - `install` now calls `check_content_installable` before the blob loop; the refusal journey gained a PetalSnapshot row (nothing stored, no record).
R2-10 FIXED - fe-database/src/handlers/crate_registry.rs:25 - delete-then-create (and uninstall's two deletes) were separate statements; a failed CREATE left the pair with no row. Each is now one transaction.
R2-11 HANDED-OFF (owner of fe-runtime/src/scene_write.rs, R-INT gate) - fe-runtime/src/scene_write.rs:673 - SEC-INBOX 9: ImportPetalSnapshot is classified with reads, so bound scene hooks never judge imported nodes; classify it as a scene write with per-node create inputs capped by MAX_HOOK_CALLS_PER_WRITE, or refuse the import while a scene hook is bound for the target scope.
R2-12 HANDED-OFF (I-RUNTIME, I-DB-1, then R-HEXON-3 for fe-hexon/src/registry.rs) - fe-hexon/src/registry.rs:300 - uninstalling a build from petal A while B holds it leaves A's crate_registry row (it reappears on reload); add `DbCommand::UninstallCrateInPetal { hexon_uri, petal_id }`, dispatch it to `crate_registry::uninstall_crate_in_petal` (currently dead), and send it from `uninstall` in the held-elsewhere branch.
R2-13 KEPT (content-addressed, hash-verified, bounded by MAX_SNAPSHOT_ARCHIVE_BYTES) - fe-api/src/petal_snapshot.rs:262 - blobs are written before the DB import can refuse; a refused import leaves verified orphan blobs for GC.
R2-14 DEFERRED (trigger: per-owner asset quota enforced on import) - fe-database/src/handlers/petal_snapshot.rs:430 - an asset row new to this node keeps its `owner_did` verbatim, so its bytes count toward that DID's quota.
R2-15 KEPT (reason names no id; node ids are ULIDs) - fe-database/src/handlers/petal_snapshot.rs:521 - the collision refusal is a one-bit existence probe for a Manager.
R2-16 DEFERRED (trigger: imported rows published through publish_clocked_row, group CLOCK) - fe-database/src/handlers/petal_snapshot.rs:367 - imported rows keep their clocks verbatim; harmless while ids are fresh locally, but must be re-stamped or refused once an import replicates.
R2-17 KEPT (local op-log rows carry zero signatures, nothing to verify) - fe-database/src/handlers/petal_snapshot.rs:470 - SEC-INBOX 6 also suggested signature checks; containment, freshness and refused types are the enforceable part today.
R2-18 HANDED-OFF (I-BATCH) - conductor/test-batches.md - `fe-database/tests/petal_snapshot_test.rs` is reached by no batch filter; add `cargo test -p fe-database --test petal_snapshot_test` to the region that owns fe-database handlers.
R2-19 KEPT (verified) - fe-api/src/petal_snapshot.rs:99 - export and import are Manager+ through role floor, token scope coverage and `authorize(Manage)`, deny with the existence-safe 404, stream the upload under a cap, and check the echoed correlation id.
R2-20 KEPT (verified) - fe-database/tests/petal_snapshot_test.rs:347 - FR-9 leg proves rows -> archive -> fresh DB -> re-export is byte-identical with volatile stamps excluded; registry_test FR-10 leg downloads the PetalSnapshot byte-for-byte.
R2-21 KEPT (pre-existing across fe-api) - fe-api/src/hexon.rs:241 - `registry.lock().unwrap()` panics on a poisoned mutex under panic=abort.
SEC-INBOX 5: CONFIRMED-FIXED (at the DB layer, R2-01/R2-02; the API relies on it)
SEC-INBOX 6: CONFIRMED-FIXED (R2-02 containment, refused audit types, marker and adoption freshness; signatures per R2-17)
SEC-INBOX 8: CONFIRMED-FIXED (R2-01, R2-06; record key need not equal node_id since nodes use generated record ids, a colliding record id already fails CREATE)
SEC-INBOX 9: HANDED-OFF (R2-11)

Verdict: PASS-WITH-FIXES (10 fixed, 3 handed off, 3 deferred).
