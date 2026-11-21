---
type: track-plan
title: Relay record warehouse — implementation plan
tags: [relay_record_warehouse_20261120]
timestamp: 2026-11-20T00:00:00Z
status: planned
resource: ./spec.md
---

# Implementation Plan: Relay record warehouse

## Overview

The plan has 39 slices in six waves. Each slice is sized for one agent. Waves 0-4
are parallel leaves, and each closes with one serial gate slice. Wave 5 is the
serial integration wave. Partitions in `metadata.json` were computed at HEAD
`92946362`. Re-verify every `owns` list with a real grep if HEAD has moved.

House rules for every slice:
- **Parallel leaves never build.** Only the wave's gate slice runs `cargo check`,
  and it runs it serially (one builder, RUNBOOK §5 doctrine:
  `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 -j4`).
- **No tests run inside slices.** Tests are written or extended and then run only
  when a steer names the batch. The batch is `RW` (defined below) plus the
  existing region batches named per slice.
- **Extend, don't add.** Extend an existing journey test file or test module
  first. A new module gets at most one table-driven journey test, reachable by the
  `RW` filter.
- **No clippy.** The gate is `cargo check -p <crate>`.
- **Unavoidable by construction.** Every new check is a required parameter, a
  `pub(crate)` raw path, or a `#[non_exhaustive]` result. Never an `Option` the
  caller can pass `None` to. This codebase's recurring defect is "built, never
  wired".
- **`fe-canonical-log` stays pure.** No dependency on fe-runtime, fe-identity,
  fe-policy, fe-database, or surrealdb. `lib.rs` and `Cargo.toml` stay untouched,
  and new files go under existing modules.
- **No dependency edits.** Root `Cargo.toml` and `Cargo.lock` have unrelated
  uncommitted edits. A slice that finds it needs a new crate dependency stops and
  reports instead of editing either file.
