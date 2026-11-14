---
type: Review
title: C-CLOCK completeness critic, round 4
track: per_key_clock_20261113
created: 2026-11-13
---

# C-CLOCK: per_key_clock_20261113 completeness at HEAD

Read at HEAD after T-CLOCK, I-DOC-CLOCK, I-BATCH-1, R-INT-5 (last clock commits `47b50e0c`, `03f0adc4`,
`ebcef708`, `492f9b6d`, `3cbcf047`). Static reading only; no cargo, no tests, no simulator.

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| CLOCK-01 track (spec, plan, metadata) | complete | `conductor/tracks/per_key_clock_20261113/spec.md`, `plan.md`, `metadata.json`; registered at `conductor/tracks.md:121` |
| CLOCK-02 fe-policy grant windows, frontier, `as_of`, `held_at` | complete | `fe-policy/src/role_grants.rs:85` (`windows`), `:89` (`MAX_GRANT_WINDOWS = 8`), `:93` (`GrantWindow`), `:126` (`continuing_with_frontier`), `:175` (`as_of` over windows), `:378` (`windows_are_valid`), `:439` (windows in canonical bytes), `:719` (`held_at`); re-exported `fe-policy/src/lib.rs:22` |
| CLOCK-03 field_clock module | complete | `fe-database/src/field_clock/wire.rs:76` (`parse_own`), `merge.rs:41` (`winner`), `:52` (`upsert_own`), `store.rs:13-21` (write + verify SQL), `:165` (`author_frontier`), `migrate.rs:27` (`seed_legacy_floor`); no `unimplemented!`/`todo!` left in the module |
| CLOCK-04 persist windows, frontier on local revoke, held_at_write helpers | complete | `fe-database/src/role_grants.rs:171` (`proven_windows`), `:335` (windows column), `:456`/`:472` (`held_at_write`, `held_at_write_in`), `:484` (`load_scope_statements`), `:497-518` (`place_statement` proves wire windows), `:613-616` (frontier, 0 when no write held), `:639` |
| CLOCK-05 inbound clocked verse/fractal/petal rows | complete | dispatch `fe-database/src/handlers/replicated_row.rs:515`; membership floor `:776`, `:806`; `handlers/clocked_row.rs:66` (`apply_clocked_static_row`), `:100` (strictest reader, 96 KiB, HLC bound `:137`), `:242-385` (store all, judge per version, fenced single transaction); legacy rows fill only floor keys `replicated_row.rs:1562-1574` |
| CLOCK-06 node publisher stamps `_fe_clock` | complete | `fe-database/src/handlers/scene_publication/publisher.rs:186` (`encode_own`), `:202` (field-clock fence), `:225-303`; wire allow-list `scene_publication/wire.rs:102`, parse `:158`; `op_log::advance_hlc_past` `fe-database/src/op_log.rs:72` (consumed `handlers/petal_snapshot.rs:260`) |
| CLOCK-07 node inbound per-key merge, write-time-only re-judge | complete | `handlers/scene_publication/inbound.rs:211` (`apply_complete`), `:377` (`key_winner`, delete-wins), `:388` (`fan_out` for v1), `:520-581` (judge via `admits_at`), `:646-650` (N-4 tombstone dominance), `:142-153` (legacy candidates judged by `held_at_write`, R-CLOCK-1 R1) |
| CLOCK-08 outbound `publish_clocked_row`; create/rename/description/catalog | complete | `field_clock/publish.rs:32`, `:61`; verse/fractal create `handlers/crud.rs:54`, `:108`; petal create `fe-database/src/lib.rs:889`; rename `lib.rs:1173`; description `lib.rs:1225`; catalog `handlers/local_catalog.rs:108` |
| CLOCK-09 remaining emitters | complete | `handlers/verse_settings.rs:97`, `petal_terrain.rs:39`, `petal_render_distance.rs:99`, `visibility_groups.rs:231` (call sites `lib.rs:1842,1853,2105` pass `&local_did`), `elevated_query.rs:467` |
| CLOCK-10 DB-local re-judge on role change | complete | `field_clock/rejudge.rs:42` (every row, yields per 4096, skips without owner); triggers: statement placement `replicated_row.rs:340-364`, owner change `:224-238`, local statement `role_manager.rs:229` + echo `handlers/inbound_admissions.rs:233`; removed nodes echoed `handlers/mod.rs:70`, `lib.rs:1254,1281,1298,1321,2436` |
| CLOCK-11 fe-sync clocked rows over real transport | complete | `fe-sync/src/replicator.rs:2541-2613` (clocked rows, identical rewrite skipped, multi-author restore re-published); docs `fe-sync/src/AGENTS.md:2308`, `:2399` |
| CLOCK-12 simulator p2p-core legs | complete (code); run blocked-user | `tools/qa/sim/scenarios/p2p-core.json:176,198` (QA-0126 agree), `:407`/`:453` (QA-0165 deterministic keep), `:505-530` (14p per-key + rename); JSON parses |
| CLOCK-13 docs and findings close-out | partial | `fe-database/src/field_clock/AGENTS.md` (§wire..§compat), `fe-database/src/merge.rs:51` pointer, `fe-database/src/AGENTS.md:2223`; but `qa/findings.jsonl:117` (QA-0126) and `:156` (QA-0165) still `open`, `:147`/`:157` `fixed_source_live_pending` (routed to I-CONDUCTOR, not yet at HEAD) |

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| §3.1 `_fe_clock` shape and parse rejections (v, author, hlc, 512 keys, identity key, property key on non-node, value xor deleted) | complete | `field_clock/wire.rs:59-72` (`is_clock_key`), `:76-131` |
| §3.2 keys and identity columns | partial | node transform as one key `wire.rs:15-18`, `keys_of` `:146`; levels `field_clock/judge.rs:57-70`. Drift: `namespace_id` is insert-once identity in code (`wire.rs:45`, fill-once under a verse-Manager gate `replicated_row.rs:778-783`), spec §3.2 lists it as a Manager clock key |
| §3.3 local tables, unique index, same-transaction write + verify | complete | `schema.rs:830-833`; `store.rs:13-21`; `clocked_row.rs:356-385` |
| §3.4 merge order `(hlc, author, canonical bytes)` | complete | `merge.rs:8-37`; `merge::tests::fold_is_order_independent` `merge.rs:100` (includes inadmissible newer versions) |
| §3.5 admissibility (owner, legacy floor, `held_at`; no current-role AND) | complete | `judge.rs:28-53`; `role_grants.rs:472`; membership floor `replicated_row.rs:806-815` |
| §3.6 grant windows (cap, carry, append, frontier, read, validate, ordering) | complete | `fe-policy/src/role_grants.rs:89,126-169,175-200,378-386,439`; tests `:1415-1526` |
| §3.7 revoke frontier; inbound windows untrusted | complete | `fe-database/src/role_grants.rs:613-616`, `:171-186`, `:509-518`; grantor authority at time `fe-policy/src/role_grants.rs:673` (materialize) |
| §3.8 deferred interaction and re-judge triggers | complete | see CLOCK-10 row; legacy `RejudgeReplicatedNodes` path kept (`inbound.rs:110-142`) |
| §3.9 tombstones delete-wins, no resurrection | complete | `inbound.rs:377-386`, `:606-650`; journey `replicated_row.rs:4150` |
| §3.10 legacy and migration | complete | v1 fan-out `inbound.rs:388`; unclocked static fill `replicated_row.rs:1562-1574`; `legacy_node_row_provable` `:249`; seed at startup, fatal on failure `lib.rs:697-704`, batches of 1024 `migrate.rs:13` |
| §3.11 size bounds | complete | 96 KiB `clocked_row.rs:107`; 512 keys `wire.rs:9`; 8 windows `fe-policy/src/role_grants.rs:89`; HLC bound `clocked_row.rs:137` (`admit_scene_hlc`) |
| §4 security analysis, residual risks 1-7 | complete | spec.md §4; `field_clock/AGENTS.md:176`; SEC-INBOX 4 closed by R-CLOCK-1 |
| §5 compatibility, flag day | complete | `field_clock/AGENTS.md:171`, `fe-database/src/AGENTS.md:2228`, `fe-sync/src/AGENTS.md:2311` |
| §6.1 revoke keeps authorized past edits (sim 13a/13b) | blocked-user (run) | code: write-time-only judge `inbound.rs:574-581`; DB journey `fe-database/tests/role_propagation_local_test.rs:304-345`; sim legs `p2p-core.json:407,453` |
| §6.2 different property keys survive, any order | complete (test written, not run) | `replicated_row.rs:4085-4127` (R10 `handlers::replicated_row::`); sim `p2p-core.json:526-527` |
| §6.3 concurrent name writes converge to max `(hlc, author)` | complete (test written, not run) | `replicated_row.rs:2440-2483` (verse rows; petal rows share `apply_clocked_static_row`) |
| §6.4 multi-author restore over real transport | complete (test written, not run) | `fe-sync/src/replicator.rs:2598-2613` (R8) |
| §6.5 documented transform winner = code | complete | one `transform` key through `merge::winner`; `merge.rs:51` marks the edit_seq register legacy-only; sim `p2p-core.json:156,176` |
| §6.6 `fold_is_order_independent` | complete | `merge.rs:100-135` (R10 `field_clock::`) |

