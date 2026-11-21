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

The plan has 45 slices. Waves 0-4 run as parallel leaves. Each of waves 1-4 ends
with one serial gate slice (G1-G4). Wave 5 runs strictly serially, one slice at a
time. The machine-readable copy is `slices.json`, in execution order.

Partitions were computed against HEAD `98470a73`, read from
`.git/refs/heads/main`. Every existing `owns` path was checked with Glob at that
commit. If HEAD has moved, re-grep before launch.

### House rules for every slice

- **Leaves never build.** Only gate slices and wave-5 slices run `cargo check`.
  They run one at a time, one builder, under the RUNBOOK §5 doctrine:
  `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 -j4`.
- **No tests run inside slices.** Write or extend tests only. They run when a
  steer names the batch: `RW` below, or the named region batches.
- **Extend before you add.** Extend an existing journey file or test module. A new
  module gets at most one table-driven journey test, and the `RW` filter must
  reach it.
- **No clippy.** The gate is `cargo check -p <crate>`.
- **Make every check unavoidable by construction.** Use a required parameter, a
  `pub(crate)` raw path, or a `#[non_exhaustive]` result. Never use an `Option`
  that a caller can fill with `None`.
- **Keep `fe-canonical-log` pure.** It must not depend on fe-runtime, fe-identity,
  fe-policy, fe-database or surrealdb. Its `lib.rs` and `Cargo.toml` stay frozen.
- **No dependency edits.** A slice that needs a new crate dependency stops and
  reports. It does not edit `Cargo.toml`.
- **No MCP tools are added.** Everything new in fe-api is REST only. This keeps
  `tools/fe-cli/catalog/mcp-tools.json` and `fe-api/tests/mcp_integration.rs`
  (tool count 64) unchanged.

### Files no slice may own

Another live session holds these:

- `fe-renderer/src/camera.rs`
- `fe-webview/src/portal_bundle/ipc.rs`
- `fe-webview/src/portal_bundle/limits.rs`
- `fe-sdk/src/render_mesh.rs`
- `tools/fe-ext/**`
- `docs/spec/portal-bundle/ipc-v1.md`
- `docs/extensions/*`
- `fe-plugin/src/wasm/*`
- `conductor/RUNBOOK.md`

`conductor/test-batches.md` has fresh commits from that session, so I8 appends to
it and never rewrites it.

Tier key: haiku = docs and bookkeeping; sonnet = standard code; opus = protocol,
crypto, authorization and inbound admission.

## Waves

| Wave | Slices | Mode |
| --- | --- | --- |
| 0 | S1, S2, S3 | parallel (docs) |
| 1 | P1, P2, P3, T1, Q1 → G1 | parallel leaves, then serial gate |
| 2 | D1, D2, D3, D4, D5, D6, D11, D12, D13 → G2 | parallel leaves (widest, 9), then serial gate |
| 3 | D7, N1, N2, A1, A2, A3, U1 → G3 | parallel leaves, then serial gate |
| 4 | D8, D9, D10, D14, D15, D16, R1 → G4 | parallel leaves, then serial gate |
| 5 | I1 → I2 → I3 → I4 → I6 → I5 → C1 → I7 → QS1 → I8 | **strictly serial**, one slice at a time, each its own check |

**Every wave-5 slice is strictly serial.** None may start before its predecessor's
check passes. This includes I3, I4 and I6, which share no files. They still run
one after another, because each builds and the machine allows one builder at a
time.

## Wave 0 — specification errata

Errata land as dated entries citing the decision. Every new wire value is marked
provisional (D-CL24(3)).

### S1 — SPEC-7 held state

**Owns:** `docs/spec/canonical-log/commit-preview-wire.md`. Tier: sonnet.
**Decision:** D-CL40.

- **§4.2:** add `held` as an ack state.
- **§6.3:** move these dispositions from the quarantine row to `held`:
  - `missing_parent`
  - `unknown_schema`
  - `opaque_payload`
  - SPEC-4 `unknown_kind`
  - `authorization_evidence_unavailable`

  `held` is retryable: the client resubmits the identical bytes. It carries no
  cursor or branch.
- **Record** that this replaces the W4-4 Quarantine→Rejected mapping and
  discharges AC-39.

### S2 — SPEC-6 relay erratum and relay-custody durability

**Owns:** `docs/spec/canonical-log/segment-shard-relay.md`,
`docs/spec/canonical-log/relay-custody-durability.md` (new). Tier: opus.
**Decisions:** D-CL36, D-CL39, D-CL43, D-CL45, D-CL48.

**SPEC-6 §6 erratum.** Rules 1 and 3 change:
- A record relay stays a seeder. It is never an author, an issuer, or a checkpoint
  authority.
- Only when the admin's keyholder setting is on may it receive the scope key
  through §10.2 wrap delivery and materialize a projection.
- Blind is the default.

**New spec, scoped to relay custody only (SPEC-9).** It defines:
- the custody receipt;
- the floor: R distinct qualifying relays, with R = `ts_replication_factor`;
- the qualifying-relay predicate;
- one relay per verse, for record relays only (D-CL48);
- the pre-GA reset rule (D-CL38);
- the NFR-4 placement-privacy analysis: what the receipts and the relay list
  reveal, to whom, measured against SPEC-3 blinded discovery.

**Review:** critic.

### S3 — SPEC-8, SPEC-4 and SPEC-5 errata, including D-CL20 lifted-kind atomicity