- **Dirty-tree check: no `owns` entry overlaps the uncommitted files**
  (Cargo.toml, Cargo.lock, fe-api extensions.rs/mcp/*, fe-plugin bus/tick,
  fe-ui native_accessibility/surface.rs, fractalengine plugin_bridge
  bindings/catalog, tools/fe-cli). Slices I4 and I5 sit next to dirty files
  (`fe-api/src/mcp/*`, `fractalengine/src/plugin_bridge/*`) and must not touch
  them.

Tier key: haiku = docs and bookkeeping; sonnet = standard code; opus = protocol,
crypto, authorization, and inbound admission.

## Slice table

| ID | Wave | Tier | Owns | Depends on |
| --- | --- | --- | --- | --- |
| S1 | 0 | sonnet | `docs/spec/canonical-log/commit-preview-wire.md` | — |
| S2 | 0 | opus | `docs/spec/canonical-log/segment-shard-relay.md`, `docs/spec/canonical-log/relay-custody-durability.md` (new) | — |
| S3 | 0 | sonnet | `docs/spec/canonical-log/migration-plan.md`, `docs/spec/canonical-log/branches-checkpoints-retention.md` | — |
| P1 | 1 | sonnet | `fe-canonical-log/src/wire/commit.rs`, `fe-canonical-log/src/wire/error.rs`, `fe-canonical-log/src/compose/storage.rs`, `fe-canonical-log/tests/spec7_conformance.rs` | S1 |
| P2 | 1 | opus | `fe-canonical-log/src/segment/custody_receipt.rs` (new), `fe-canonical-log/src/segment/mod.rs`, `fe-canonical-log/src/retention/durability_floor.rs` (new), `fe-canonical-log/src/retention/mod.rs`, `fe-canonical-log/src/checkpoint.rs`, `fe-canonical-log/tests/cross_spec_integration.rs` | S2, S3 |
| P3 | 1 | sonnet | `fe-canonical-log/src/wire/session.rs`, `fe-canonical-log/src/compose/wire.rs` | — |
| T1 | 1 | sonnet | `fe-runtime/src/warehouse.rs` (new) | S2 |
| Q1 | 1 | sonnet | `fe-query/src/columnar/observation/scan.rs` (new), `fe-query/src/columnar/observation/mod.rs` | — |
| G1 | 1-gate | sonnet | `fe-runtime/src/lib.rs` (mod line only) | P1, P2, P3, T1, Q1 |
| D1 | 2 | opus | `fe-database/src/canon_log/local_author.rs` (new), `fe-database/src/canon_log/issuance.rs` (new) | G1 |
| D2 | 2 | sonnet | `fe-database/src/canon_log/segment_producer.rs` (new) | G1 |
| D3 | 2 | opus | `fe-database/src/canon_log/proof_environment.rs` (new), `fe-database/src/canon_log/segment_ingest.rs` (new), `fe-database/tests/canon_log_composed_admission_test.rs` | G1 |
| D4 | 2 | sonnet | `fe-database/src/canon_log/wire_registry.rs` (new) | G1 |
| D5 | 2 | opus | `fe-database/src/canon_log/custody_ledger.rs` (new), `fe-database/src/canon_log/checkpoint_issuer.rs` (new), `fe-database/src/canon_log/prune.rs` (new) | G1 |
| D6 | 2 | opus | `fe-database/src/canon_log/wrap_delivery.rs` (new), `fe-database/src/canon_log/device_enrolment.rs`, `fe-database/src/handlers/replicated_row.rs`, `fe-database/tests/device_enrolment_test.rs` | G1 |
| D11 | 2 | sonnet | `fe-database/src/handlers/warehouse_settings.rs` (new) | G1 |
| D12 | 2 | sonnet | `fe-database/src/handlers/ts_partial.rs`, `fe-database/src/ts_partial_reader.rs` (new), `fe-database/src/handlers/inbound_admissions.rs` | G1 |
| D13 | 2 | sonnet | `fe-database/src/canon_log/observation/local_bi.rs` (new), `fe-database/src/canon_log/observation/mod.rs` | G1 |
| G2 | 2-gate | sonnet | `fe-database/src/schema.rs`, `fe-database/src/canon_log/mod.rs`, `fe-database/src/handlers/mod.rs`, `fe-database/src/lib.rs` (mod lines only) | D1-D6, D11-D13 |
| D7 | 3 | opus | `fe-database/src/migration/scene_write_tap.rs` (new), `fe-database/src/migration/mappings/mod.rs` (new), `fe-database/src/migration/boundary.rs`, `fe-database/src/migration/mode.rs`, `fe-database/src/migration/mapping_inventory.rs`, `fe-database/tests/migration_conformance_test.rs` | G2 |
| N1 | 3 | opus | `fe-sync/src/canonical_segments.rs` (new) | G2 |
| N2 | 3 | sonnet | `fe-sync/src/gossip_auth.rs`, `fe-sync/src/relay_config.rs`, `fe-sync/src/relay_server.rs` | G2 |
| A1 | 3 | opus | `fe-api/src/canonical_ws/handler.rs`, `fe-api/src/canonical_ws/state.rs`, `fe-api/src/canonical_ws/router.rs`, `fe-api/src/canonical_ws/production.rs` (new) | G2 |
| A2 | 3 | sonnet | `fe-api/src/warehouse_query.rs` (new) | G2 |
| U1 | 3 | sonnet | `fe-ui/src/warehouse_settings.rs` (new), `fe-ui/src/ui_shell/right_sidebar.rs` | G2 |
| G3 | 3-gate | sonnet | `fe-database/src/migration/mod.rs`, `fe-sync/src/lib.rs`, `fe-api/src/lib.rs`, `fe-api/src/canonical_ws/mod.rs`, `fe-ui/src/lib.rs` (mod lines only) | D7, N1, N2, A1, A2, U1 |
| D8 | 4 | sonnet | `fe-database/src/migration/mappings/property.rs` (new), `fe-database/src/migration/mappings/presentation.rs` (new) | G3 |
| D9 | 4 | sonnet | `fe-database/src/migration/mappings/node_lifecycle.rs` (new) | G3 |
| D10 | 4 | opus | `fe-database/src/migration/mappings/editor_batch.rs` (new), `fe-database/tests/migration_equivalence_harness.rs` | G3 |
| R1 | 4 | opus | `fractalengine-relay/src/warehouse.rs` (new), `fractalengine-relay/src/relay_serve.rs`, `fractalengine-relay/src/replicas.rs` | G3 |
| G4 | 4-gate | sonnet | `fe-database/src/migration/mappings/mod.rs` (mod lines), `fractalengine-relay/src/main.rs` (mod line only) | D8, D9, D10, R1 |
| I1 | 5 | sonnet | `fe-runtime/src/messages.rs`, `fe-runtime/src/app.rs`, `fe-runtime/src/channels.rs`, `fe-sync/src/messages.rs` | G4 |
| I2 | 5 | opus | `fe-database/src/lib.rs`, `fe-database/src/command_lane.rs` | I1 |
| I3 | 5 | sonnet | `fe-sync/src/sync_thread.rs`, `fe-sync/src/distributed_query.rs`, `fe-test-harness/src/peer.rs` | I2 |
| I4 | 5 | sonnet | `fe-api/src/server.rs`, `fe-api/src/lib.rs` | I2 |
| I5 | 5 | sonnet | `fractalengine/src/main.rs`, `fractalengine/src/relay_serve_bridge.rs`, `fractalengine-relay/src/main.rs` | I3, I4 |
| I6 | 5 | sonnet | `fe-ui/src/plugin.rs`, `fe-ui/src/verse_manager/db_results/mod.rs` | I1 |
| I7 | 5 | opus | `fe-test-harness/src/scenarios/relay_record_warehouse.rs` (new), `fe-test-harness/src/scenarios/mod.rs`, `fe-test-harness/tests/integration.rs`, `conductor/tracks/relay_record_warehouse_20261120/enforcement-census.md` (new) | I5, I6 |
| I8 | 5 | haiku | every `AGENTS.md` named in "Docs owed" below, `conductor/test-batches.md`, `conductor/doneness-ledger.md`, `conductor/tracks.md`, `fe-canonical-log/src/AGENTS.md` (CDL-PROV register rows) | I7 |

Count: 39 slices. The widest wave is wave 2, with 9 parallel fe-database leaves.
Wave 5 is strictly serial in the order I1 → I2 → (I3 ∥ I4 ∥ I6, which share no
files) → I5 → I7 → I8. Only one of I3/I4/I6 builds at a time.

## Wave 0 — specification errata (docs only, parallel)

Goal: turn each decision into normative text before code. Errata land as dated
entries citing D-CL35..D-CL45, as D-CL28 errata did. All new wire values are
marked provisional (D-CL24(3)).

- [ ] **S1 — SPEC-7 held state (D-CL40).** In §4.2 add `held` as a commit-ack
  state. In §6.3 move `missing_parent`, `unknown_schema`, and `opaque_payload`,
  plus the SPEC-4 `unknown_kind` and `authorization_evidence_unavailable`
  dispositions, from "quarantine disposition, no committed success" to an explicit
  `held`: retryable, identical-bytes resubmission is the retry, no cursor or
  branch fields. Record that this replaces the W4-4 Quarantine→Rejected mapping
  and discharges AC-39. Gate: none (docs).
- [ ] **S2 — SPEC-6 relay erratum and relay-custody durability (D-CL36, D-CL39,
  D-CL43, D-CL45).** Write an erratum to SPEC-6 §6 rules 1 and 3. A record relay
  is still a seeder and never an author, issuer, or checkpoint authority. With the
  verse admin's keyholder setting, it MAY receive the verse scope key through
  SPEC-3 §10.2 wrap delivery and MAY materialize a projection. Blind stays the
  default.

  The new `relay-custody-durability.md` is a scoped SPEC-9 for relay custody only.
  It covers:
  - the custody receipt artifact;
  - the floor rule: R distinct qualifying relays, with R = `ts_replication_factor`;
  - the qualifying-relay predicate;
  - one relay, one verse;
  - the pre-GA reset rule (D-CL38);
  - the NFR-4 placement-privacy analysis: what a receipt and the record-relay list
    reveal against SPEC-3 blinded discovery, and which party sees each.

  This analysis is the SPEC-9 hard gate, scoped to relays. Gate: none (docs).
  Review: critic (opus), because this is contested design.
- [ ] **S3 — SPEC-8 and SPEC-5 errata (D-CL37, D-CL39).** SPEC-8:
  - in `dual_emit_shadow`, admitted candidates are canonical records that may be
    sealed and seeded to record relays;
  - the §2.5 no-shadow-exposure rule still bans legacy public surfaces but no
    longer bans relay seeding;
  - the dual-emit scope is `SCENE_WRITE_CENSUS`;
  - D-CL20 kinds stay `Unsupported(DCl20)` (assumption A-1);
  - `legacy_only` remains a local kill switch (A-8).

  SPEC-5 retention: a peer may release segment bytes behind a verified checkpoint
  only with durability-floor evidence for every covered segment (D-CL39).
  Retention durations stay owner policy. Gate: none (docs).
- [ ] **Verification — wave 0 [checkpoint marker]:** a critic pass reads S1-S3
  against the decision table verbatim and records a one-line verdict in the track's
  review log. Any reading that would need a new owner decision is listed, not
  assumed.

## Wave 1 — pure protocol and type leaves (parallel)

- [ ] **P1 — held state in code (FR-7).** Add `PipelineResult::Held(HeldReason)`
  and `CommitAckState::Held { category }`. Neither has a cursor, and each encodes a
  distinct key set (keys 5-8 structurally absent). The provisional state value is
  `4`. In `compose/storage.rs::admission_outcome_to_pipeline_result`, map these to
  `Held`: `MissingParent`, `UnknownSchema`, `UnknownKind`,
  `AuthorizationEvidenceUnavailable` (fixing the `MissingParent` mislabel at
  `:369-371`), and `OpaquePayload`. Keep every match exhaustive with no wildcard
  arm. Tests: extend `tests/spec7_conformance.rs` with one table-driven case over
  the five dispositions. It asserts the encoded ack has no keys 5-8 and that the
  same bytes resubmitted after the parent arrives reach `Committed` (batch R10, RW).
  Gate: G1.
- [ ] **P2 — custody receipt, floor, compaction gate (FR-5).**
  - `segment/custody_receipt.rs`: a canonical-CBOR `CustodyReceipt {verse_id,
    manifest_id, relay_principal, held_at_ms}` plus Ed25519 signature under the
    provisional domain `fe-custody-receipt-v1\0`.
  - `verify_custody_receipt` returns a `#[non_exhaustive]` `VerifiedCustodyReceipt`.
  - `retention/durability_floor.rs`: `DurabilityFloorEvidence::assess(receipts, r,
    &dyn RelayQualificationView) -> FloorVerdict`. It counts distinct qualifying
    relay principals. A relay qualifies when it holds a valid `seed` capability,
    is on the designated list, and is receipt-bound to this verse.
  - `checkpoint::compaction_decision` gains a **required**
    `&DurabilityFloorEvidence` parameter and refuses `DurabilityFloorNotMet`
    before the tombstone checks. `may_compact_tombstone` delegates as before.

  Tests: extend `tests/cross_spec_integration.rs` with one table-driven floor case
  (duplicate relay, no seed capability, off-list, foreign verse, bad signature,
  floor met). Gate: G1. Security review: yes (new signed artifact).
- [ ] **P3 — `leaf_certificate_id` (FR-6).** Widen
  `wire::session::VerifiedAuthorization` with `leaf_certificate_id: Hash32`, and
  have `ChainCapabilityVerifier` fill it from `capability::chain::VerifiedCapability`.
  Leave the frozen authorize/authorized wire keys alone; this is the in-process
  result type. Tests: extend the existing `wire::session` module tests (R10). Gate: G1.
- [ ] **T1 — `fe_runtime::warehouse` (FR-8).** `VerseWarehouseSettings
  {warehouse_mode, relay_key_custody: Blind|Keyholder, record_relays: Vec<String>,
  peer_prune_after_floor}` with `sanitized(...)` and `from_verse_row(...)`
  matching `timeseries.rs`. An invalid DID in `record_relays` is refused, and a
  duplicate is refused. The type does not hold R, which is read from
  `VerseTimeseriesSettings` (D-CL39). Tests: one table-driven `sanitized` test
  (filter `warehouse::`). Gate: G1.
- [ ] **Q1 — columnar scan provider (FR-13).** `columnar/observation/scan.rs`: a
  DataFusion `TableProvider` over a caller-supplied list of opened observation
  hexon byte buffers. It reads with the existing `observation/reader.rs` and
  pushes projection and filter down to row-group statistics. It has no I/O, keys,
  or SurrealDB access: the caller hands it bytes. Tests: one table-driven aggregate
  test (R3 `cargo test -p fe-query`, RW). Gate: G1 (`--features datafusion`).
- [ ] **G1 — wave-1 gate (serial).** Add `pub mod warehouse;` to
  `fe-runtime/src/lib.rs`. Run `cargo check -p fe-canonical-log --all-targets`,
  `cargo check -p fe-runtime`, and `cargo check -p fe-query --features datafusion`.
  Fix only compile errors in wave-1 files.
- [ ] **Verification — wave 1 [checkpoint marker]:** `/security-review` on P1 and
  P2 (the disclosure surface of `held`, and the receipt signing domain) and
  `/code-review` on P3, T1, and Q1. Both run in a context separate from the
  authors. Advocate pass, then critic pass (house rule). Record each verdict.

## Wave 2 — fe-database storage leaves (parallel, 9 wide)

Interfaces between these leaves are declared here so the slices can run in
parallel. A slice that needs another's type writes against the signature below.
G2 reconciles any drift.

- `local_author::LocalCanonicalAuthor::for_verse(db, custody, verse) ->
  Result<Self>`: signs envelopes with the local device or principal key, owns the
  admitted frontier, and issues HLCs.
- `issuance::issue_chain(db, issuer, audience, verbs, scope, ttl) ->
  Result<ChainBytes>`.
- `custody_ledger::record_receipt(db, bytes) -> Result<ReceiptOutcome>`,
  `floor_evidence(db, verse, manifest_ids) -> DurabilityFloorEvidence`.
- `segment_ingest::ingest_segment(db, verifier, manifest_bytes, fetch:
  &dyn ArtifactFetch) -> IngestReport`.

Slices:

- [ ] **D1 — local author and capability issuance (FR-4, FR-6, FR-1; A-7).**
  `local_author.rs` is the one signer of dual-emitted operations. Its keys come
  from `SecretStore`, and it never exports a private key. `issuance.rs` mints
  SPEC-3 chains from the interim Manager authority
  (`canon_log` §interim-manager-authority) and only for verbs the issuer's role
  grants. It issues `append/op` for the local principal, `seed` and `fetch` for a
  designated record relay (only when the relay DID is on the verse's
  `record_relays` list), and client chains for `/ws/canonical`. Every issued chain
  passes `capability::verify_chain_bytes` before it is returned, as the
  self-check. Tests: one journey in `local_author`'s module covering issue →
  verify → refuse-unauthorized-verb (R10, RW). Gate: G2. **Security review:
  mandatory (authority).**
- [ ] **D2 — segment producer (FR-1).** Pack admitted operations from
  `verified_op_log` into header segments, payload shards, HashSeq nodes, and a
  manifest, sealing every artifact through `seal_artifact` (the only door).
  Write the artifacts to the local-only `canon_sealed_artifact` store. Do this per
  verse, per closed window, and only in warehouse-mode verses (the mode is a
  required argument). Tests: one module journey: pack, then `admit_sealed`
  round-trips every artifact. Gate: G2.
- [ ] **D3 — durable proof environment and inbound segment ingest (FR-2, FR-3).**
  `proof_environment.rs` holds the durable `ParentWalk` and `PayloadAdmission`,
  wrapped in `compose::SealedProofEnvironment`. `segment_ingest.rs` is the one
  inbound door, running `admit_sealed` → receipt re-hash and cross-check →
  `run_proof` → `admit_and_append_durably`. The raw append is
  `pub(crate)`-unreachable. Quarantine uses the per-reason budgets, and a refusal
  is counted, never logged per item. Tests: extend
  `tests/canon_log_composed_admission_test.rs` with one table-driven inbound case
  (forged signature, wrong `op_id`, unreferenced payload, retired epoch, parent
  missing → quarantine then admit) (R10, RW). Gate: G2. **Security review:
  mandatory (inbound P2P admission).**
- [ ] **D4 — durable wire registry (FR-6).** `wire_registry.rs`: a `BranchRegistry`
  over `verified_op_log` plus apply markers (cursors only through the provided
  `issue_cursor`); a `ScopeSnapshotSource` over the projection; and
  `DbThreadCommitPipeline`, a `CanonicalCommitPipeline` whose `submit_candidate`
  sends a command and awaits its reply. Appends stay on the DB thread. The command
  variant is declared in I1, so this slice writes against a
  `CanonicalSubmitSender` trait it defines locally. Tests: module journey over
  `validate`, `replay_after`, and `snapshot_cursor` (R10). Gate: G2.
- [ ] **D5 — custody ledger, checkpoint issuer, prune (FR-5, FR-11).**
  - `custody_ledger.rs` stores verified receipts. It refuses a foreign verse
    (D-CL45) and refuses a relay that is not designated or holds no `seed`
    capability. It answers `floor_evidence`.
  - `checkpoint_issuer.rs` lets a Manager+ node sign a SPEC-5 checkpoint claim
    over covered segments.
  - `prune.rs` is the only caller that releases bytes. It calls
    `compaction_decision` with floor evidence (a required parameter) and only when
    `peer_prune_after_floor` is on.
  - The `warehouse_format_epoch` constant and the wipe rule live here.

  Tests: module journey for "floor not met → refuse, met → release". It asserts
  that a no-resurrection tombstone survives (R10, RW). Gate: G2. **Security
  review: yes.**
- [ ] **D6 — wrap delivery and auto-enrolment (FR-9b, FR-8).**
  - `wrap_delivery.rs` issues `issue_scope_key_wrap` to every enrolled device of
    every principal holding `decrypt` for the scope. It issues to a record relay's
    device only when `relay_key_custody = keyholder`.
  - Wraps are delivered as recipient-sealed replicated rows (A-6). There is one
    new admission arm in `handlers/replicated_row.rs`, placed before the bootstrap
    window as with enrolment. On receipt it verifies issuer authority through
    `DurableDeliveryView` and stores the key only in `SecretStore` custody.
  - Switching keyholder → blind triggers an epoch bump.
  - `device_enrolment.rs` gains `enrol_on_warehouse_open` (idempotent).

  Tests: extend `tests/device_enrolment_test.rs`. A wrap opens only for its
  device, a blind relay gets no wrap, an unauthorized issuer is refused, and a
  stale key fails after the bump (register this file in R10; it is missing from
  the R10 line today). Gate: G2. **Security review: mandatory (key custody).**
- [ ] **D11 — warehouse settings handler (FR-8).** `warehouse_settings.rs`, a
  copy of the `verse_settings.rs` shape: `require_manage_role` at verse scope,
  `VerseWarehouseSettings::sanitized`, an UPDATE of `wh_*` columns, a read-back
  echo, and manifest re-emission. The column DDL is handed to G2 in the slice
  report. Tests: one table-driven role test (Viewer, Editor, petal Manager,
  fractal Manager refused; verse Manager accepted, with the echo equal to the
  read-back) under `handlers::warehouse_settings::` (R10, RW). Gate: G2.
- [ ] **D12 — ts partial reader worker (FR-12).** `ts_partial_reader.rs`: a
  dedicated thread over a clone of the shared handle, consuming a bounded channel
  of partial requests (the channel type is declared in I1, so write against a
  local alias). Keep the deadline skip and the F23 failed-flag rule.
  `inbound_admissions.rs` exposes an `Arc`-shared read-only snapshot of the
  deferred-shard set, so `execute_ts_partial` no longer borrows DB-thread state.
  `execute_ts_partial` takes `&Surreal` (reader) plus the snapshot. Tests: extend
  `handlers::ts_partial` module tests (R10 line already filters it). Gate: G2.
- [ ] **D13 — local BI source (FR-13).** `observation/local_bi.rs` enumerates
  observation hexons held locally (pulled bulk artifacts), opens each with held
  scope keys through the same AAD and key search as the hydrator, and hands bytes
  to Q1's provider. It is read-only and refuses a verse the caller lacks Viewer+
  on. Tests: extend the `canon_log::observation` hydrator journey with one
  scan-vs-hot-plane equality case (R10, RW). Gate: G2.
- [ ] **G2 — wave-2 gate (serial).** Add tables and columns to `schema.rs`:
  `wh_*` verse columns with defaults matching T1, plus `canon_custody_receipt`
  and `canon_scope_key_wrap`. Keep them **out of `ALL_TABLE_NAMES`** (same reason
  as the existing canonical tables). Add `pub mod` lines to `canon_log/mod.rs`,
  `handlers/mod.rs`, and `lib.rs` (for `ts_partial_reader`). Run
  `cargo check -p fe-database --all-targets`.
- [ ] **Verification — wave 2 [checkpoint marker]:** `/security-review` (high) on
  D1, D3, D5, and D6, the four mandatory points. `/code-review` on D2, D4, D11,
  D12, and D13. A critic checks D5 against S2's floor rule. Record each verdict.

## Wave 3 — dual-emit tap and the other crates' leaves (parallel)

- [ ] **D7 — dual-emit tap (FR-4, FR-10).**
  - `scene_write_tap.rs` exposes `dual_emit_scene_write(cmd: &DbCommand, run:
    impl FnOnce() -> Fut)`. It is an exhaustive `match` over every
    `SCENE_WRITE_CENSUS` variant, each arm returning a mapping or
    `Unsupported { reason }` (D-CL20 kinds: `Unsupported(DCl20)`). There is no
    wildcard arm.
  - The tap derives the candidate before `run()`, runs legacy once, then admits
    through `admit_and_append_durably` with `LocalCanonicalAuthor`. It returns
    the legacy result unchanged.
  - `boundary.rs`: `DualEmitShadow` now admits as well as ledgers (the S3
    erratum).
  - `mode.rs`: the default becomes `DualEmitShadow`, and `legacy_only` stays
    parseable (A-8).
  - `mapping_inventory.rs`: add the six missing census variants (Contradiction
    C-4).
  - `mappings/mod.rs` defines the `SceneWriteMapping` trait. The existing
    `transform_mapping` is adapted as its first implementor.

  Tests: extend `tests/migration_conformance_test.rs` with one census-coverage
  test. Every census variant yields a candidate or a counted `Unsupported`, and
  the legacy reply is byte-identical in both modes (R10, RW). Gate: G3.
  **Security review: yes (authoring path).**
- [ ] **N1 — canonical segment transport (FR-1, FR-2, FR-9).**
  `canonical_segments.rs` publishes local sealed artifacts as one HashSeq per
  manifest through `BlobTransport`, and fetches by manifest id from record relays
  first, then peers. It announces new manifests and carries custody receipts as
  opaque signed gossip payloads. It never parses the manifest and hands every
  fetched byte to the DB ingest command, so admission is not done here. Per-peer
  byte and entry caps follow §byte-bounds. Tests: module journey over a
  `virtual_transport` pair (publish → fetch → bytes identical, plus an oversized
  refusal) (R8, RW). Gate: G3. **Security review: yes (inbound bytes).**
- [ ] **N2 — signed plane defaults and record-relay admission (FR-9, FR-14,
  FR-10).**
  - `gossip_auth.rs`: a new `GossipFrameKind::CanonicalAnnounce` (custody receipt
    and manifest announce) with its own `max_age_ms`. The default mode becomes
    `require`.
  - `relay_config.rs`: `RelayAdvertSignedMode` defaults to `require`.
  - `relay_server.rs`: a record-relay profile serves exactly one verse
    (`max_served_verses = 1`, refusing a hosted set of any other size).

  Tests: extend the existing `gossip_auth` and `relay_config` module tests (an
  unsigned frame or row is refused by default), and the `relay_server` tests
  under `--features relay-server` (R8). Gate: G3. **Security review: yes.**
- [ ] **A1 — `/ws/canonical` production state and auth (FR-6, FR-7).**
  - `production.rs` exposes `production_canonical_state(api_state, db, ...) ->
    Arc<CanonicalLogState>`, built from D4's registry and pipeline,
    `DurableAuthorizationView`, and a per-connection
    `ChainCapabilityVerifier::for_requester`.
  - `handler.rs`: the upgrade handler first runs the verse-scoped token check
    (`auth::authenticate` + `require_current_credential`). It passes the token
    principal into the connection as the requester, refusing before upgrade on
    failure. The `chain_id` stand-in is replaced with `leaf_certificate_id`
    (P3). `held` acks are encoded.
  - `state.rs`: an epoch-bump event source subscribes to
    `canonical_epoch` changes and calls `invalidate_on_epoch_bump`. A process-wide
    sweep task replaces the per-connection-only sweep.
  - `router.rs`: `canonical` stops being an `Option`. The function takes
    `Arc<CanonicalLogState>`, so a binary cannot "forget" to mount.

  Tests: extend the `canonical_ws` handler module tests. Cover no-token, foreign
  audience, foreign verse (byte-identical to the existence-oracle test), revoked
  mid-session, and the `held` ack shape (R9, RW). Gate: G3. **Security review:
  mandatory (auth).**
- [ ] **A2 — local BI route (FR-13).** `warehouse_query.rs`: a read-only `POST
  /api/v1/verses/{v}/warehouse/query` taking a structured spec (the same shapes
  as `fe_runtime::distributed_query`, never SQL). It requires Viewer+ at verse
  scope plus `query_guard` limits, and runs Q1 over D13's bytes. The route is
  mounted in I4. Tests: module tests under `warehouse_query::` (R9, RW). Gate: G3.
- [ ] **U1 — warehouse settings surface (FR-8).** `warehouse_settings.rs` holds a
  state resource mirroring per-verse settings. It is written with the `SettingDraft`
  pattern and displays only the read-back echo. `right_sidebar.rs` adds a
  Warehouse section directly under Timeseries Fabric with:
  - a mode toggle;
  - a custody radio (Blind / Keyholder);
  - a record-relay DID list with add and remove;
  - the R value read-only with an "R relays needed, N listed" warning;
  - a prune toggle.

  Controls are disabled with an accessible reason for non-Managers. Apply
  `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Tests: one module test of the
  draft/echo cycle under `warehouse_settings::` (RW). Gate: G3.
- [ ] **G3 — wave-3 gate (serial).** Add mod lines to `migration/mod.rs`
  (`scene_write_tap`, `mappings`), `fe-sync/src/lib.rs`, `fe-api/src/lib.rs`,
  `fe-api/src/canonical_ws/mod.rs`, and `fe-ui/src/lib.rs`. Run `cargo check -p
  fe-database -p fe-sync -p fe-api -p fe-ui` serially. Also run `cargo check -p
  fe-sync --features relay-server`.
- [ ] **Verification — wave 3 [checkpoint marker]:** `/security-review` (high) on
  A1, N1, N2, and D7. `/code-review` on A2 and U1, with a UI review against the
  UI-UX guidance. Record each verdict.

## Wave 4 — mappings and the relay service (parallel)

- [ ] **D8 — property and presentation mappings.** Provisional schemas for
  `SetNodeProperty`, `DeleteNodeProperty`, `ClearNodeProperties`, `UpdateNodeUrl`,
  `SetNodeView`, and `CommitStampOverrides`. One kind-1 member each, payload
  sealed at verse scope, schema hash by the SPEC-10 §4.1 rule 3 convention, rows
  handed to I8 for the CDL-PROV register. Tests: extend the
  `migration_equivalence_harness` corpus through D10's table, using the
  harness-extension hook D10 adds: one row per kind (R10). Gate: G4.
- [ ] **D9 — node lifecycle mappings.** `CreateNode`, `CreateNodeWithAsset`,
  `PlaceStoredGlb`, `CreateObject`, `PromoteInstance`, `PromoteCheckedStamp`,
  `DeleteNode`, `TombstoneNode`, `CascadeTombstoneNode`. Creation carries a
  `legacy_identifier` anchor, and tombstones are SPEC-4 tombstone intents.
  `CascadeTombstoneNode` is a multi-member candidate set. Tests: as D8. Gate: G4.
- [ ] **D10 — multi-row mappings and harness extension.** `CommitCheckedEditorBatch`,
  `ImportPetalSnapshot`, `CommitNodeMutation`, `CommitCheckedNodeMutation`: atomic
  candidate sets (all members admitted or none). If a kind cannot be mapped
  atomically, it gets `Unsupported { reason }` with the reason recorded; it is not
  forced. Owns the harness file and adds the table-driven per-kind corpus hook D8
  and D9 feed. Tests: `migration_equivalence_harness` (R10, RW). Gate: G4.
- [ ] **R1 — relay warehouse service (FR-1, FR-3, FR-11, FR-14).**
  - `warehouse.rs` runs the record-relay loop:
    1. accept a seed only through N1 plus the DB custody ingest;
    2. re-hash and store;
    3. sign and return a custody receipt with the relay's node key;
    4. in keyholder mode, replay admitted operations into the relay projection;
    5. on a `warehouse_format_epoch` mismatch, wipe and request re-seed (D-CL38).
  - `relay_serve.rs` and `replicas.rs`: `FE_RELAY_VERSE=<id>` is required, and the
    relay opens only that verse's replica. Zero or more than one verse exits
    non-zero with a named reason. This replaces `load_hosted_verse_ids` for record
    mode.

  Tests: extend `relay_serve` module tests (`cargo test -p fractalengine-relay
  --features relay-server -- relay_serve`) and add `warehouse::` module tests
  (R8, RW). Gate: G4. **Security review: mandatory (key custody on a
  third-party host).**
- [ ] **G4 — wave-4 gate (serial).** Add mod lines to `migration/mappings/mod.rs`
  and `fractalengine-relay/src/main.rs` (`mod warehouse;` only). Run `cargo check
  -p fe-database --all-targets` and `cargo check -p fractalengine-relay --features
  relay-server`.
- [ ] **Verification — wave 4 [checkpoint marker]:** `/code-review` high on
  D8-D10 (the equivalence argument for each mapping) and `/security-review` on R1.
  Record each verdict.

## Wave 5 — serial integration

- [ ] **I1 — messages and channels.** New `DbCommand` variants:
  `SetVerseWarehouseSettings`, `SubmitCanonicalCandidate {bytes, payload, reply}`,
  `IngestCanonicalSegment {manifest, fetch_ticket, reply}`, `RecordCustodyReceipt`,
  `PackClosedSegments`, `PruneBehindCheckpoint`. Remove `ExecuteTsPartial`, which
  moves to a `TsPartialRequest` on its own channel in `channels.rs`. New
  `DbResult` variants: `VerseWarehouseSettingsSet` (read-back echo) and
  `CustodyFloorChanged`. New `SyncCommand` variants: `SeedSegments`,
  `FetchSegment`, `PublishCustodyReceipt`. Classify reply families in `app.rs` per
  `fe-runtime` §api-reply-correlation. Gate: `cargo check -p fe-runtime -p
  fe-sync`.
- [ ] **I2 — DB dispatch.** In `lib.rs`:
  - route every census scene-write arm through `dual_emit_scene_write`, which
    makes the tap the only path;
  - add the new command arms;
  - delete the `ExecuteTsPartial` arm (lines ~2304-2335 at `92946362`);
  - spawn the D12 reader worker beside the DB thread in `spawn_db_thread_gated`,
    with the shared handle clone.

  In `command_lane.rs`, classify the new commands: ingest is a row-apply barrier
  class; submit is a write. Gate: `cargo check -p fe-database --all-targets`.
  Review: `/code-review` high (dispatch is where "built, never wired" happens).
- [ ] **I3 — sync wiring.** In `sync_thread.rs`:
  - handle `SeedSegments`, `FetchSegment`, and `PublishCustodyReceipt` through N1;
  - feed inbound `CanonicalAnnounce` frames to `IngestCanonicalSegment` and
    `RecordCustodyReceipt`;
  - auto-enrol on warehouse-verse open (D6).

  `distributed_query.rs` sends partials to the reader channel instead of the DB
  channel, and the harness `peer.rs` matches. Gate: `cargo check -p fe-sync -p
  fractalengine-test-harness`.
- [ ] **I4 — API wiring.** In `server.rs` and `lib.rs`, `run_server` calls
  `build_router_with_canonical_log(state, production_canonical_state(...))`, so
  the mount is no longer optional. Mount A2's route. Do not touch
  `fe-api/src/mcp/*` (uncommitted edits by another session). Gate: `cargo check
  -p fe-api`.
- [ ] **I5 — binaries.** `fractalengine/src/main.rs` and `relay_serve_bridge.rs`:
  log the canonical surfaces at startup (FR-10), spawn the segment pack/seed
  timer for warehouse verses, and leave desktop relay mode unchanged (A-5).
  `fractalengine-relay/src/main.rs`: record-relay mode by `FE_RELAY_VERSE`, start
  the R1 service, and keep startup ordering (gate before the sync thread). Gate:
  `cargo check -p fractalengine -p fractalengine-relay --features relay-server`.
- [ ] **I6 — UI wiring.** In `plugin.rs`, register the warehouse state resource.
  In `db_results/mod.rs`, fold `VerseWarehouseSettingsSet` and the hierarchy
  columns into U1's state, inside an existing `SystemParam` bundle (the 16-param
  ceiling). Gate: `cargo check -p fe-ui`.
- [ ] **I7 — seam tests, harness scenario, enforcement census.**
  - Scenario `relay_record_warehouse`: peer A, peer B, and two record relays,
    with R=2 in a warehouse verse. A writes. Both relays return receipts and
    the floor is met. A goes offline. B joins, fetches from the relays, and
    replays; the projection root equals A's (AC-1.1, AC-3.1, AC-5.1). Then a
    forged segment is refused (AC-2.1), and a relay started with two verses
    refuses (AC-14.1).
  - Register the scenario in `scenarios/mod.rs` and `tests/integration.rs`.
  - **Enforcement census** (`enforcement-census.md`): list every `fn
    (assert|validate|verify|check|require|may|decide)_\w+` in the touched trees
    (fe-canonical-log; fe-database canon_log, migration, and handlers touched
    here; fe-sync gossip_auth, relay_config, relay_server, canonical_segments;
    fe-api canonical_ws and warehouse_query; fractalengine-relay). Classify each
    as production-called, test-only-by-design (with reason), or dormant. The
    gate is **0 dormant**.
  - Close greps:
    - `build_router_with_canonical_log` has exactly one production caller per
      binary;
    - every `SCENE_WRITE_CENSUS` variant appears in `scene_write_tap.rs`;
    - `execute_ts_partial` is called only from `ts_partial_reader.rs`;
    - `compaction_decision` callers all pass floor evidence;
    - `seal_artifact` remains the only seal path.

  Tests run: none here. The scenario joins the Harness batch and runs when the
  owner names it.
- [ ] **I8 — docs and bookkeeping (haiku).** Coordinate with the agent that owns
  tracks.md, RUNBOOK.md, the ledger, and AGENTS.md files. Hand it these edits, or
  take them over once that agent releases them. List under "Docs owed".
- [ ] **Verification — wave 5 [checkpoint marker]:** `/security-review` (high) of
  the composed system: auth, issuance, key custody, and inbound admission
  end to end. A critic checks the shipped behavior against `conductor/product.md`
  and D-CL35..D-CL45 verbatim. The owner then names the `RW` and `Harness` batches
  for the one test sweep.

## Docs owed (I8)

- `fe-canonical-log/src/AGENTS.md`: §trait-composition-map rows (wire
  `CapabilityVerifier`, `ProofEnvironment`, `CanonicalCommitPipeline` now have
  production callers), plus Provisional wire numbering rows (held state value,
  custody receipt keys and domain, mapping schemas from D8-D10).
- `fe-canonical-log/src/{segment,retention,wire}/AGENTS.md`: custody receipt, the
  floor, `held`.
- `fe-api/src/canonical_ws/AGENTS.md`: replace §deliberately-unmounted with §mount
  and §auth (token, then chain). Update Known limitations.
- `fe-database/src/canon_log/AGENTS.md`: rewrite §parallel-and-dormant (C-12);
  add §issuance, §segment-production, §segment-ingest, §custody-ledger,
  §wrap-delivery, §warehouse.
- `fe-database/src/migration/AGENTS.md`: §boundary, §no-shadow-exposure (S3
  erratum), §scene-write-tap, §mapping-inventory census sync.
- `fe-database/src/AGENTS.md`: §ts-partials (reader worker), §warehouse-settings.
- `fe-sync/src/AGENTS.md`: §canonical-segments, §gossip-signing (default
  require), §relay-advertise (signed require), §peer-relay-server (record relay,
  one verse).
- `fe-runtime/src/AGENTS.md`: §warehouse, new commands and channel.
- `fe-ui/src/ui_shell/AGENTS.md`: §warehouse-settings.
- `fractalengine-relay/AGENTS.md`: record-relay mode.
- `conductor/test-batches.md`: add batch `RW` (below); add
  `device_enrolment_test` to the R10 line.
- `conductor/doneness-ledger.md`: close CDL-NET, CANON-WS, SYNC-SIGNED-PLANE
  (remainder), and CDL-WRAP-DELIVERY. Move CANON-DUAL-EMIT to "wired, cutover
  gated".

## New test batch `RW` (registered by I8)

```bash
cargo test -p fe-canonical-log --test spec7_conformance --test cross_spec_integration
cargo test -p fe-canonical-log --lib -- wire::commit wire::session segment::custody_receipt retention::durability_floor checkpoint::
cargo test -p fe-runtime --lib -- warehouse::
cargo test -p fe-query --features datafusion --lib -- columnar::observation::scan
cargo test -p fe-database --lib -- canon_log:: migration:: handlers::warehouse_settings handlers::ts_partial ts_partial_reader
cargo test -p fe-database --test canon_log_composed_admission_test --test migration_conformance_test --test migration_equivalence_harness --test device_enrolment_test
cargo test -p fe-sync --lib -- canonical_segments gossip_auth relay_config
cargo test -p fe-sync --features relay-server --lib -- relay_server
cargo test -p fe-api --lib -- canonical_ws:: warehouse_query
cargo test -p fractalengine-relay --features relay-server -- relay_serve warehouse
cargo test -p fe-ui --lib -- warehouse_settings
cargo test -p fractalengine-test-harness -- relay_record_warehouse
```

Run only through `tools\qa\test_batches.ps1 -Batches RW` when named, and never
with `cargo test --workspace`.

## Security-review points (mandatory)

| Point | Slices |
| --- | --- |
| Auth: token, then chain; requester binding; existence oracle | A1, I4 |
| Crypto: custody receipt domain, wrap sealing, signing defaults | P2, D6, N2 |
| Key custody: issuance root, keyholder relay, epoch bump on custody change | D1, D6, R1 |
| Inbound P2P admission: segment ingest, transport caps, quarantine budgets | D3, N1, I3 |

## Risks

1. Dispatch rewrite (I2) touches every scene-write arm. Mitigation: the exhaustive
   tap match, plus the byte-identical-legacy test (AC-4.2).
2. Signing defaults flip to `require`. Older peers stop interoperating. This is
   pre-GA and accepted (D-CL38), and it must be called out in release notes.
3. Issuance rests on the interim Manager authority (A-7). Re-anchoring it when
   SPEC-2 authority history lands is a breaking change.
4. Build pressure: fe-database recompiles surrealdb-core. Use one builder and
   prune PDBs between gates.
5. Mapping effort for multi-row kinds (D10) may be larger than one slice. The
   escape is an explicit counted `Unsupported`, never a forced mapping.
