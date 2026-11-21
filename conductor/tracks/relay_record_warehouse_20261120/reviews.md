---
type: review-log
title: Relay record warehouse — review log
tags: [relay_record_warehouse_20261120]
---

# Review log

## 2026-11-20 — wave 0 (S1, S2, S3) — advocate

**Verdict:** PASS WITH FIXES — all three briefs are fully implemented and
cite their decisions (D-CL36..D-CL40, D-CL43, D-CL45, D-CL47, D-CL48);
by-construction rules (required floor evidence, single-verse refusal at
start, all-or-nothing set admission) are stated normatively; five findings
fixed in place, none left open.

- **S1, SPEC-7 §4.2 rule 6 and test 14:** a retry of a `held` operation could
  only become `committed`, contradicting track AC-7.1. Fix: both now admit
  `committed`, `already_committed` and `accepted_pending_materialization`;
  test 14 also asserts `commit_ack` keys 5..8 are absent (AC-7.2).
- **S1, SPEC-7 §6 rule 3:** the `protocol_error` table listed the `held` row
  while saying every row is a `protocol_error` category. Fix: rule 3 exempts
  rows answered by an ack state; the erratum records the provisional state
  value `4` and points to the single `CDL-PROV` register.
- **S1/S3, SPEC-4 §5:** SPEC-7 mapped `authorization_evidence_unavailable` to
  `held` as a SPEC-4 disposition, but SPEC-4 never defined it. Fix: SPEC-4 §5
  gains the category (retryable quarantine, distinct from
  `unauthorized_operation`), erratum RRW-3a.
- **S3, SPEC-4 §10.4 rule 4:** an admitted-but-unclosed set member was
  described as "pending, like `missing_parent`", which is a quarantine outside
  the verified log. Fix: reworded to an admitted member with its projection
  pending (apply marker held, as for `referenced_artifact_unavailable`); test
  15 now names every AC-4.5 multi-member kind as a table row.
- **S2/S3, bulk-artifact prune gap:** glTF bytes (in the record since D-CL47)
  are in no custody set, and SPEC-5 rule 6 only gated "sealed segments", so a
  peer could release the last copy of a GLB behind a checkpoint. Fix: SPEC-5
  §5.2 rule 6.5 and SPEC-9 relay §4.1 rule 4 refuse bulk-artifact release
  (costs disk, never data); SPEC-9 open item 3 now asks the owner whether bulk
  artifacts get their own receipts. Also named R by its column,
  `ts_replication_factor`.
- **Tests:** wave 0 is spec text; the named conformance cases are acceptance
  requirements for later slices, which must land them as rows in the `RW`
  batch's journey files. Nothing to run here.

## 2026-11-20 — wave 0 (S1, S2, S3) — critic

**Verdict:** PASS WITH FIXES — S1-S3 match D-CL35..D-CL49 with no reading
that needs a new owner decision; seven defects refuted the advocate pass
(two silent allows, one internal contradiction, three inconsistencies, one
false "already recorded" claim), all fixed in owned files, none open.

- **S3, SPEC-5 §5.2 rule 6 (silent allow):** "floor evidence for every sealed
  segment the release covers" was vacuously true for a release that covers no
  segment (snapshot components). Fix: rule 6.5 now requires each released
  artifact to be in the custody set of a floor-met manifest; an artifact in no
  custody set is refused; test 19 gained that row.
- **S2, SPEC-9 §4 rule 1 (silent allow):** "R is at least 1" asserted a fact
  instead of handling a missing/zero column, so an implementation could
  default to 1. Fix: a missing, zero or unreadable R makes the floor unknown,
  never met (matches SPEC-5 rule 6.2).
- **S3, SPEC-4 §10.4 (contradiction):** rule 1 (one durable append for the
  whole set) contradicted rules 4-5 (a receiver holds a member whose closing
  member has not arrived). Fix: rules 1-2 scoped to the authoring peer;
  receivers admit per member and get atomicity from the projection gate
  (RRW-3b).
- **S3, SPEC-8 §3 rule 5 (D-CL43 gap):** seeding admitted members was gated on
  nothing per verse, so every dual-emit build seeded by default, and nothing
  separated the replicated per-verse setting from the local migration mode
  (§2 rule 2 forbids replicated-row activation). Fix: seeding requires
  `warehouse_mode` on and a non-empty `record_relays`; that setting never
  changes the local mode (RRW-1a).
- **S3, SPEC-5 rule 6.4 vs SPEC-9 §4.1 rule 1:** SPEC-9 requires
  `peer_prune_after_floor` on; SPEC-5 did not name it. Fix: rule 6.4 names it.
- **S2, SPEC-9 §5 rule 5:** named the `record_relays` list (a legacy-replicated
  verse column) as the sole control against non-record relays signing
  receipts. Fix: Q6 and Q7 together; Q7's Manager+-issued `seed` chain
  (D-CL46) is the signed half and is never skipped.
- **S1, SPEC-7 test 14 and erratum:** the advocate entry claims test 14 admits
  `already_committed`, but the text did not; it also asserted only keys 5..8
  absent although key 4 (category) is `rejected`-only in the register. Fix:
  test 14 names `already_committed` and keys `4`..`8`. The erratum said state
  `4` is "recorded" in the `CDL-PROV` register, which still lists `0`..`3`;
  now says it is to be recorded by slice I8 (owner of
  `fe-canonical-log/src/AGENTS.md`). SPEC-9's header likewise names I8.
- **S3, SPEC-4 §5:** `authorization_evidence_unavailable` overlapped
  `opaque_payload` on "payload scope key not held". Fix: a missing key is this
  category (as code's `PayloadKeyUnavailable` already is), a missing artifact
  is `opaque_payload`.
- **Enforcement census:** this wave added no `assert_`/`validate_`/`verify_`/
  `check_`/`require_`/`may_`/`decide_` functions (spec text only):
  PRODUCTION_CALLER 0, TEST_ONLY 0, NO_CALLER 0. Normative checks that later
  slices MUST give a production caller (integration wave to confirm by name):
  custody-receipt verification (SPEC-9 §3.2), floor assessment Q1-Q7
  (SPEC-9 §4), the required floor-evidence input to
  `checkpoint::compaction_decision` (SPEC-5 §5.2 rule 6.3), record-relay
  single-verse start refusal (SPEC-9 §5 rule 1), format-epoch wipe
  (SPEC-9 §6 rule 2), the `held` ack mapping (SPEC-7 §6), and the
  author-side all-or-nothing set append (SPEC-4 §10.4 rule 1).
- **Over-engineering:** none removed. The custody-set binding (Q4) and signed
  format epoch each close a specific prune-data-loss path; open item 1
  (prune before `CDL-PROV` ratification) remains correctly listed for the
  owner, not assumed.