**Owns:** `docs/spec/canonical-log/migration-plan.md`,
`docs/spec/canonical-log/log-first-materialization.md`,
`docs/spec/canonical-log/branches-checkpoints-retention.md`. Tier: opus.
**Decisions:** D-CL37, D-CL39, D-CL47.

**SPEC-8 (`migration-plan.md`):**
- Admitted dual-emit candidates are canonical records and may be seeded to record
  relays.
- §2.5 still forbids legacy public surfaces.
- The scope is `SCENE_WRITE_CENSUS`.
- `legacy_only` stays as a kill switch.
- §4.2.5 now names only `CreateVerse`, `CreateFractal` and `CreatePetal` as
  deferred.

**SPEC-4 §10 erratum — multi-row atomicity (`log-first-materialization.md`).**
Write an atomic intent contract for each lifted kind:
- **`ImportGltf`:** node row plus asset row plus content hash. The asset bytes are
  referenced by hash and never inlined.
- **`DuplicateNode`:** new node plus copied properties, with the source anchor
  recorded.
- **`RenameNode`:** single-row display name.

Also state the general rule: a multi-member candidate set is admitted
all-or-nothing, and a partial admission rolls back with no member appended.

**SPEC-5 (`branches-checkpoints-retention.md`):** pruning behind a checkpoint
requires floor evidence for every covered segment.

### Verification — wave 0 [checkpoint marker]

A critic reads S1-S3 against D-CL35..D-CL49 verbatim. The one-line verdict is
recorded. Any reading that would need a new owner decision is listed, not assumed.

## Wave 1 — pure protocol and type leaves

### P1 — held state in code

**Owns:** `fe-canonical-log/src/wire/commit.rs`, `fe-canonical-log/src/wire/error.rs`,
`fe-canonical-log/src/compose/storage.rs`,
`fe-canonical-log/tests/spec7_conformance.rs`. Tier: sonnet. **Security review:** yes.

**Code changes:**
- Add `PipelineResult::Held(HeldReason)` and `CommitAckState::Held { category }`.
  The provisional state value is `4`, and keys 5-8 are absent.
- In `admission_outcome_to_pipeline_result`, map the five quarantine outcomes to
  `Held`.
- Fix the `AuthorizationEvidenceUnavailable`→`MissingParent` mislabel at
  `storage.rs:369-371`.
- Every match stays exhaustive, with no wildcard.

**Test:** extend `spec7_conformance.rs` with one table-driven case over the five
outcomes. It asserts no keys 5-8, and that a resubmit after the parent arrives
returns `Committed`.

### P2 — custody receipt, durability floor, compaction gate

**Owns:** `fe-canonical-log/src/segment/custody_receipt.rs` (new),
`fe-canonical-log/src/segment/mod.rs`,
`fe-canonical-log/src/retention/durability_floor.rs` (new),
`fe-canonical-log/src/retention/mod.rs`, `fe-canonical-log/src/checkpoint.rs`,
`fe-canonical-log/tests/cross_spec_integration.rs`. Tier: opus.
**Security review:** yes.

**New code:**
- **`CustodyReceipt`:** canonical CBOR plus Ed25519, under the domain
  `fe-custody-receipt-v1\0`.
- **`verify_custody_receipt`:** returns a `#[non_exhaustive]` verified type.
- **`DurabilityFloorEvidence::assess(receipts, r, &dyn RelayQualificationView)`:**
  produces the floor evidence.

**Changed code:** `checkpoint::compaction_decision` gains a **required**
`&DurabilityFloorEvidence` parameter and refuses `DurabilityFloorNotMet` before
the tombstone checks.

**Test:** extend `cross_spec_integration.rs` with one table-driven floor case.

### P3 — leaf_certificate_id

**Owns:** `fe-canonical-log/src/wire/session.rs`, `fe-canonical-log/src/compose/wire.rs`.
Tier: sonnet.

**Change:** widen `VerifiedAuthorization` with `leaf_certificate_id: Hash32`,
filled by `ChainCapabilityVerifier`. This is in-process only; wire keys are
unchanged.

**Test:** extend the `wire::session` tests.

### T1 — fe_runtime::warehouse

**Owns:** `fe-runtime/src/warehouse.rs` (new). Tier: sonnet.

**New type:** `VerseWarehouseSettings {warehouse_mode, relay_key_custody,
record_relays, peer_prune_after_floor}`, with `sanitized` and `from_verse_row` in
the style of `timeseries.rs`.
- `sanitized` refuses a bad or duplicate DID.
- R is not held here.

**Test:** one table-driven `warehouse::` test.

### Q1 — columnar scan provider

**Owns:** `fe-query/src/columnar/observation/scan.rs` (new),
`fe-query/src/columnar/observation/mod.rs`. Tier: sonnet.

**New code:** a DataFusion `TableProvider` over caller-supplied, already-opened
hexon bytes, via `observation/reader.rs`. It pushes projections and filters down
to row-group statistics. It does no I/O and holds no keys.

**Test:** one aggregate test.

### G1 — wave-1 gate

**Owns:** `fe-runtime/src/lib.rs` (mod line only). Serial.

**Steps:**
1. Add `pub mod warehouse;`.
2. Run:
   `cargo check -p fe-canonical-log --all-targets && cargo check -p fe-runtime && cargo check -p fe-query --features datafusion`.
3. Fix compile errors in wave-1 files only.

### Verification — wave 1 [checkpoint marker]

- `/security-review` on P1 and P2.
- `/code-review` on P3, T1 and Q1.
- Run an advocate pass, then a critic pass, in separate contexts, and record the
  verdicts.

