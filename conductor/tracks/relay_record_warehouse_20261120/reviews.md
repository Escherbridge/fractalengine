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

## 2026-11-20 — wave 1 (P1, P2, P3, T1, Q1, gate G1) — advocate

**Verdict:** PASS WITH FIXES — every brief is implemented with no stub (held
state, custody receipt + floor + required compaction parameter,
`leaf_certificate_id`, warehouse settings, columnar scan provider), checks are
unavoidable by construction (required `&DurabilityFloorEvidence`, receipts
only from `issue_custody_receipt`, `#[non_exhaustive]` verified receipt,
evidence obtainable only from `assess`); five findings fixed, none open.
`cargo check -p fe-canonical-log --all-targets`, `-p fe-runtime --lib --tests`,
`-p fe-query --features datafusion --tests` green.

- **P1, `held` ack leaked a category (SPEC-7 §4.2 rule 5, test 14):**
  `CommitAckState::Held { category }` encoded key 4, which the critic-amended
  spec forbids for `held` (category is `rejected`-only). Fix:
  `CommitAckState::Held` is a unit state (keys 0..3 only, decode requires
  exactly four keys); the reason stays in-process on
  `PipelineResult::Held(HeldReason)`; the now-unused provisional
  `HeldReason::to_u64/from_u64` and its test were removed so no unregistered
  wire number exists. The plan brief's `Held { category }` shape is superseded
  by the spec.
- **P1, test 14 incomplete:** renamed to the spec's
  `held_dispositions_answer_held_and_retry_identical_bytes`, asserts state `4`,
  keys `4`..`8` absent, a retry while the parent is missing stays `held`, and
  exactly one append (the retry after the parent commits, the next identical
  submit is `already_committed` at the same binding).
- **P2, floor evidence not bound to the checkpoint's verse (by construction):**
  a met floor assessed for verse B could release verse A's history.
  Fix: `CheckpointVerification::Verified` now carries the claim's `verse_id`
  (the variant is `#[non_exhaustive]`, so no caller can forge it), and
  `compaction_decision` refuses `FloorEvidenceForAnotherVerse` before the floor
  shortfall; `checkpoint::tests` gained the row.
- **P3, brief named `wire::session` tests, only `compose::wire` was extended:**
  `authorized_body_round_trips` now asserts the `authorized` body stays exactly
  keys 0..6 (the new field is in-process only).
- **T1, DID check admitted non-base58 characters:** `is_valid_relay_did`
  refused `0`/`O`/`I`/`l` too; table row added. Column constants gained their
  own one-line docs.
- **Q1:** no defect found — validates every hexon through
  `read_observation_hexon`, prunes only on proven min/max ranges (NaN, null,
  mistyped literals and `OR` never prune, pushdown reported `Inexact`), no I/O,
  no keys. Its test is under `datafusion`, so plain `cargo test -p fe-query`
  does not reach it; the planned `RW` line (`--features datafusion --lib --
  columnar::observation::scan`) does — I8 must land that line.
