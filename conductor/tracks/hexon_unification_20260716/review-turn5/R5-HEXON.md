---
type: Review
track: hexon_unification_20260716
gate: R5-HEXON
reviews: [HEXON5-01, HEXON5-03, HEXON5-04, HEXON5-05, HEXON5-06, HEXON5-07, HEXON5-08, HEXON5-09]
security_review: true
date: 2026-11-14
---

# R5-HEXON: adversarial review and fix (round 5)

Read each slice task, its commit (edecb18a, 92a80db3, 062c0ecb, 617194da, 1043f2f9, a639dcda, b9962c54, 7f409a8d) and the code at HEAD, trying to refute it. Security review done in the same pass (authn/authz, trust boundaries, fail-closed, forgery). No cargo was run.

Second pass (round-5 resume, after I5-API 34c611c1 and I5-MANIFEST 518714e6 landed): no owned file changed since f1ddf509. Every finding was re-checked at HEAD. F6 and F8 are now resolved by their owners. F3, F4, F5 and F7 are still open with the I5 slices that run after this gate. The second pass found no new defect; F13 and F14 record two suspected issues it ruled out.

## Findings

- F1 FIXED - fe-api/src/petal_snapshot.rs:66-77,355-391 - HEXON5-05 rolled back newly written blobs on every non-success outcome, including a timeout, a dropped reply and an unexpected/mismatched reply. In those cases the DB thread may still commit the import, leaving rows that point at deleted blobs. `db_round_trip` now returns `RoundTripFailure { response, unsent }`. Blobs are discarded only on Refused, RequestFailed, a failed send or the petal-row mismatch. Every unknown outcome keeps them and logs a warning: an orphan blob is preferred to a dangling row. Re-checked: RequestFailed arises only after `run_restore` failed and the cascade cleanup ran (handlers/petal_snapshot.rs:303-316); a failed post-commit ImportPetal audit write is logged and still returns Ok (:330-332), so no committed import reaches the rollback.
- F2 FIXED - fe-database/src/space_manager.rs:211,283,329 - the room/model metadata UPDATEs compared the record `id` with a bound string (`WHERE id = $id`). SurrealDB 3 does not coerce a string to a record id (the import code uses `type::record` for exactly this, handlers/petal_snapshot.rs:512). They now use `WHERE id = type::record($id)`.
- F3 HANDED-OFF (I5-DB, or round 6 if I5-DB cannot take an extra file) - fe-database/src/handlers/preconditions.rs:153 - `require_record_in_petal` still uses the same `WHERE id = $id` string comparison at HEAD. HEXON5-08's journey (fe-database/tests/petal_snapshot_test.rs:292-301) passes `<string> id` and will fail with "matched no room" until the line becomes `SELECT petal_id FROM {table} WHERE id = type::record($id) LIMIT 1`. Its only callers are require_room_in_petal and require_model_in_petal (:107,:117), used only by space_manager, which has no production caller, so only the test is affected today.
- F4 HANDED-OFF (I5-RUNTIME, I5-DB) - fe-runtime/src/messages.rs:1357, fe-database/src/lib.rs:2375, fe-test-harness/src/peer.rs:1117 - still open at HEAD: `DbCommand::UninstallCrate` has no `petal_id`, so fe-hexon/src/registry.rs:410 does not compile yet. Both dispatch arms destructure `{ hexon_uri }` with no `..`, so adding the field breaks compilation until each arm is rewritten. That makes a silent global uninstall impossible as long as the new arms do not use `..`. Required arm: `Some(p) => uninstall_crate_in_petal(&db, &hexon_uri, &p)`, `None => uninstall_crate(&db, &hexon_uri)`, with the same reply.
- F5 HANDED-OFF (I5-RUNTIME, I5-DB) - fe-runtime/src/messages.rs:1191, fe-database/src/lib.rs:2387, fe-runtime/src/scene_write.rs:869 - still open at HEAD: `ImportPetalSnapshot.actor` is not on the variant yet. The lib.rs arm must pass `actor` to `import_petal_snapshot_reply(&db, *rows, correlation_id, actor)`, and the scene_write fixture needs `actor: None`.
- F6 HANDED-OFF (I5-API) - resolved at HEAD by 34c611c1 - `pub mod imported_history;` is at fe-api/src/lib.rs:37, and the route is at fe-api/src/server.rs:288-291, in the same token-authenticated router as `/snapshot.hexon` (auth layer at :553-555).
- F7 HANDED-OFF (I5-BATCH) - conductor/test-batches.md:362 - still open at HEAD: the R10 lib filter lacks `handlers::crate_registry::`, so `crate_registry_rows_follow_per_petal_uninstall_journey` (fe-database/src/handlers/crate_registry.rs:163) is reached by no batch.
- F8 HANDED-OFF (I5-MANIFEST) - resolved at HEAD by 518714e6 - fe-hexon/Cargo.toml no longer lists reqwest `multipart`.
- F9 DEFERRED (trigger: two imports or an install sharing blobs run concurrently in a multi-user host, or asset GC starts scanning the blob dir) - fe-hexon/src/snapshot.rs:283-294 - `newly_stored` is per-call, and `FsBlobStore::store` (fe-hexon/src/registry.rs:97-127) can report `Ok(true)` to two concurrent writers of one hash. If import A writes blob X, import B (or a crate install) finds X already present and commits a row on it, and A is then refused, A's rollback deletes X under B. Mitigation: a blob-store write lock spanning restore-to-reply, or reference-checked removal.
- F10 KEPT (the exporter never emits an empty petal object; an unknown petal is refused before export) - fe-format/src/snapshot.rs:144-148 vs fe-runtime/src/petal_snapshot.rs:57 - the producer counts `petal` as 1 for any non-null row, while the importer counts 0 for an empty `{}` object. Only a hand-built `{}` petal would diverge, and it is refused (fail closed).
- F11 KEPT (pagination contract: the client stops on an empty page) - fe-api/src/imported_history.rs:72 - `next_after_lamport` is returned even when the page is shorter than the limit.
- F12 DEFERRED (trigger: the next slice that tests space_manager) - fe-database/src/space_manager.rs:451-520 - ten `#[ignore]` placeholder tests with empty bodies assert nothing. Swap them for one journey when space_manager gets real coverage.
- F13 KEPT (the DB thread awaits each command serially, fe-database/src/lib.rs:2375-2381, so no install interleaves) - fe-database/src/handlers/crate_registry.rs:116-139 - `uninstall_crate_in_petal` runs its delete, holder check and entry delete as three statements rather than one transaction. A crash between them leaves orphan `crate_entry` rows, which the next install of that URI replaces.
- F14 KEPT (one import per petal) - fe-api/src/imported_history.rs:22-23 - the cursor is lamport-exclusive, so equal lamports from two imports of one petal could straddle a page. This cannot happen: the import refuses while any `op_log_imported` row names the petal (handlers/petal_snapshot.rs:420), and deleting a petal removes those rows (handlers/entity.rs:379-396). Lamports within one import are strictly ascending (fe-format check_oplog_order).