## Wave 2 — fe-database storage leaves (9 wide)

**Interface contract between the D-slices.** G2 reconciles any drift.

- `LocalCanonicalAuthor::for_verse(db, custody, verse)`
- `issuance::issue_chain(db, issuer, audience, verbs, scope, ttl)`
- `custody_ledger::{record_receipt, floor_evidence}`
- `segment_ingest::ingest_segment(db, verifier, manifest_bytes, &dyn ArtifactFetch)`

### D1 — local author and capability issuance

**Owns:** `fe-database/src/canon_log/local_author.rs` (new),
`fe-database/src/canon_log/issuance.rs` (new). Tier: opus.
**Security review:** mandatory. **Decision:** D-CL46.

- **Local author.** This is the one signer. Its keys come from `SecretStore` and
  are never exported.
- **Issuance.** Chains are rooted in the verse role table at Manager+ (D-CL46),
  and only for verbs the issuer's role grants. It issues:
  - `append/op` for the local principal;
  - `seed`/`fetch` for a relay, only if the relay is listed in `record_relays`;
  - client chains for `/ws/canonical`.
- **Self-check.** Every chain passes `verify_chain_bytes` before it is returned.

**Test:** one module journey: issue → verify → refuse an unauthorized verb.

### D2 — segment producer

**Owns:** `fe-database/src/canon_log/segment_producer.rs` (new). Tier: sonnet.

**What it does:** pack admitted operations into a header segment, payload shards,
HashSeq nodes and a manifest.
- It seals only through `seal_artifact`.
- It writes to `canon_sealed_artifact`.
- It runs per verse and per closed window.
- Warehouse mode is a required argument.

**Test:** module journey: pack, then `admit_sealed` round-trips each artifact.

### D3 — durable proof environment and inbound segment ingest

**Owns:** `fe-database/src/canon_log/proof_environment.rs` (new),
`fe-database/src/canon_log/segment_ingest.rs` (new),
`fe-database/tests/canon_log_composed_admission_test.rs`. Tier: opus.
**Security review:** mandatory.

- **Durable traits.** Implement `ParentWalk` and `PayloadAdmission` durably, inside
  `SealedProofEnvironment`.
- **One inbound door,** in this order:
  1. `admit_sealed`
  2. re-hash and cross-check
  3. `run_proof`
  4. `admit_and_append_durably`
- **Quarantine.** Per-reason budgets; refusals are counted, not logged per item.

**Test:** extend the admission test with one table-driven inbound case:
- forged signature;
- wrong `op_id`;
- unreferenced payload;
- retired epoch;
- missing parent → quarantine, then admit once the parent arrives.

### D4 — durable wire registry

**Owns:** `fe-database/src/canon_log/wire_registry.rs` (new). Tier: sonnet.

**New types:**
- A durable `BranchRegistry`. Cursors are created only via `issue_cursor`.
- A `ScopeSnapshotSource`.
- `DbThreadCommitPipeline`, which submits over a local `CanonicalSubmitSender`
  trait. I1 supplies the real command.

**Test:** module journey.

### D5 — custody ledger, checkpoint issuer, prune

**Owns:** `fe-database/src/canon_log/custody_ledger.rs` (new),
`fe-database/src/canon_log/checkpoint_issuer.rs` (new),
`fe-database/src/canon_log/prune.rs` (new). Tier: opus. **Security review:** yes.

- **Custody ledger.** Stores verified receipts. It refuses:
  - a foreign verse;
  - an unlisted relay;
  - a relay without a `seed` capability.
- **Checkpoint issuer.** A Manager+ checkpoint claim.
- **Prune.** The only byte release, gated by `compaction_decision` with floor
  evidence and `peer_prune_after_floor`.
- **Reset.** `warehouse_format_epoch`, plus the wipe rule.
- **Status read.** Exposes a `warehouse_status(db, verse)` read: floor per recent
  manifest, receipt counts, and the projection root. A3 serves it.

**Test:** module journey: floor not met → refuse; floor met → release; the
no-resurrection tombstone survives.

### D6 — wrap delivery and auto-enrolment

**Owns:** `fe-database/src/canon_log/wrap_delivery.rs` (new),
`fe-database/src/canon_log/device_enrolment.rs`,
`fe-database/src/handlers/replicated_row.rs`,
`fe-database/tests/device_enrolment_test.rs`. Tier: opus.
**Security review:** mandatory.

- **Issuing.** `issue_scope_key_wrap` goes to the enrolled devices of `decrypt`
  holders, and to a relay's device only when the verse is in keyholder custody.
- **Delivery.** Wraps travel as recipient-sealed replicated rows (A-6), with one
  admission arm placed before the bootstrap window.
- **On receipt.** Verify via `DurableDeliveryView`, then store into `SecretStore`.
- **Custody change.** Keyholder → blind bumps the epoch.
- **Enrolment.** Add `enrol_on_warehouse_open`.

**Test:** extend `device_enrolment_test.rs`.

### D11 — warehouse settings handler

**Owns:** `fe-database/src/handlers/warehouse_settings.rs` (new). Tier: sonnet.

- **Shape.** Same as `verse_settings.rs`: `require_manage_role` at verse scope,
  then sanitize, then UPDATE the `wh_*` columns, then a read-back echo, then
  manifest re-emission.
- **Hand-off.** Pass the column DDL to G2.

**Test:** one table-driven role test under `handlers::warehouse_settings::`.

### D12 — ts partial reader worker