- **Handoffs (outside this wave's files, not fixed here):**
  `fe-api/src/canonical_ws/{handler,state}.rs` test fixtures build
  `VerifiedAuthorization` without `leaf_certificate_id` and will not compile
  under `cargo check -p fe-api --tests` until A1 adds the field (handler.rs:301
  note is also stale). G1 also touched `retention/tombstone.rs` and
  `wire/test_support.rs` (compile follow-ons of the required parameter/field)
  and T1 added `warehouse::` to an existing `conductor/test-batches.md` line
  outside its owns list; both are benign, recorded for I8. Enforcement census:
  `compaction_decision`, `assess` and `issue_custody_receipt` have no
  production caller yet — D5 (custody ledger, checkpoint issuer, prune) owns
  them and the integration wave must confirm them by name.

## 2026-11-20 — wave 1 (P1, P2, P3, T1, Q1, gate G1) — critic

**Verdict:** PASS WITH FIXES — the advocate's held state, custody receipt,
floor and `leaf_certificate_id` hold up against SPEC-5 §5.2 rule 6, SPEC-7
§4.2 and SPEC-9 §2-§4; four defects refuted it (one permit wider than its
evidence, one silent fall-through, one unvalidated deserialize door, one
mis-reported floor state), all fixed in owned files, none open.
`cargo check -p fe-canonical-log --all-targets`, `-p fe-runtime --lib --tests`,
`-p fe-query --features datafusion --tests` green, no warnings in owned files.

- **P2, permit not bound to the assessed release (SPEC-5 §5.2 rule 6.5,
  SPEC-9 §4.1 rule 1):** `compaction_decision` returned a bare
  `Permitted { checkpoint_id }`, so floor evidence assessed for one covered
  artifact let the D5 prune drop anything behind the checkpoint. Fix:
  `DurabilityFloorEvidence` keeps the release (ascending, deduplicated,
  `released_artifacts()`), and `CompactionDecision::Permitted` now carries
  `released_artifacts`, the exact set the floor covered; the prune can only
  drop what the permit names. `checkpoint::tests` and
  `cross_spec_integration.rs` assert the permit's set.
- **P2, `compaction_decision` tombstone loop was `if let Err(MissingSuppressionProof)`:**
  any future `TombstoneError` variant would have fallen through to
  `Permitted` (silent allow). Fix: exhaustive `match` with no wildcard, so a
  new variant fails to compile until it refuses.
- **P2, floor state for an unhashable custody set:** a `Computed` set whose
  `custody_set_id` failed was reported `BelowFloor` with 0 relays while its
  receipts said `CustodySetUnknown`. Fix: such a set is treated as unknown
  at intake, so the status read reports `Unknown(CustodySetUnavailable)`
  (SPEC-9 §4 rule 5). Still never met either way.
- **T1, `Deserialize` bypassed `sanitized`:** a serde-decoded
  `VerseWarehouseSettings` (the shape a settings command or API body would
  carry) admitted a non-`did:key` or duplicated relay. Fix: the struct
  deserializes through `UncheckedVerseWarehouseSettings` via
  `#[serde(try_from)]`, and `sanitized` and the serde path share one
  `check_record_relays`; the `warehouse::` test gained a refusal row.
- **Refuted, no change:** P1 maps exactly the five SPEC-4 quarantine
  dispositions to `held` with keys 0..3 only; P3 keeps `authorized` at keys
  0..6; Q1 prunes only on proven min/max over all-primitive signed columns,
  and NaN, mistyped or `OR` predicates never prune. No over-engineering
  removed: `ManifestCustody::Unavailable`, `RejectedReceipt` and the
  per-manifest states are what D5's status read (P-3) needs.
- **Enforcement census** (`assert_`/`validate_`/`verify_`/`check_`/
  `require_`/`may_`/`decide_` added or re-signed this wave):
  - `segment::custody_receipt::verify_custody_receipt`: PRODUCTION_CALLER in
    library code (`DurabilityFloorEvidence::assess`), but `assess` has no
    runtime caller, so it is unreachable at runtime today.
  - `RecordRelayBinding::requires_reset`: PRODUCTION_CALLER in library code
    (`issue_custody_receipt`), which is itself TEST_ONLY.
  - `retention::tombstone::may_compact_tombstone` (gained `floor`): TEST_ONLY.
  - `fe_runtime::warehouse::check_record_relays` (added by this review):
    PRODUCTION_CALLER (`sanitized`, serde `try_from`); the settings type has
    no runtime consumer yet.
  - P1, P3 and Q1 added no function with these prefixes.
  - NO_CALLER (runtime), for the integration wave to confirm by name:
    `checkpoint::compaction_decision` and
    `retention::durability_floor::DurabilityFloorEvidence::assess` (D5 prune
    and status read), `segment::custody_receipt::issue_custody_receipt` and
    `CustodySet::decode_canonical` (the record-relay ledger and seed path),
    `fe_query::columnar::observation::ObservationScanTable` (the query
    surface that registers it), `fe_runtime::warehouse::VerseWarehouseSettings`
    (the `SetVerseWarehouseSettings`/`GetVerseWarehouseSettings` handlers), and
    `wire::commit::PipelineResult::Held` through `VerifyingCommitPipeline`
    (CANON-WS mount).
- **Handoff:** D5's prune must release `Permitted.released_artifacts` and
  nothing else. `retention/AGENTS.md` §tombstone and the crate register
  (owned by I8) should mention the floor parameter and the permit's set.

## 2026-11-20 — wave 1 (P1, P2) — security

**Verdict:** PASS WITH FIXES — one custody-provenance hole (a peer's seed
commitment could stand in for the assessor's own walk) fixed in owned files;
no authentication, key-custody, oracle or redaction defect found in P1/P2.
`cargo check -p fe-canonical-log --all-targets`, `-p fe-runtime`, `-p fe-query`
green.

- **P2, floor accepted a peer-supplied custody set (SPEC-9 §2 rule 2, §4.1
  rule 4):** `CustodySet::decode_canonical` (the relay's seed-commitment
  decoder) and `for_manifest` produced the same type, so a decoded seed
  commitment could be passed as `ManifestCustody::Computed`. A malicious
  seeder plus colluding listed relays could then list artifacts the manifest
  never indexes (a bulk artifact, another manifest's nodes), receipt them, and
  have the floor permit releasing them. Fix: `CustodySet` carries a private
  origin (`Walked` from `for_manifest`, `SeedCommitment` from
  `decode_canonical`, exposed as `is_walked()`); `DurabilityFloorEvidence::assess`
  treats a non-walked set as unknown (`CustodySetUnavailable`), so it never
  counts. Relays still receipt decoded sets. `cross_spec_integration.rs`
  asserts the decoded set keeps the same members and id and that a laundered
  set yields `Unknown`, not met. Residual: `for_manifest` still trusts its
  caller to pass a walked list; D5's manifest walker should become its only
  production caller.
- **Refuted, no change:** P1's `held` ack carries keys 0..3 only, so it
  reveals less than the old per-category rejects; whether a parent or chain
  exists was already visible through `missing_parent` and is spec-mandated
  (SPEC-7 §4.2 rule 6, D-CL40). Nothing is retained on `held`, so there is no
  quarantine budget to exhaust. Receipt verification is canonical
  re-encode, then canonical DID-to-key binding, then `verify_domain` over the
  domain-separated preimage; distinct relays are counted by public key; Q5-Q7
  are evaluated fresh. No relay path touches a scope key (issuance re-hashes
  ciphertext only), and no error or Debug output carries key material.
- **Handoffs (outside this wave's files):** (1) `compaction_decision` binds
  the floor to the checkpoint's verse but not to the checkpoint's coverage.
  D5 must intersect `Permitted.released_artifacts` with what SPEC-5 permits
  behind that checkpoint. (2) `DurabilityFloorEvidence` is not `Clone`, but it
  is passed by reference, so one assessment can be reused. Taking it by value
  in `compaction_decision`/`may_compact_tombstone` (tombstone.rs is not owned
  here) would make each assessment single-use, as §4.1 rule 1 ("at the
  moment of release") intends.
