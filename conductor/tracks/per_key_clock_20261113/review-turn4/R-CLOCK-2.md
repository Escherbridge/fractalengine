---
type: Review
---

# R-CLOCK-2 — adversarial review and security review of CLOCK-09..12

SEC-INBOX: no untriaged entry names a file this gate owns. Entry 4 (closed by R-CLOCK-1) was re-checked against CLOCK-10: `rematerialize` and the node re-judge judge only through `VerseView::admits_at` -> `held_at_write_in` over `load_admitted`, so only locally proven windows can keep a version. Entry 6: CLOCK-10 adds no import path; nothing to add.

F1 FIXED - fe-database/src/field_clock/rejudge.rs:53 - `rows.iter().take(4096)` over a SORTED row list skipped the same tail on every call, so after a late revoke the winners of rows past 4096 were never demoted (fail-open); every row is now re-judged, yielding to the runtime every 4096 rows.
F2 FIXED - fe-database/src/field_clock/rejudge.rs:58 - with the verse owner unknown, `VerseView::settled` refuses every non-floor version, so a re-judge would unset every non-required key of the verse's rows; the re-judge now skips (warned) until the manifest names an owner.
F3 FIXED - fe-database/src/field_clock/rejudge.rs:131 - the local outbox dropped one entry per push even when still over the cap; it now trims oldest entries until under the cap, always keeping the newest.
F4 FIXED - fe-database/src/handlers/inbound_admissions.rs:122 - `removed_nodes` grew without bound because nothing takes it; it is now capped at 4096 with one warn per trim (no per-row log).
F5 HANDED-OFF (I-DB-2) - fe-database/src/lib.rs:2414,1240,1266,1282,1304 - `InboundAdmissions::take_removed_nodes` has no caller, so a node a role-change re-judge removes stays on screen until a hierarchy reload; drain it after every `echo_applied` of `admissions.apply` / `retry_author_roles` and send `DbResult::NodeDeleted { node_id, petal_id }` per entry.
F6 HANDED-OFF (I-DB-2) - fe-database/src/lib.rs:1825,1836,2088 - CLOCK-09 added `local_did` to `visibility_groups::publish_registry`, but the three call sites still pass four arguments; insert `&local_did` before the petal id. `cargo check -p fe-database` fails until this lands.
F7 FIXED - fe-database/src/handlers/petal_terrain.rs:374 - CLOCK-09 extended only the visibility-groups journey; the terrain journey now asserts the published bytes carry `_fe_clock` signed by the local author with the terrain key's own version (batch R4 `handlers::petal_terrain`).
F8 DEFERRED (T-CLOCK sweep, or the first report of an unclocked settings row) - fe-database/src/handlers/petal_render_distance.rs:99, verse_settings.rs:96 - no published-bytes assertion for render distance or the verse manifest; both call the same `publish_clocked_row` seam F7 now proves.
F9 KEPT (API tokens are minted with `sub` = the node DID, fe-database/src/handlers/api_token.rs:126) - fe-database/src/handlers/elevated_query.rs:467 - the elevated publish clocks with `request.actor_did`, not the DB thread's `local_did`; a mismatch would make every peer deny the row (clock author != entry author). Revisit if tokens ever carry a delegate subject.
F10 KEPT (spec 3.2 decision) - fe-database/src/field_clock/judge.rs:57 - petal/fractal `name` is admitted inbound for an Editor while the local rename door needs Manager (entity.rs prepare_parent_mutation); same as the pre-clock static-row gate.
F11 KEPT (authority-gated) - fe-database/src/handlers/replicated_row.rs:339 - every admitted role statement triggers an O(rows of the peer) re-judge, so a Manager churning grant/revoke costs every peer work; only Managers can admit statements. Revisit on a perf report.
F12 KEPT (echo-only, tests assert with contains) - fe-database/src/field_clock/rejudge.rs:107 - `LOCAL_OUTBOX` is process-global keyed by verse id, so parallel tests sharing a verse id can take each other's echoes; the DB rows themselves are already correct.
F13 FIXED - tools/qa/sim/scenarios/p2p-core.json:417,450 - the N2 `agree` checks claimed to prove QA-0165 "keep", but the step-6 winner is unpredictable and a consistent revert also agrees; the covers now say agreement only, and a deterministic leg was added: editor2 writes `pre_revoke2` in 13a (:407) and 13b asserts it stays on every peer for 40 s after its revoke (:453).
F14 FIXED - tools/qa/sim/scenarios/p2p-core.json:529 - the 14p rename leg is single-author (only a Manager may rename a petal), so it cannot reproduce QA-0166's two-author restore; covers now says so and points at the R8 replicator journey that does.
F15 KEPT (verified by reading) - fe-sync/src/replicator.rs:2541 - CLOCK-11's hand-built `_fe_clock` matches `parse_own` (v 1, author = entry author, decimal-string hlc, exactly one of value/deleted); no fe-sync parser uses `deny_unknown_fields` on hierarchy rows (only scene_asset_gate's classification).
F16 KEPT (checked) - fe-database/src/role_manager.rs:219 - local-statement re-judge warns and never fails the committed statement; it runs before `publish_statement`, and every lib.rs arm that issues local statements calls `retry_author_roles`, which takes the outbox (the v1 invite path echoes on the next role command).

Verdict: PASS-WITH-FIXES (F5 and F6 are I-DB-2's; F6 blocks `cargo check -p fe-database` until applied).