**Owns:** `fe-database/src/handlers/ts_partial.rs`,
`fe-database/src/ts_partial_reader.rs` (new),
`fe-database/src/handlers/inbound_admissions.rs`. Tier: sonnet.

- **Worker.** A dedicated reader thread on a clone of the shared handle, with a
  bounded channel (local alias until I1).
- **Shared state.** An `Arc` snapshot of the deferred shards.
- **Signature change.** `execute_ts_partial(&Surreal, snapshot, ...)`.
- **Preserved behaviour.** The deadline skip and the F23 failed-flag.

**Test:** extend the `handlers::ts_partial` tests.

### D13 — local BI source

**Owns:** `fe-database/src/canon_log/observation/local_bi.rs` (new),
`fe-database/src/canon_log/observation/mod.rs`. Tier: sonnet.

**What it does:** enumerate the observation hexons held locally and open them with
held keys, using the hydrator's AAD and key search. Read-only, and it requires
Viewer+.

**Test:** one scan-vs-hot-plane equality case in the hydrator journey.

### G2 — wave-2 gate

**Owns:** `fe-database/src/schema.rs`, `fe-database/src/canon_log/mod.rs`,
`fe-database/src/handlers/mod.rs`, `fe-database/src/lib.rs` (mod lines only).
Serial.

**Steps:**
1. Add the `wh_*` verse columns, plus `canon_custody_receipt` and
   `canon_scope_key_wrap`. Keep them out of `ALL_TABLE_NAMES`.
2. Add the mod lines.
3. Run: `cargo check -p fe-database --all-targets`.

### Verification — wave 2 [checkpoint marker]

- `/security-review` (high) on D1, D3, D5 and D6.
- `/code-review` on the rest.
- A critic checks D5 against S2.
- Record the verdicts.

## Wave 3 — dual-emit tap and leaves in the other crates

### D7 — dual-emit tap, D-CL20 lift, inventory sync

**Owns:**
- `fe-database/src/migration/scene_write_tap.rs` (new)
- `fe-database/src/migration/mappings/mod.rs` (new)
- `fe-database/src/migration/boundary.rs`
- `fe-database/src/migration/mode.rs`
- `fe-database/src/migration/mapping_inventory.rs`
- `fe-database/src/handlers/crud.rs`: only the bypass one-liners at `:320`, `:1694`
  and `:1816`
- `fe-database/tests/migration_conformance_test.rs`
- `fe-database/tests/dcl20_bypass_counter_test.rs`

Tier: opus. **Security review:** yes. **Decisions:** D-CL37, D-CL47.

**The tap.** `dual_emit_scene_write(cmd, run)` is an exhaustive match over all 23
`SCENE_WRITE_CENSUS` variants, with no wildcard. Each arm names its mapping module:

| Module | Variants |
| --- | --- |
| `transform_mapping` | `UpdateNodeTransform` |
| D8 | the property and presentation variants |
| D9 | the lifecycle variants |
| D10 | the multi-row variants |
| D14 | `ImportGltf` |
| D15 | `DuplicateNode` |
| D16 | `RenameNode` |

**Write path.** Derive the candidate set before `run()`. Run legacy once. Admit
via `admit_and_append_durably` with `LocalCanonicalAuthor`. Return the legacy
result unchanged.

**`boundary.rs`.** In dual-emit mode, admit as well as write to the ledger.

**`mode.rs`.** The default becomes `DualEmitShadow`; `legacy_only` is kept (A-8).

**`mapping_inventory.rs`:**
- Add the six missing census variants.
- Shrink `D_CL20_MUTATION_KINDS` to `CreateVerse`, `CreateFractal` and
  `CreatePetal`.
- Let `register_mapped` accept the three lifted kinds.

**`crud.rs`.** Remove the three lifted bypass counter lines.

**`dcl20_bypass_counter_test.rs`.** Now pins only the three remaining kinds.

**`mappings/mod.rs`.** Defines the `SceneWriteMapping` trait, including the
atomic-candidate-set contract from S3.

**Test:** one census-coverage test in `migration_conformance_test.rs`. Every
variant is mapped, `Unsupported = 0` is required, and the legacy reply is
byte-identical in both modes.

### N1 — canonical segment transport

**Owns:** `fe-sync/src/canonical_segments.rs` (new). Tier: opus.
**Security review:** yes.

**What it does:**
- Publishes one HashSeq per manifest via `BlobTransport`.
- Fetches from record relays first, then from peers.
- Announces manifests and carries receipts as opaque signed gossip payloads.
- Never parses a manifest; hands every byte to the DB ingest command.
- Respects the §byte-bounds caps.

**Test:** module journey over `virtual_transport`.

### N2 — signed-plane defaults and record-relay profile

**Owns:** `fe-sync/src/gossip_auth.rs`, `fe-sync/src/relay_config.rs`,
`fe-sync/src/relay_server.rs`. Tier: sonnet. **Security review:** yes.
**Decision:** D-CL48.

- **`gossip_auth.rs`.** Add `GossipFrameKind::CanonicalAnnounce`. The mode default
  becomes `require`.
- **`relay_config.rs`.** `RelayAdvertSignedMode` defaults to `require`.
- **`relay_server.rs`.** Add a record-relay profile with
  `max_served_verses = 1`, which refuses any other hosted-set size. Desktop mode is
  unchanged (D-CL48).

**Test:** extend the existing module tests; `relay_server` needs
`--features relay-server`.

### A1 — /ws/canonical production state and auth