## Security review (refute pass)

- Uninstall: the registry sends `Some(petal)` only (fe-hexon/src/registry.rs:408-414). `uninstall_crate_in_petal` deletes only that petal's row and drops shared `crate_entry` rows only when `SELECT ... LIMIT 1` finds no holder (crate_registry.rs:116-139). Global uninstall is unreachable from the registry. The half-landed hazard (SEC-INBOX 16) cannot compile away silently while the arms destructure without `..` (F4).
- imported_by: an archive value is removed before the stamp (handlers/petal_snapshot.rs:606-610). The stamp is the token's `sub` (fe-api/src/petal_snapshot.rs:346; the gate command carries `actor: None` and never reaches the DB channel), bounded to 256 characters, dropped on export (`QUARANTINE_COLUMNS`, :175) and read only by the history listing (fe-api/src/imported_history.rs:22). It is never an authorization input: a grep over fe-api, fe-database, fe-sync, fe-ui, fe-runtime and fe-policy finds no other reader.
- imported-history: a ULID check (400, independent of existence), then `manager_covers` (role floor, scope, registry Manage) with the shared existence-safe 404. Petal, cursor and limit are bound parameters, the limit is clamped to 1..=500, the cursor saturates at i64::MAX, and payloads are excluded (imported_history.rs:22-23,40-56).
- Blob rollback: only hashes for which `FsBlobStore::store` returned `Ok(true)` are removed. `remove` validates the hash via `blob_path` and ignores NotFound (fe-hexon/src/registry.rs:131-137). A mid-loop store failure rolls back this call's own writes (fe-hexon/src/snapshot.rs:283-294). F1 closes the commit-after-timeout hole; F9 records the concurrent-sharing hole.
- verify_strict: every manifest check routes through `fe_format::verify_manifest` (signature.rs:101): fe-format/src/integrity.rs:284, fe-hexon/src/package.rs:204,531, fe-hexon/src/registry_source.rs:1373. The registry index uses verify_strict (registry_source.rs:802) and entitlement proofs do too (fe-format/src/entitlement.rs:860). No non-test `ed25519_dalek::Verifier` import remains in fe-format, fe-hexon, fe-hexon-registry, fe-terrain, fe-api or fe-sync. The test row is a true mutation check (signature.rs:219-242): the same input passes plain `verify`.
- Entitlement: refusals stay errors and are only logged quietly (composite.rs:131,166; registry.rs:247); both arms still return a miss, so they fail closed. `refused_before_read` runs before any archive or container read (hexon_source.rs:441-448,588-594,611), so no disk read happens. The `debug!` fields are evaluated only when that level is enabled, so the hot path does not allocate.
- table_row_counts: checked in the parse-only pass (fe-format/src/snapshot.rs:354), before blob names are read. fe-api runs that pass before the scene gate and before any blob or row write (fe-api/src/petal_snapshot.rs:252-262). The producer (fe-hexon/src/snapshot.rs:188-193) counts exactly the rows it ships.
- Secrets: nothing new is logged except petal ids and hashes.

