---
type: Review
---

# R5-CLOCK: adversarial and security review of CLOCK5-01, CLOCK5-02, CLOCK5-04, CLOCK5-05

Base `a85dc9ad`. The reviewer read the diffs and the code around them and tried to refute each one. The security review covers the publisher on the replication path and the revoke frontier. Nothing was compiled or run.

## Findings

F1 FIXED - fe-database/src/field_clock/mod.rs:1-14 - CLOCK5-04 kept `#![allow(unused_imports)]` with a comment saying `apply_keys` is reached only through submodule paths, but CLOCK5-01 now calls `field_clock::apply_keys` (publisher.rs:310), so the comment was wrong and the allow hid that drift. The four re-exports nothing uses (`admissible`, `VerseView`, `rejudge_authors`, `load_rows_by_authors`) are now deleted, along with the module-wide allow and its comment. One item-level allow remains, on `pub use seed_legacy_floor`, because only a replicated_row test reaches it through that path.
F2 HANDED-OFF (owner of field_clock/judge.rs and rejudge.rs, later round) - fe-database/src/field_clock/judge.rs:96, rejudge.rs:32 - removing the blanket `dead_code` allow (CLOCK5-04) exposes two functions nothing calls: `judge::admissible` and `rejudge::rejudge_authors` (callers use `rejudge_authors_detailed`). Each now raises a dead-code warning, not an error. Either delete each one, or give it `#[allow(dead_code)]` with a one-line reason.
F3 HANDED-OFF (fe-database/src/role_grants.rs owner, later round; security) - fe-database/src/role_grants.rs:622 with fe-policy/src/role_grants.rs:144,180 - this is an off-by-one in the revoke frontier. A window is `[since, until)` (`at < until`), and the frontier is the revoked author's newest stored write W, so the window ends at W and leaves W out. The revoker's own copy of M's last write is demoted on re-judge on every peer. Spec section 3.7 says the opposite: only versions stamped "above the frontier" are refused, and residual risk 1 says the window covers "at or below" it. Fix: `Some(author_frontier(..).await?.map_or(0, |frontier| frontier.saturating_add(1)))`, and keep the empty-frontier case at 0. Then the CLOCK5-02 leg in role_propagation_local_test.rs:386-429 must change: expect `until: write_hlc + 1`, and add `held_at(write_hlc)` must be true. Until that lands, the leg pins the current behaviour.
F4 HANDED-OFF (owner of scene_publication/inbound.rs, later round) - fe-database/src/handlers/scene_publication/inbound.rs:604-608 - `pending_local_keys` only protects an unpublished local key while the journal `after_state` equals the stored row exactly. Once a first remote merge changes another key, a second merge before publication finds the two unequal and puts the cached winner back over the local value. The publisher then sees committed == winner, stamps nothing, and the local edit is lost silently. Fix: run the inbound check with the publisher's `journal_covers` rule (publisher.rs:259). The trigger is an offline edit followed by two or more remote snapshots before `recover()`.
F5 HANDED-OFF (I5-DB, which owns handlers/replicated_row.rs "only if CLOCK5-01 asks") - fe-database/src/handlers/replicated_row.rs:3956 - CLOCK5-01 shipped no acceptance test, because node_lifecycle_test cannot place a merge between commit and publish. Extend `stale_live_row_cannot_resurrect_locally_tombstoned_node` with these steps, in order:
  1. Commit a local edit of key A at journal hlc H, with no sender.
  2. Apply a clocked remote snapshot from an admitted Editor that sets key B at H' > H.
  3. Call `scene_publication::prepare(db, id, local)` followed by `emit`.
  4. Assert the published `_fe_clock.own` has A at H.
  5. Assert the stored `row_field_clock` winner of B is unchanged.
  6. Assert `node_scene_clock.hlc == H'` and its publisher is still the remote author.
  7. As a negative leg, assert a tombstone below a known later tombstone is still refused.
  Filter: `cargo test -p fe-database --lib -- handlers::replicated_row`.