**Owns:** `fe-api/src/canonical_ws/handler.rs`, `fe-api/src/canonical_ws/state.rs`,
`fe-api/src/canonical_ws/router.rs`, `fe-api/src/canonical_ws/production.rs` (new).
Tier: opus. **Security review:** mandatory. **Decisions:** D-CL41, D-CL46.

- **`production_canonical_state`.** Built from D4, `DurableAuthorizationView`, and
  a per-connection `ChainCapabilityVerifier::for_requester`.
- **Token before upgrade.** The verse-scoped token check (`auth::authenticate` and
  `require_current_credential`) runs before the WebSocket upgrade. The token's
  principal becomes the requester.
- **`leaf_certificate_id`.** Replaces the old stand-in.
- **`held` acks.** The connection now encodes them.
- **Epoch bumps.** A real epoch-bump source calls `invalidate_on_epoch_bump`, and a
  process-wide sweep runs alongside.
- **Router.** `router.rs` takes `Arc<CanonicalLogState>` and no longer takes an
  `Option`.

**Test:** extend the `canonical_ws` tests:
- no token;
- foreign audience;
- foreign verse (byte-identical to the existence oracle);
- revoked mid-session;
- `held` shape.

### A2 — local BI query route

**Owns:** `fe-api/src/warehouse_query.rs` (new). Tier: sonnet.

**Route:** `POST /api/v1/verses/{v}/warehouse/query`.
- The body is a structured spec, never SQL.
- Requires Viewer+, plus `query_guard`.
- Runs Q1 over D13.
- REST only.

**Test:** `warehouse_query::` module tests.

### A3 — warehouse settings and status REST routes

**Owns:** `fe-api/src/warehouse_settings_api.rs` (new). Tier: sonnet.
**Security review:** yes, because this is a governance write over REST.
**Decision:** D-CL49.

| Route | Gate | Behaviour |
| --- | --- | --- |
| `GET /api/v1/verses/{v}/warehouse/settings` | Viewer+ | Read the settings |
| `PUT /api/v1/verses/{v}/warehouse/settings` | Manager+ at verse scope | Sends `SetVerseWarehouseSettings` through the API command channel and returns the read-back echo |
| `GET /api/v1/verses/{v}/warehouse/status` | Viewer+ | Serves D5's `warehouse_status` |

- An unknown verse and a denied verse both get the uniform not-found.
- REST only; no MCP tool.

**Test:** `warehouse_settings_api::` module tests (role table).

### U1 — warehouse settings surface

**Owns:** `fe-ui/src/warehouse_settings.rs` (new),
`fe-ui/src/ui_shell/right_sidebar.rs`. Tier: sonnet.

**State.** A state resource using the `SettingDraft` pattern; it displays only the
echo.

**Warehouse section,** placed under Timeseries Fabric:
- mode toggle;
- custody radio;
- relay DID list;
- R shown read-only, with an "R needed, N listed" warning;
- prune toggle.

Non-Managers see the controls disabled, with an accessible reason. Apply
`C:/Users/atooz/.gemini/UI-UX-guidance.md`.

**Test:** `warehouse_settings::` draft and echo.

### G3 — wave-3 gate

**Owns:** `fe-database/src/migration/mod.rs`, `fe-sync/src/lib.rs`,
`fe-api/src/lib.rs`, `fe-api/src/canonical_ws/mod.rs`, `fe-ui/src/lib.rs` (mod
lines only). Serial.

**Steps:**
1. Add mod lines for `scene_write_tap`, `mappings`, `canonical_segments`,
   `warehouse_query`, `warehouse_settings_api`, `production` and
   `warehouse_settings`.
2. Run:
   `cargo check -p fe-database && cargo check -p fe-sync && cargo check -p fe-sync --features relay-server && cargo check -p fe-api && cargo check -p fe-ui`.

### Verification — wave 3 [checkpoint marker]

- `/security-review` (high) on A1, A3, N1, N2 and D7.
- `/code-review` on A2 and U1, plus a UI review against the UI-UX guidance.
- Record the verdicts.

## Wave 4 — mappings and the relay service

D8-D10 and D14-D16 each export their equivalence corpus rows from their own
module. D10 alone edits the harness file, so it consumes them.

### D8 — property and presentation mappings

**Owns:** `fe-database/src/migration/mappings/property.rs` (new),
`fe-database/src/migration/mappings/presentation.rs` (new). Tier: sonnet.

**Variants:** `SetNodeProperty`, `DeleteNodeProperty`, `ClearNodeProperties`,
`UpdateNodeUrl`, `SetNodeView`, `CommitStampOverrides` (one of the C-4 variants).

- Each is one kind-1 member, sealed at verse scope.
- Schema hashes follow the SPEC-10 §4.1 rule 3 convention.
- Hand the register rows to I8.

### D9 — node lifecycle mappings

**Owns:** `fe-database/src/migration/mappings/node_lifecycle.rs` (new).
Tier: sonnet.

**Variants:** `CreateNode`, `CreateNodeWithAsset`, `PlaceStoredGlb`, `CreateObject`
(C-4), `PromoteInstance`, `PromoteCheckedStamp` (C-4), `DeleteNode`,
`TombstoneNode`, `CascadeTombstoneNode`.

- Every create carries a `legacy_identifier` anchor.
- `CascadeTombstoneNode` is an atomic multi-member set.

### D10 — multi-row atomic mappings and harness corpus hook

**Owns:** `fe-database/src/migration/mappings/editor_batch.rs` (new),
`fe-database/tests/migration_equivalence_harness.rs`. Tier: opus.