## SEC-INBOX

- SEC-INBOX 5: STILL-FIXED - in-transaction identity guards and the petal-exists guard (handlers/petal_snapshot.rs:410-421,667-676).
- SEC-INBOX 6: STILL-FIXED - imported history is quarantined in `op_log_imported`, op types are allowlisted (:222-232), and nothing imported enters the live op_log (:598-616).
- SEC-INBOX 8: STILL-FIXED - each node must belong to the petal (`require_petal`, :741) and identity collisions throw `COLLISION_MARKER` (:220,676).
- SEC-INBOX 16: HANDED-OFF (I5-RUNTIME, I5-DB) - see F4. Still open at HEAD (messages.rs:1357 has no field; lib.rs:2375 and peer.rs:1117 call the global uninstall). The DB-level row "uninstall in A leaves B" is in crate_registry.rs:180-182. The dispatch-level row (lib.rs arm with `Some(petal)`) is I5-DB's per the 04:10 steer.

## Completeness

- HEXON5-01 STILL OPEN - leaf done (fe-hexon/src/registry.rs:405-415 always per-petal; alias removed from fe-hexon/src/manifest.rs; journey at fe-database/src/handlers/crate_registry.rs:163; multipart dropped in 518714e6). Still missing: the variant field and arms (F4) and the batch filter (F7) - fe-runtime/src/messages.rs:1357.
- HEXON5-03 STILL OPEN - handler and API done (handlers/petal_snapshot.rs:262-268,606-610; fe-api/src/petal_snapshot.rs:346; tests petal_snapshot_test.rs:349-354,392-397 and export_share_test.rs:2000-2005). Still missing: the `actor` field and the lib.rs arm (F5) - fe-runtime/src/messages.rs:1191.
- HEXON5-04 COMPLETE - handler fe-api/src/imported_history.rs:35-74, mounted at fe-api/src/lib.rs:37 and fe-api/src/server.rs:288-291, journey export_share_test.rs:2014-2079 (R9). It compiles once F5 lands, because the journey's responder destructures `actor`.
- HEXON5-05 COMPLETE - fe-hexon/src/snapshot.rs:242-295, fe-hexon/src/registry.rs:130-137, fe-api/src/petal_snapshot.rs:355-391 (after F1); test fe-hexon/src/snapshot.rs:361-374.
- HEXON5-06 COMPLETE - fe-format/src/signature.rs:101, test :219-242.
- HEXON5-07 COMPLETE - fe-terrain/src/tiles/entitlement.rs:24, composite.rs:131,166, registry.rs:247; test composite.rs:826-879. The fe-terrain AGENTS.md section is I5-DOC-LIBS's job.
- HEXON5-08 STILL OPEN - payloads done (fe-database/src/space_manager.rs:203,269,339). The journey fails until preconditions.rs:153 resolves the record id (F3).
- HEXON5-09 COMPLETE - fe-format/src/snapshot.rs:142-163,354; tests :484-499,634-663.
- INT5-12 STILL OPEN - superseded by HEXON5-01's Option field; open on the same seam (F4) - fe-database/src/lib.rs:2375.

## Verdict

PASS-WITH-FIXES - two defects fixed in owned files (F1 rollback after an unknown outcome, F2 record-id binding). Two hand-offs have landed (F6, F8). Four remain with I5 owners: F3, which blocks a test, plus F4, F5 and F7. No security regression.