F6 FIXED - fe-database/src/handlers/scene_publication/diagnostics.rs:69-72 - three THROW messages that the publication transaction can raise still reported `unrecognized_database_error`: `scene clock was not written` (mod.rs CLOCK_VERIFY), and `field version was not written` and `field clock was not written` (field_clock/store.rs:15,21). They now map to `scene_clock_not_written` and `field_clock_not_written`.
F7 KEPT (correct as built) - fe-database/src/handlers/scene_publication/publisher.rs:191 - the task text asked the v1 `_fe_scene` marker to carry `$scene_clock`. The publisher instead keeps the marker at the journal clock, signed by the actor, while node_scene_clock keeps the known clock. That is required: inbound denies a marker whose publisher is not the sending author (inbound.rs:274). Every peer upgrades together, and a clocked receiver judges by `_fe_clock` per key, so a marker below the receiver's known clock is not refused.
F8 KEPT (harmless) - publisher.rs:144 - `holds_winner` is true when the actor holds any winning key, including keys from earlier publications. Below a known clock, an edit whose own keys all lost therefore re-emits a snapshot that changes nothing. Receivers' `upsert_own` keeps the same versions, so the merge returns NotApplicable. The only cost is one redundant blob.
F9 HANDED-OFF (I5-DOC-DB) - fe-database/src/handlers/scene_publication/AGENTS.md §Wire and ordering "Per-key clock" - publisher.rs:139 points there, but the section has no text yet. Add: "Below a known later scene clock a journaled edit still publishes its own keys at the journal hlc while it holds a winning key; node_scene_clock is rewritten unchanged (never lowered), and the `_fe_scene` marker keeps the journal clock signed by the publisher. A journal after_state may differ from the committed row only on keys a stored non-floor winner holds with the committed value (`journal_covers`). A tombstone keeps the strict rule."
F10 KEPT (verified) - CLOCK5-05, petal_render_distance.rs:270-281 and verse_settings.rs:278-283 - the key names `render_distance` and `ts_mode` are column names in `keys_of`, and the shape matches `encode_own` (wire.rs:135: `own.<key>.hlc` is a string). If the handler fell back to a plain `replicate_row`, there would be no `_fe_clock` and both assertions would fail.
F11 KEPT (verified) - CLOCK5-02, role_propagation_local_test.rs:347-429 - the leg uses real stores (a raw `row_field_version` row and the real revoke path). The burn assigns separate the revoke clock from W; `author_frontier` reads only `row_field_version` and `node_scene_clock`, so M's role statements do not move the frontier. The leg fails if `unwrap_or(0)` becomes the revoke clock, or if the window closes at the revoke hlc.

## Security review (replication path)

- Authn/authz: `prepare` still requires Editor through `prepare_scope_permission` (publisher.rs:61-67) and the journal boundary check `require_committed_boundary(..., actor)`. The `journal_covers` relaxation never lets the actor sign a value it did not write. A key that differs from the journal is accepted only when a stored non-floor winner already holds the committed value, and such a key is never stamped as the actor's own (stamp_own_versions:338-346). A stale cache entry has no backing version, so it is not a winner and the relaxation fails closed.
- Tombstone: if the journal and the row differ on the tombstone key, `journal_covers` returns false, so the strict path runs: a unique local `NodeTombstoned` op and `known <= clock`. A remote tombstone is never re-signed.
- Clock: node_scene_clock is never lowered. `scene_clock = max(known, journal clock)` is bound to both CLOCK_WRITE and CLOCK_VERIFY, so they stay consistent. CLOCK_FENCE still guards the value read before the write.
- No current-role check was put back on replication: merge_once still calls `prepare_scope_permission(.., RoleLevel::None)` (inbound.rs:533).
- No secrets are logged. No new send path was added (`enqueue` is unchanged).
- SEC-INBOX 4: STILL-FIXED - fe-database/src/role_grants.rs:171 (`proven_windows`) and :516-525 (`place_statement` keeps only proven windows). The wire window cannot widen. F3 is the separate off-by-one in local frontier derivation.

## Completeness

CLOCK5-01 STILL OPEN - publisher and diagnostics are done (publisher.rs:137-146,182-184,259-311; diagnostics.rs:67-68), but the acceptance test is missing (F5, I5-DB) and the AGENTS.md text is missing (F9, I5-DOC-DB).
CLOCK5-02 COMPLETE fe-database/tests/role_propagation_local_test.rs:347-429. F3 will change its expected `until`.
CLOCK5-04 COMPLETE fe-database/src/field_clock/mod.rs:1-14 after F1. The dead functions are F2.
CLOCK5-05 COMPLETE fe-database/src/handlers/petal_render_distance.rs:254-281, fe-database/src/handlers/verse_settings.rs:278-283

Verdict: PASS-WITH-FIXES (F1 and F6 fixed here; F3 is a security-relevant off-by-one and F4 a silent lost-edit path, both handed off).