**Variants:** `CommitCheckedEditorBatch`, `ImportPetalSnapshot` (C-4),
`CommitNodeMutation` (C-4), `CommitCheckedNodeMutation` (C-4). These are atomic
candidate sets.

**Harness:** add the table-driven per-kind corpus hook that every mapping slice
feeds. Each multi-row kind gets an all-or-nothing case: fail one member, assert
zero appended.

### D14 — ImportGltf mapping

**Owns:** `fe-database/src/migration/mappings/import_gltf.rs` (new). Tier: opus.
**Decision:** D-CL47.

**Contract:** S3's atomic contract. The node row and asset row form one candidate
set. The content hash is referenced, and the GLB bytes stay in the blob store.

**Unchanged behaviour:** a KTX2-refused import emits nothing, and the legacy
refusal is unchanged.

**Corpus rows:** one success, one KTX2 refusal, one atomicity failure.

### D15 — DuplicateNode mapping

**Owns:** `fe-database/src/migration/mappings/duplicate_node.rs` (new).
Tier: sonnet. **Decision:** D-CL47.

**Contract:** a new node plus its copied properties, as an atomic set that records
the source anchor.

**Corpus rows:** duplicate with properties; duplicate of a tombstoned source
(refused by legacy, so no candidate).

### D16 — RenameNode mapping

**Owns:** `fe-database/src/migration/mappings/rename_node.rs` (new). Tier: sonnet.
**Decision:** D-CL47.

**Contract:** a single kind-1 member with the NFC display name.

**Corpus rows:** rename; rename of a tombstoned node (legacy refusal, no
candidate).

### R1 — record-relay warehouse service

**Owns:** `fractalengine-relay/src/warehouse.rs` (new),
`fractalengine-relay/src/relay_serve.rs`, `fractalengine-relay/src/replicas.rs`.
Tier: opus. **Security review:** mandatory.

**Seed handling, in order:**
1. Accept the seed via N1 and DB ingest.
2. Re-hash and store.
3. Sign a receipt with the relay node key.
4. In keyholder mode, replay into the relay projection.

**Format epoch.** On a format-epoch mismatch, wipe and request a re-seed (D-CL38).

**Single verse.** `FE_RELAY_VERSE=<id>` is required, and only that replica opens.
Zero or several verses exits non-zero with a named reason. This replaces
`load_hosted_verse_ids` in record mode.

**Test:** extend the `relay_serve` tests and add `warehouse::` tests.

### G4 — wave-4 gate

**Owns:** `fe-database/src/migration/mappings/mod.rs` (mod lines for the six
mapping files), `fractalengine-relay/src/main.rs` (`mod warehouse;` only). Serial.

**Check:** `cargo check -p fe-database --all-targets && cargo check -p fractalengine-relay --features relay-server`

### Verification — wave 4 [checkpoint marker]

- `/code-review` (high) on D8-D10 and D14-D16, covering each mapping's
  equivalence argument and atomicity.
- `/security-review` on R1.
- Record the verdicts.

## Wave 5 — strictly serial integration

Run in exactly this order, one slice at a time:

I1 → I2 → I3 → I4 → I6 → I5 → C1 → I7 → QS1 → I8

### I1 — messages and channels

**Owns:** `fe-runtime/src/messages.rs`, `fe-runtime/src/app.rs`,
`fe-runtime/src/channels.rs`, `fe-sync/src/messages.rs`. Tier: sonnet.

**New `DbCommand` variants:**
- `SetVerseWarehouseSettings`
- `GetVerseWarehouseSettings`
- `GetWarehouseStatus`
- `SubmitCanonicalCandidate`
- `IngestCanonicalSegment`
- `RecordCustodyReceipt`
- `PackClosedSegments`
- `PruneBehindCheckpoint`

**Moved:** remove `ExecuteTsPartial` and add a `TsPartialRequest` channel.

**New `DbResult` variants:** `VerseWarehouseSettingsSet` (read-back) and
`CustodyFloorChanged`.

**New `SyncCommand` variants:** `SeedSegments`, `FetchSegment`,
`PublishCustodyReceipt`.

**Reply families:** follow `fe-runtime` §api-reply-correlation.

**Check:** `cargo check -p fe-runtime && cargo check -p fe-sync`

### I2 — DB dispatch

**Owns:** `fe-database/src/lib.rs`, `fe-database/src/command_lane.rs`. Tier: opus.

- Route every census arm through `dual_emit_scene_write`, which becomes the only
  path.
- Add the new command arms.
- Delete the `ExecuteTsPartial` arm (~2304-2335).
- Spawn the D12 worker beside the DB thread.
- Classify the new commands in `command_lane`.

**Check:** `cargo check -p fe-database --all-targets`. **Review:** `/code-review`
high.

### I3 — sync wiring

**Owns:** `fe-sync/src/sync_thread.rs`, `fe-sync/src/distributed_query.rs`,
`fe-test-harness/src/peer.rs`. Tier: sonnet. **Security review:** yes, because it
wires inbound traffic.

- Handle the new sync commands via N1.
- Feed `CanonicalAnnounce` frames into ingest and receipt recording.
- Auto-enrol a device when a warehouse verse opens.
- Route partials to the reader channel; update the harness peer to match.

**Check:** `cargo check -p fe-sync --all-targets && cargo check -p fractalengine-test-harness`

### I4 — API wiring

**Owns:** `fe-api/src/server.rs`, `fe-api/src/lib.rs`. Tier: sonnet.
**Security review:** yes.

- Make the mount mandatory:
  `build_router_with_canonical_log(state, production_canonical_state(...))`.