## Gate verdicts and hand-offs

- R-CLOCK-1 (PASS-WITH-FIXES): R8 HANDED-OFF to CLOCK-10 reached it (`rejudge.rs` has a body, no `unimplemented!`). SEC-INBOX 6 HANDED-OFF reached R-HEXON-2 (`hexon_unification_20260716/review-turn4/R-HEXON-2.md:36`, CONFIRMED-FIXED); the clock side stays clean: `handlers/petal_snapshot.rs` writes no `row_field_*` or `role_grant` rows. R6 and R7 are open user decisions recorded in `metadata.json` `open_user_decisions`.
- R-CLOCK-2 (PASS-WITH-FIXES): F5 HANDED-OFF (I-DB-2) reached it (`handlers/mod.rs:70`, `lib.rs:1254`); F6 HANDED-OFF (I-DB-2) reached it (`lib.rs:1842,1853,2105`). F8 DEFERRED stands (see below).
- SEC-INBOX: no entry names this critic's owned file. Entry 4: closed by R-CLOCK-1, re-checked here at `fe-database/src/role_grants.rs:171-186`, `:511`. Entry 6 (clock clause): closed as above.

## Plan §9 defaults vs code

| Question | Default | Code |
|---|---|---|
| q1/q6 compatibility | flag day | matches: no dual write, `_fe_clock` always stamped (`publisher.rs:186`, `publish.rs:32`) |
| q2 revoke frontier | highest write clock seen from the author | matches `role_grants.rs:613-616`; none seen = 0 (empty window) |
| q3 forged old clock | accepted, documented | matches spec §4 risk 1, `field_clock/AGENTS.md:176` |
| q4 window cap | 8 | matches `fe-policy/src/role_grants.rs:89`, oldest dropped `:165-167` |
| q5 storage | keep every author's latest per key, no GC | matches: no GC code in `field_clock/` |
| q7 transform | one key, max `(hlc, author)` | matches `wire.rs:15-18`, `merge.rs:51` |

