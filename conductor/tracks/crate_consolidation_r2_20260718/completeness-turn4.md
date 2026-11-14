---
type: Review
track: crate_consolidation_r2_20260718
critic: C-REL
round: 4
date: 2026-11-13
---

# C-REL completeness: crate_consolidation_r2_20260718

Checked against the working tree on 2026-11-13 (HEAD 733a3256; no file cited below is modified in the
tree). No cargo. Gate outcomes are the §9 q40 defaults (DEFER all three); REL-11 and REL-12 are NOT PLANNED
(turn4-plan §6.3) and are reported under that class, not as gaps.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 G-1 fe-plugin-test into fe-plugin | complete (DEFER recorded) | `conductor/decisions/crate-consolidation-r2-gates-20261113.md:19-33`. Re-grepped consumers: `fe-plugin/Cargo.toml:63` (dev-dep), `tools/fe-ext/Cargo.toml:23`, `extensions/iot-bridge/Cargo.toml:19`, matching `evidence/consumer-map.md` FR-1 table. |
| FR-2 G-2 fe-hexon-registry into fe-hexon | complete (DEFER recorded) | Decision record `:35-49`. Re-grepped: dev-dep of `fe-hexon/Cargo.toml:45` and `fractalengine/Cargo.toml:32`; `docker/Dockerfile.hexon-registry` builds it alone. Matches the evidence FR-2 table. Gate condition (measured image cost) not measured: `cargo tree` closure sizes deferred, stated in the evidence file. |
| FR-2 format-merge gate routed to hexon_unification | complete | `conductor/tracks/hexon_unification_20260716/spec.md:456` and `plan.md:48` (applied by T-HEXON, b9951457); the rule "whichever track executes second inherits the other's landed shape" is visible from both tracks. |
| FR-3 G-3 fe-query into fe-api, evidence first | complete (DEFER recorded) | Decision record `:51-65`; `evidence/consumer-map.md` FR-3. Re-grepped edges `fe-api/Cargo.toml:18`, `fe-database/Cargo.toml:15`; `fe_query` appears 19 times in 16 files under `fe-database/src` (e.g. `api_token_store.rs`, `rbac.rs`, `role_manager.rs`, `handlers/scoped_query.rs`), more than the 7 sites the evidence carries from the scoper, which only strengthens the layering-inversion finding. |
| FR-4 bookkeeping per ACCEPTED merge | not planned (no merge accepted) | Members unchanged: `Cargo.toml:2-33` still lists `fe-plugin-test`, `fe-hexon-registry`, `fe-query`; README count "26 workspace members" (`README.md:339`) stays correct. One register entry per gate exists (decision record); RUNBOOK mirrors G-1/G-2 at `conductor/RUNBOOK.md:420`. |
| Acceptance: every gate has a recorded outcome | complete | Decision record `status: DEFER (default outcomes, pending the user's answer)` (`:5`). |
| Acceptance: FR-3 consumer map as an artifact | complete | `evidence/consumer-map.md` (OKF `type: Evidence`). |
| Acceptance: hexon_unification cross-reference | complete | see FR-2 row. |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| P1 FR-3 consumer map | complete | `evidence/consumer-map.md` FR-3 |
| P1 FR-1/FR-2 dep snapshot | partial | Direct deps listed; closure sizes not measured (no cargo this round), as the evidence file states. |
| P1 cross-reference task | complete, checkbox still `[ ]` | `hexon_unification_20260716/spec.md:456`, `plan.md:48` |
| P1 verification checkpoint | complete except closure sizes, checkbox `[ ]` | |
| P2 G-1 / G-2 / G-3 decided | complete | decision record `:19-65` |
| P2 verification "three register entries, user-visible" | complete, checkbox `[ ]` | decision record; §9 q40 puts the question to the user |
| P3 execute G-1 / G-2 / G-3 | not planned (DEFER) | REL-11 / REL-12 NOT PLANNED; G-3 obsolete per scoper |
| P3 FR-4 bookkeeping per merge | not planned (DEFER) | |
| P4 workspace sweep / docker build / retro-archive | blocked-user (scoper) | |
| P4 update oss_release checklist + tracks.md with final crate count | partial | Crate count unchanged (26) and README is correct; `conductor/tracks.md:392-398` still reads "Nothing executed", "0 / 15" and "fe-plugin-test has zero consumers", which is false (three consumers, above). |

## Round-4 items

- REL-10 (evidence + gate outcomes): complete; 6763b192.
- REL-11 (G-1 execution), REL-12 (G-2 execution): NOT PLANNED, default DEFER (§9 q40). Code confirms
  neither ran: both crates are still members and no `test-utils` or `registry-service` feature exists.
- §9 q40 default (DEFER all three, run neither REL-11 nor REL-12): matches the code above.
- No R- gate covered REL-10 (docs only). SEC-INBOX has no entry naming this track or its evidence files.

## Excluded by the scoper

- obsolete: G-3 execution (fe-query into fe-api); fe-database is a consumer, so the merge inverts layering.
- blocked-user: Phase 4 close-out (workspace sweep, docker image build, retro/archive).

## HANDED-OFF (owner = next round)

- HANDED-OFF (REL bookkeeping, next round): tick Phase 1 cross-reference task and the Phase 2 verification
  checkpoint in `plan.md`, citing `hexon_unification_20260716/spec.md:456`, `plan.md:48` and the decision record.
- HANDED-OFF (REL bookkeeping, next round): refresh `evidence/consumer-map.md` FR-3 use sites to the
  re-grepped 16 files / 19 occurrences in `fe-database/src` (currently 7 carried from the scoper).
- HANDED-OFF (I-CONDUCTOR, next round): `conductor/tracks.md:392-398` board entry: gates recorded DEFER,
  evidence artifact landed, fe-plugin-test has three consumers.
- HANDED-OFF (user-gated, whenever G-1/G-2 is reopened): run `cargo tree -p fe-plugin-test` and
  `cargo tree -p fe-hexon-registry` to size the closures the FR-1/FR-2 gates ask for.

Verdict: COMPLETE FOR THIS ROUND - all three gates carry a recorded DEFER with evidence; only bookkeeping ticks, a board refresh and the user-gated closure measurement remain.