- Mount the A2 and A3 routes.
- Do not touch `fe-api/src/mcp/*` and do not add MCP tools.

**Check:** `cargo check -p fe-api --all-targets`

### I6 — UI wiring

**Owns:** `fe-ui/src/plugin.rs`, `fe-ui/src/verse_manager/db_results/mod.rs`.
Tier: sonnet.

- Register U1's resource.
- Fold the echo and the hierarchy columns into an existing `SystemParam` bundle,
  to stay within the 16-param ceiling.

**Check:** `cargo check -p fe-ui`

### I5 — binaries

**Owns:** `fractalengine/src/main.rs`, `fractalengine/src/relay_serve_bridge.rs`,
`fractalengine-relay/src/main.rs`. Tier: sonnet.

- Print a startup diagnostic of the active surfaces.
- Start the pack/seed timer for warehouse verses.
- Desktop relay mode stays unchanged (D-CL48).
- Add relay record mode via `FE_RELAY_VERSE`, keeping the
  gate-before-sync-thread ordering.

**Check:** `cargo check -p fractalengine && cargo check -p fractalengine-relay --features relay-server`

### C1 — fe CLI warehouse verbs

**Owns:** `tools/fe-cli/src/catalog.rs`, `tools/fe-cli/tests/cli_journey.rs`.
Tier: sonnet. **Decision:** D-CL49.

- **Rows.** Add phase-2 rows in `catalog::VERBS` for `fe warehouse get`,
  `fe warehouse set` and `fe warehouse status`, each mapped to the A3 REST routes
  and dispatched by the existing `commands/generic.rs`.
- **Shaping.** Only if argument shaping cannot be expressed as a row, add a new
  `tools/fe-cli/src/commands/warehouse.rs` and register it. That registration is a
  stop-and-report, because `commands/mod.rs` and `generic.rs` stay unowned.
- **Phases.** Respect `SHIPPED_PHASE`.
- **Rules.** Loopback only, server-side role checks, and the existing exit table.
  No MCP tool, so `catalog/mcp-tools.json` stays unchanged and the drift leg stays
  green.
- **Journey leg** in `cli_journey.rs`:
  1. a viewer `set` exits 4;
  2. the Manager's `set` returns the read-back;
  3. `status --json` carries the floor fields.

This slice runs after I4 because the journey builds the real router.

**Check:** `cargo check -p fe-cli --all-targets`

### I7 — harness scenario, enforcement census, close greps

**Owns:** `fe-test-harness/src/scenarios/relay_record_warehouse.rs` (new),
`fe-test-harness/src/scenarios/mod.rs`, `fe-test-harness/tests/integration.rs`,
`conductor/tracks/relay_record_warehouse_20261120/enforcement-census.md` (new).
Tier: opus.

**Scenario.** Peers A and B, two record relays, R=2:
1. A writes, including a glTF import, a duplicate and a rename.
2. Both relays return receipts, so the floor is met.
3. A goes offline.
4. B bootstraps from the relays; its projection root equals A's.
5. A forged segment is refused.
6. A relay started with two verses refuses to start.

**Census.** List every `fn (assert|validate|verify|check|require|may|decide)_\w+`
in the touched trees and classify it as production, test-by-design or dormant.
The gate is **0 dormant**.

**Close greps:**
- one production `build_router_with_canonical_log` caller per binary;
- every census variant is in `scene_write_tap.rs`, and there are 0 `Unsupported`
  arms;
- `execute_ts_partial` is called only from `ts_partial_reader.rs`;
- every `compaction_decision` caller passes floor evidence;
- `seal_artifact` is the only seal path;
- `record_bypass_for` is called only for the three hierarchy-creation kinds.

**Check:** `cargo check -p fractalengine-test-harness --all-targets`

### QS1 — simulator relay-record scenario

**Owns:**
- `tools/qa/sim/scenarios/relay-record.json` (new)
- `tools/qa/sim/driver.py`
- `tools/qa/sim/run.py`
- `tools/qa/sim/prober.py`
- `tools/qa/sim/scenario.py`
- `tools/qa/sim/scenario.schema.json` (regenerated only by
  `python tools/qa/sim/run.py schema`)
- `tools/qa/stage.ps1`

Tier: sonnet. **Decision:** D-CL49.

- **`stage.ps1`.** Also builds `fractalengine-relay` with `--features
  relay-server`, as its own serial Cargo call (C-13).
- **`run.py`.** Starts and stops one record relay by exact process identity, with
  `FE_RELAY_VERSE` set to the fixture verse. It stops the relay in `finally`.
- **`driver.py`.** New host ops `start_relay` and `stop_relay`, plus REST ops
  `warehouse_set` and `warehouse_status`.
- **`prober.py`.** A `warehouse` probe that reads `warehouse/status` (floor and
  projection root).
- **Scenario steps:**
  1. enable warehouse with the relay and R=1;
  2. a mixed write set, including glTF import, duplicate and rename;
  3. wait for the floor;
  4. record the author's root and fingerprint;
  5. `stop_peer` the author;
  6. bootstrap a fresh peer from the relay (`restart` with `no_bootstrap` plus
     join);
  7. expect equal root and fingerprint.
- **Running it.** Only when the owner names `relay-record`.

**Check:** `python tools/qa/sim/run.py validate relay-record` (structural; does
not launch the app).

### I8 — docs and bookkeeping