## Excluded by the scoper (not gaps)

- blocked-user: run the P2P simulator `p2p-core` to verify the clock (RUNBOOK §3; only when the user names it).
- obsolete: hierarchy deletes (`DeleteEntity` for verse/fractal/petal) replicating as tombstones.
- obsolete: per-group clocks inside `petal.visibility_groups` and per-key clocks inside terrain JSON (one key per column).

## HANDED-OFF (owner = next round)

- HANDED-OFF (I-CONDUCTOR): `qa/findings.jsonl` QA-0165 and QA-0126 to `fixed_source_live_pending` citing this track; keep QA-0156/QA-0166 pending the sim run.
- HANDED-OFF (next CLOCK docs slice): spec.md §3.2 lists `namespace_id` as a Manager clock key; code and `field_clock/AGENTS.md:61` make it insert-once identity filled once by a verse Manager. Amend the spec table; spec frontmatter `status: planned` is stale too.
- HANDED-OFF (next CLOCK fe-policy slice): `fe-policy/src/role_grants.rs:704` doc on `materialize_at` still says "Callers AND it with the current view"; clocked rows use `held_at` alone. One-line doc fix.
- HANDED-OFF (next CLOCK test slice): no DB journey proves a NON-empty frontier (revoker holds a write of the author: window `[since, frontier)` admits that write, refuses one stamped above it). Extend `role_propagation_local_test.rs:304` (R7), no new test function.
- HANDED-OFF (T-CLOCK next round): `metadata.json` `handed_off` still lists R-CLOCK-2 F5/F6 as open; both landed in `ebcef708`.
- DEFERRED (R-CLOCK-2 F8; trigger: first report of an unclocked settings row): published-bytes assertions for render distance and the verse manifest.
- DEFERRED (trigger: a QA report of an imported node losing a key to a legacy row): nodes restored by snapshot import get no legacy-floor versions until the next startup seed (`migrate.rs:27` runs only at `lib.rs:697`).

Verdict: COMPLETE-WITH-GAPS: 12 of 13 plan tasks complete at HEAD (CLOCK-13 partial on the QA ledger, routed to I-CONDUCTOR); spec complete except a §3.2 doc drift on `namespace_id`; acceptance runs are user-gated.