**Owns:** the AGENTS.md set listed under "Docs owed", `conductor/test-batches.md`
(append only), `conductor/doneness-ledger.md`, `conductor/tracks.md`, and the
CDL-PROV register rows in `fe-canonical-log/src/AGENTS.md`. Tier: haiku. No check.

**Coordination.** Those files are owned elsewhere until they are released, so
coordinate with their owners first. Never touch `conductor/RUNBOOK.md`.

### Verification — wave 5 [checkpoint marker]

- `/security-review` (high) of the composed system.
- A critic checks the result against `conductor/product.md` and
  D-CL35..D-CL49 verbatim.
- The owner then names `RW`, `R9-CLI` and `Harness`, plus optionally the
  simulator `relay-record` run, for the single test sweep.

## Docs owed (I8)

| Doc | Changes |
| --- | --- |
| `fe-canonical-log/src/AGENTS.md` | §trait-composition-map rows; Provisional wire numbering rows: held, custody receipt, every mapping schema from D8-D10 and D14-D16 |
| `fe-canonical-log/src/{segment,retention,wire}/AGENTS.md` | Matching notes |
| `fe-api/src/canonical_ws/AGENTS.md` | §mount and §auth replace §deliberately-unmounted |
| `fe-api/AGENTS.md` | Warehouse REST routes, REST only |
| `fe-database/src/canon_log/AGENTS.md` | Rewrite §parallel-and-dormant (C-12); add the new sections |
| `fe-database/src/migration/AGENTS.md` | §boundary, §no-shadow-exposure, §scene-write-tap, §bypass-counter (three kinds left) |
| `fe-database/src/AGENTS.md` | §ts-partials, §warehouse-settings |
| `fe-sync/src/AGENTS.md` | §canonical-segments, §gossip-signing, §relay-advertise, §peer-relay-server |
| `fe-runtime/src/AGENTS.md` | Matching notes |
| `fe-ui/src/ui_shell/AGENTS.md` | §warehouse-settings |
| `fractalengine-relay/AGENTS.md` | Record mode |
| `tools/fe-cli/AGENTS.md` | `warehouse` verbs in §phase-2 |
| `tools/qa/sim/AGENTS.md` | `relay-record` in the scenario list; relay host ops |
| `tools/qa/AGENTS.md` | §sim relay staging |
| `conductor/test-batches.md` | Append batch `RW`; add `device_enrolment_test` and `dcl20_bypass_counter_test` remain in R10 |
| `conductor/doneness-ledger.md` | Close CDL-NET, CANON-WS, SYNC-SIGNED-PLANE (remainder), CDL-WRAP-DELIVERY; CANON-DUAL-EMIT "wired, cutover gated" |

## New test batch `RW` (I8 appends it)

```bash
cargo test -p fe-canonical-log --test spec7_conformance --test cross_spec_integration
cargo test -p fe-canonical-log --lib -- wire::commit wire::session segment::custody_receipt retention::durability_floor checkpoint::
cargo test -p fe-runtime --lib -- warehouse::
cargo test -p fe-query --features datafusion --lib -- columnar::observation::scan
cargo test -p fe-database --lib -- canon_log:: migration:: handlers::warehouse_settings handlers::ts_partial ts_partial_reader
cargo test -p fe-database --test canon_log_composed_admission_test --test migration_conformance_test --test migration_equivalence_harness --test device_enrolment_test --test dcl20_bypass_counter_test
cargo test -p fe-sync --lib -- canonical_segments gossip_auth relay_config
cargo test -p fe-sync --features relay-server --lib -- relay_server
cargo test -p fe-api --lib -- canonical_ws:: warehouse_query warehouse_settings_api
cargo test -p fractalengine-relay --features relay-server -- relay_serve warehouse
cargo test -p fe-ui --lib -- warehouse_settings
cargo test -p fe-cli --test cli_journey
cargo test -p fractalengine-test-harness -- relay_record_warehouse
python tools/qa/sim/run.py validate relay-record
# simulator run: only when the owner names it (launches peers + relay)
python tools/qa/sim/run.py relay-record
```

Run `RW` only through `tools\qa\test_batches.ps1 -Batches RW` when it is named. The
simulator line runs only when the owner names `relay-record`. Never run
`cargo test --workspace`.

## Mandatory security-review points

| Point | Slices |
| --- | --- |
| Auth (token then chain; existence oracle; REST governance write) | A1, A3, I4 |
| Crypto (receipt domain, wrap sealing, signing defaults) | P2, D6, N2 |
| Key custody (issuance root D-CL46, keyholder relay, epoch bump) | D1, D6, R1 |
| Inbound P2P admission (segment ingest, transport caps, quarantine) | D3, N1, I3 |
| Authoring path (tap, lifted D-CL20 kinds) | D7 |

## Risks

1. **I2 rewrites every scene-write arm.** Mitigation: the exhaustive tap, plus the
   byte-identical-legacy test.
2. **Signing defaults flip to `require`.** Older peers can no longer interoperate.
   This is pre-GA and accepted (D-CL38).
3. **Issuance is rooted in the role table (D-CL46).** Re-anchoring it once SPEC-2
   authority history exists will be a breaking change.
4. **fe-database rebuilds pull in surrealdb-core.** Use one builder and prune PDBs.
5. **Multi-row mappings (D10, D14) may run past one slice.** If a kind truly cannot
   be made atomic, the escalation is an explicit `Unsupported` that blocks closure
   and goes to the owner. Never force it.
6. **QS1 needs the relay binary staged.** `stage.ps1` gains a serial relay build.
   A stage made before QS1 has no relay, and the scenario must refuse with a named
   error.
