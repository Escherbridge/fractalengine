---
type: plan
track: canonical_data_log_20260808
created: 2026-10-13
title: CDL Workstream G Wave 4 — serial integration campaign plan (CDL-W4 + CDL-R2 remainder)
status: proposed
resource: ./metadata.json
ledger_rows: [CDL-W4, CDL-R2]
supersedes_brief: workstream-g-plan.json waves[3] W4-integration (brief written 2026-08-08; reality has moved, see §2)
---

# CDL Wave 4 — serial integration campaign plan

Planner output, 2026-10-13. Read-only recon at HEAD `877e801`; no cargo was run. Every claim
below cites the file:line it was checked against. Slices run **strictly sequentially**; each
executor runs `cargo check` + `cargo clippy -D warnings` on its own crates only. Tests are
**written** in each slice and **run once** at campaign end (user standing rule, RUNBOOK §3
"Working rules").

## 1. Goal and acceptance

**Goal.** Finish the wiring the parallel Wave 1-3 leaves deliberately deferred, so every
canonical-log gate has a production implementor and a production (library) caller, and give
the owner a production `DeviceEnrolmentView` under the 2026-10-13 R2 decision — all strictly
LOCAL. Then build the equivalence-evidence harness that the owner needs before deciding
CANON-DUAL-EMIT.

**Sources of acceptance.**

| Source | Acceptance it contributes |
| --- | --- |
| Ledger `CDL-W4` (`conductor/doneness-ledger.md:100`) | six strictly-additive `record_bypass` calls with a test asserting each counter increments and no extra OpLogEntry is built; composed CandidateVerifier / CheckpointView / ScopeKeyResolver / SealedArtifactStore / CapabilityVerifier; `cross_spec_integration.rs` green without DB or network; **0 NO_CALLER gates**; `/security-review` verdict recorded in the track |
| Ledger `CDL-R2` (`doneness-ledger.md:102`) | DB-backed views; wrap to an unenrolled device refused, non-Manager issuer refused, enrolled+authorized succeeds; `crypto/AGENTS.md` row "Not built" → "Structural" |
| RUNBOOK §3 2026-10-13 **R2** (`conductor/RUNBOOK.md:166-169`) | enrolment = **self-signed, replicated**: the principal's ed25519 key signs `{principal, device X25519 key, enrolled_at}`; only that principal may write it; replicates as an ordinary row; revocation = signed tombstone |
| `workstream-g-plan.json` waves[3] acceptance | `cross_spec_integration.rs` passes; crud.rs diff = exactly six added lines; `is_available()` untouched on the canonical path; no binary references `build_router_with_canonical_log`; one consolidated provisional-wire index |
| Track spec hard limits (`spec.md` "Status and boundary") + task hard limits | canonical-log network exposure stays NO_GO: no `/ws/canonical` mount, no relay seeding, no inbound P2P on the canonical path; CANON-DUAL-EMIT stays unwired; CDL-ROTATION-ASSEMBLY stays unscheduled |
| Track `spec.md` "Required outcomes" | one serial integrated workspace test/lint/format sweep passes |

**Campaign-end gate (run once, after slice 9):** `cargo test --workspace`;
`cargo +1.99.0 clippy --workspace --all-targets --keep-going -- -D warnings`;
`cargo fmt --check -p <touched crates>`; `node --test docs/spec/canonical-log/operation-envelope-v1.test.mjs`;
`cargo run -p fractalengine-test-harness` (enrolment rows ride the real row path); greps:
`build_router_with_canonical_log` has zero non-`#[cfg(test)]` callers outside `fe-api/src/canonical_ws/`,
`commit_operation_dual_emit` has zero callers under `fe-database/src/handlers/`.

## 2. Current-code reality (what the 2026-08-08 brief no longer matches)

Done already (do NOT redo):

- `compose.rs` is not a placeholder. It owns the single `QuarantineReason` vocabulary
  (`fe-canonical-log/src/compose.rs:23`), the budget-class mapping (`:150`), the fallible
  SPEC-5→SPEC-4 binding `checkpoint_binding_for_selection` (`:219`), and the **CheckpointView**
  composition `SignedCheckpointProofView` (`:256`, impl `:282`). Brief item 1a is done.
- **ScopeKeyResolver** has a production implementor: `LaneScopeKeyResolver`
  (`fe-canonical-log/src/crypto/scope_key.rs:138`, impl `:167`). Brief item 1b is done.
- The **compaction gate** is real: `checkpoint::compaction_decision` (`checkpoint.rs:651`) takes
  a real `CheckpointVerification`; the old `CheckpointVerdict` stand-in is deleted
  (`fe-canonical-log/src/AGENTS.md:209-221`). Brief item 1f is done.
- The **single provisional-wire register** exists: `fe-canonical-log/src/AGENTS.md:258`
  ("This section is the single ratification surface"). Brief item 2's index is done; W4 only
  appends new rows to it.
- `IssuerAuthorityView` has a production implementor, `DurableAuthorizationView`
  (`fe-database/src/canon_log/canonical_epoch.rs:630`); it also implements `AuthorizationView`
  (`:584`) and `ManagerAuthorityView` (`:594`). CDL-R2's issuer half is done.
- `VerifiedEnvelopeMeta` is sealed (`#[non_exhaustive]`, `materialize/traits.rs:48`, compile_fail
  doctest `:32-46`) — CDL-R1 closed.
- `AppendError::StorageUnavailable` exists (`materialize/errors.rs:153`) and the Surreal store
  uses it (`fe-database/src/canon_log/append_store.rs:441`).

Still missing (this plan's scope):

| Gap | Evidence |
| --- | --- |
| No production **CandidateVerifier** | only `FixedVerifier` (`materialize/traits.rs:401`), the `PreconditionGuardedVerifier` wrapper (`fe-database/src/canon_log/admission.rs:247`) and test doubles (`fe-database/tests/canon_log_materialization_test.rs:230`) |
| The trait cannot be implemented honestly | `verify_authorization(&self, meta)` / `verify_payload(&self, meta)` (`traits.rs:214-220`) receive only `VerifiedEnvelopeMeta`, which carries no `CapabilityRef` and no `PayloadRef` (`traits.rs:49-68`; the envelope has both, `envelope.rs:625,635`) and no payload ciphertext — the chain cannot be found and the AAD cannot be rebuilt |
| `AdmissionOutcome` has drifted from SPEC-4 §5 | §5 has an `unknown_kind` row (erratum G1, `docs/spec/canonical-log/log-first-materialization.md:158`); `AdmissionOutcome` has no `UnknownKind` variant (`materialize/errors.rs:12-66`). Nor can it express SPEC-2's "missing capability chain or epoch state remains quarantined" (`author-key-lifecycle.md:240-241`) |
| No production **wire CapabilityVerifier** | only `MockCapabilityVerifier` (`wire/test_support.rs:391`) and an fe-api test double (`fe-api/src/canonical_ws/handler.rs:2343`). The trait (`wire/session.rs:247-257`) carries no authenticated requester, while SPEC-3 §2.4 step 6 forbids exercising a chain "merely because it can fetch the chain artifact" (`capabilities-and-revocation.md:141-143`) |
| **SealedArtifactStore** only in-memory | `InMemorySealedArtifactStore` (`segment/store.rs:66`); no injected-byte-store adapter |
| **No production `CanonicalCommitPipeline`** | only scripted/mock (`wire/test_support.rs:185,227`) |
| **No production `DeviceEnrolmentView`** | trait `crypto/key_wrap.rs:143`; implementors are test-only (`key_wrap.rs:817`, `canonical_epoch.rs:852`); row "Not built — blocked on an owner decision" (`crypto/AGENTS.md:29`) — the decision is now R2 |
| **No production `ManagerPlusAuthorizationView`** | trait `checkpoint.rs:327`; `AdmittedCheckpoint::admit` takes one (`fe-database/src/canon_log/rebuild.rs:76`); only a test fixture implements it (`canon_log_materialization_test.rs:122`) |
| **No DeliveryAuthorizationView** production impl | trait `key_wrap.rs:153`; `issue_scope_key_wrap` consults it (`key_wrap.rs:602-622`) |
| **SPEC-2 admission entrypoints have no production caller** | `admit_rotation`/`admit_disavow`/`admit_disavow_rescind`/`admit_continuity_grant` (`author_key/admission.rs:348,407,439,488`) — grep finds no caller outside the module; `LineageIndex::active_key_at` (`author_key/lineage.rs:211`) likewise |
| Two **ScopeKeyStore** traits | `crypto::scope_key::ScopeKeyStore` (`crypto/scope_key.rs:26`) and `retention::crypto_shred::ScopeKeyStore` (`retention/crypto_shred.rs:52`) — the duplicated-vocabulary shape `src/AGENTS.md` §unified-vocabularies forbids |
| Materializer identity is caller-mapped only | `BindingSelection` carries both `materializer_id: Identifier32` and `MaterializerVersion` (`compose.rs:196-208`); no type derives one from the other (metadata G-WAVE-4: "CheckpointClaimV1 materializer identity type" unbuilt) |
| **D-CL20 bypass counters** absent from the six handlers | `record_bypass()` is called only at `fe-database/src/migration/boundary.rs:141`; it is one global counter with no kind (`migration/mapping_inventory.rs:204-211`). Handlers: `create_verse_handler` `crud.rs:22`, `create_fractal_handler` `:87`, `create_petal_handler` `:130`, `import_gltf_handler` `:359`, `rename_node_handler` `:1609`, `duplicate_node_handler` `:1676` (the brief's line numbers are stale) |
| `lib.rs` has no curated surface | bare module list (`fe-canonical-log/src/lib.rs:5-22`); module-ownership table still says `compose` = "Wave 3 integration" (`src/AGENTS.md:179`) |
| Stale docs | `fe-database/src/canon_log/AGENTS.md:94-110` still says `AppendError` has two variants and faults map to `HashMismatch`; `materialize/traits.rs:236-247` still calls `StorageUnavailable` a filed erratum; `fe-database/src/AGENTS.md` has no `§canon-log-materialization` (sections at `:6-620`) |
| **No equivalence evidence path** | every kind is `UnmappedDeferred` (`mapping_inventory.rs:107-126`); `MeasurementReport::blocks_promotion` is permanently true (`migration/report.rs:156`); `commit_operation_dual_emit` has no live caller (`migration/AGENTS.md:166-170`) |

Ground truth on the hard limits: `build_router_with_canonical_log` (`fe-api/src/canonical_ws/router.rs:18`)
is only called from its own tests (`router.rs:135,171`); the row-replication path is
`replicate_row` (`fe-database/src/lib.rs:155`) → `ReplicationEvent` → `SyncCommand::WriteRowEntry`
(`fe-sync/src/messages.rs:44`); inbound rows are gated by `admit_inbound_row`
(`fe-database/src/handlers/replicated_row.rs:261-314`) and dispatched by `apply_row_mode`
(`:177-207`), which returns `NotApplicable` for unknown tables (`:201-206`).

## 3. Slices (strictly sequential, one executor each)

Model tier per user standing rules: sonnet for standard, opus where the slice is a security
boundary or protocol composition. "Crates" = what the executor `cargo check`s + clippys.

---

### Slice W4-1 — D-CL20 per-kind bypass counters (S, sonnet)

**Owned files:** `fe-database/src/handlers/crud.rs` (six lines only),
`fe-database/src/migration/mapping_inventory.rs`, `fe-database/src/migration/AGENTS.md`
(§bypass-counter), NEW `fe-database/tests/dcl20_bypass_counter_test.rs`.
**Crates:** fe-database.

**Steps.**
1. In `mapping_inventory.rs` add `pub fn record_bypass_for(kind: &'static str)` and
   `pub fn bypass_count_for(kind: &str) -> u64`: a fixed `[AtomicU64; 6]` indexed by the
   position in `D_CL20_MUTATION_KINDS` (`:31-38`), plus the existing global
   `UNMAPPED_BYPASS_COUNT` incremented in the same call so `bypass_count()` (`:209`) and
   `MeasurementReport` (`report.rs:64`) keep their meaning. `record_bypass_for` with a kind
   outside the six `debug_assert!`s and increments only the global counter. `boundary.rs:141`
   keeps calling `record_bypass()` unchanged.
2. Add exactly one line as the FIRST statement of each of the six handlers
   (`crud.rs:22,87,130,359,1609,1676` — re-grep before editing):
   `crate::migration::mapping_inventory::record_bypass_for("CreateVerse");` etc., using the
   exact strings of `D_CL20_MUTATION_KINDS`.

**Invariants.** `migration/AGENTS.md` §bypass-counter (Relaxed AtomicU64, accessor read) and
§boundary (no dual-emit wiring); migration-plan §4.2.5 / D-CL20: no `OpLogEntry`, no
`commit_operation_dual_emit`, no invented canonical payload; no control-flow, return-type or
error-text change (`git diff crud.rs` must show exactly 6 `+` lines and 0 `-` lines).
`fe-database/src/AGENTS.md` §log-first-commit untouched.

**Flow tests** (`dcl20_bypass_counter_test.rs`, own binary so the process-global counters are
not shared; ONE `#[tokio::test]` driving all six sequentially through the real DB thread +
`DbCommand` API, harness pattern of `fe-database/tests/node_lifecycle_test.rs:30-60`):
- `each_dcl20_mutation_records_exactly_one_bypass_of_its_own_kind`: CreateVerse → CreateFractal
  → CreatePetal → ImportGltf (fixture GLB) → RenameNode → DuplicateNode; after each, assert the
  matching `bypass_count_for` rose by exactly 1 and the other five did not move; assert the
  `DbResult` is the same success variant the pre-change handler returns, and that the `op_log`
  row count delta per command equals the legacy handler's own delta (read before/after) — i.e.
  the counter added no op-log row. Mutation check: deleting any one line turns its assertion red.
- One unhappy path kept (reachable, consequential): a RenameNode on a tombstoned node still
  records the bypass (the call precedes the lookup) and still returns the original error text —
  proves "first statement" and "error text unchanged" together.

---

### Slice W4-2 — Admission vocabulary + candidate shape alignment (L, opus)

**Owned files:** `fe-canonical-log/src/materialize/traits.rs`,
`fe-canonical-log/src/materialize/errors.rs`, `fe-canonical-log/src/materialize/mod.rs`,
`fe-canonical-log/src/materialize/AGENTS.md`, `fe-database/src/canon_log/admission.rs`
(signature alignment only), `fe-database/tests/canon_log_materialization_test.rs` (alignment of
`FixtureVerifier` only).
**Crates:** fe-canonical-log, fe-database.

**Steps.**
1. Add `#[non_exhaustive] pub struct VerifiedCandidate { pub meta: VerifiedEnvelopeMeta, pub envelope: CompleteEnvelope }`
   whose only constructor is `VerifiedCandidate::verify_from_bytes` (wraps
   `verify_and_project_envelope`, `traits.rs:98-104`). Add a compile_fail doctest mirroring the
   meta one.
2. Change `CandidateVerifier` (`traits.rs:204-221`): `verify_envelope` returns
   `Result<VerifiedCandidate, AdmissionOutcome>`; `verify_authorization(&self, &VerifiedCandidate)`;
   `verify_payload(&self, &VerifiedCandidate, payload_ciphertext: Option<&[u8]>)`.
   `admit_candidate` (`:331-363`) gains `payload_ciphertext: Option<&[u8]>` and still returns
   `VerifiedEnvelopeMeta` so `fe-database` callers keep their types.
3. `AdmissionOutcome` (`errors.rs:12`): add `UnknownKind { operation_kind: u16 }` (Quarantine —
   SPEC-4 §5 row, erratum G1) and `AuthorizationEvidenceUnavailable { reason: QuarantineReason }`
   (Quarantine — SPEC-2 `author-key-lifecycle.md:240-241`; constructor refuses any reason other
   than `CapabilityChainUnavailable` / `EpochStateUnavailable` / `PayloadKeyUnavailable`).
   `disposition()` stays a total match with no wildcard.
4. Re-align `PreconditionGuardedVerifier` (`fe-database/src/canon_log/admission.rs:247-280`) and
   the two test doubles; `admit_and_append` (`admission.rs:174`) gains the ciphertext parameter
   and passes it through. No behavior change to the precondition ordering.
5. Fix the stale doc comment `traits.rs:236-247` (StorageUnavailable has landed).

**Invariants.** `materialize/AGENTS.md` sealing (possession of `VerifiedCandidate` = proof
`decode_and_admit` ran); `src/AGENTS.md` §ingress (one door for peer bytes);
`canon_log/AGENTS.md` §admission-order (precondition before append; validate-then-log);
§unified-vocabularies (no second quarantine enum — the new variant WRAPS `QuarantineReason`).

**Flow tests** (in `traits.rs` tests + `canon_log_materialization_test.rs`):
- `an_unknown_kind_candidate_is_quarantined_not_rejected`: a validly signed envelope with an
  unregistered kind → `admit_candidate` returns `UnknownKind`, disposition Quarantine, nothing
  appended (fe-database: `verified_op_log` row count unchanged).
- `a_candidate_whose_chain_is_unavailable_is_quarantined_and_admits_once_it_arrives`: verifier
  stub answering `AuthorizationEvidenceUnavailable` first, then Ok → second `admit_and_append`
  appends exactly one row (real Surreal store).
- Table-driven `disposition` test over every `AdmissionOutcome` variant (pure logic with real
  branching — the one allowed unit table).

---

### Slice W4-3 — Composed verifiers (CandidateVerifier + wire CapabilityVerifier + materializer identity) (L, opus)

**Owned files:** `fe-canonical-log/src/compose.rs` (new items only; existing items untouched),
NEW `fe-canonical-log/src/compose/` is NOT created — keep one file unless it passes ~1200 lines,
then split into `compose/{mod,verifier,wire,identity}.rs` inside this slice;
`fe-canonical-log/src/materialize/identity.rs`; `fe-canonical-log/src/AGENTS.md`
(§unified-vocabularies additions + register rows).
**Crates:** fe-canonical-log.

**Steps.**
1. Seams (pure traits, sync, `Send + Sync`): `CapabilityChainSource { fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>> }`;
   `PayloadSchemaRegistry { fn validate(&self, schema_hash, operation_kind, plaintext) -> SchemaVerdict /* Valid | Invalid | UnknownSchema | UnknownKind */ }`;
   `PayloadCiphertextSource { fn ciphertext(&self, hash: &Hash32) -> Option<Vec<u8>> }`;
   `AuthorKeyLifecycleView { fn key_state(&self, author: &[u8;32], scope: &Scope, parents: &[Hash32]) -> KeyState }`;
   `Clock { fn now_ms(&self) -> u64 }`. All three-state where an answer can be missing; a
   missing answer is never an allow.
2. `ComposedCandidateVerifier<…>` implementing `CandidateVerifier`:
   - `verify_envelope` = `VerifiedCandidate::verify_from_bytes` + `kind::OperationKind::from_u16`
     → unknown kind maps to `UnknownKind` (NOT `InvalidEnvelope`).
   - `verify_authorization` = chain bytes by `envelope.unsigned.capability.chain_id`
     (absent → `AuthorizationEvidenceUnavailable(CapabilityChainUnavailable)`) →
     `capability::verify_chain_bytes` (`capability/chain.rs:551`) with
     `AuthorizationRequest { verb: Append, object_class: Op, operation_author = requester = envelope author, operation_scope_epoch, operation_kind, schema_hash, payload_bytes, now_ms: clock }`,
     `VerificationOptions::live()`; epoch-scope unknown → `EpochStateUnavailable`; any other
     `ChainError` → `UnauthorizedOperation`. Then `AuthorKeyLifecycleView::key_state` at the
     candidate's parents: `Retired`/`ForkedUnresolved` → `UnauthorizedOperation`
     (SPEC-2 rules 4, 6, 7 — executor must cite the exact §4 text for `Unknown`; default:
     `Unknown` = un-rotated key, authority rests on the chain).
   - `verify_payload`: no-payload form must have no ciphertext; otherwise ciphertext from the
     argument or `PayloadCiphertextSource` (absent → `OpaquePayload`); length + BLAKE3 vs
     `PayloadRef` → `InvalidPayload`; key via `ScopeKeyStore` for `encryption.key_id` (absent →
     `AuthorizationEvidenceUnavailable(PayloadKeyUnavailable)`); open with
     `crypto::aead::open` over `payload_aad::payload_aad_preimage(&envelope.unsigned)`
     (`payload_aad.rs:58`) → `InvalidPayload` on failure; then `PayloadSchemaRegistry`.
3. `ChainCapabilityVerifier<V: ManagerAuthorityView, C: Clock>` implementing the wire
   `CapabilityVerifier`, **constructed per authenticated connection** with the requester
   `Principal` bound at construction (`ChainCapabilityVerifier::for_requester(requester, view, clock)`).
   The requester never comes from the chain. Maps `VerbBits`/`ObjectClassBits` to exactly one
   `Verb`/`ObjectClass` (more than one bit set → `WireError`), returns `VerifiedAuthorization`
   from `VerifiedCapability` (`chain.rs:513`).
4. `materialize/identity.rs`: `MaterializerVersion::registry_id(&self) -> Identifier32` =
   BLAKE3 derive-key under a NUL-terminated provisional domain `fe-materializer-id-v1\0` over the
   canonical CBOR `{0: name, 1: version}`; `checkpoint_binding_for_selection` additionally
   refuses a `BindingSelection` whose `materializer_id != materializer_version.registry_id()`
   (new `CompositionError::MaterializerIdentityUnbound`). Add the domain and map to the
   §Provisional register.
5. `AuthorKeyLifecycleView` reference implementor `CausalLifecycleView<'a, C: CausalOperationView>`
   over `LineageIndex` + `DisavowIndex`, and a `LifecycleDispatcher` that routes a verified
   candidate of a SPEC-2 lifecycle kind to `admit_rotation` / `admit_disavow` /
   `admit_disavow_rescind` / `admit_continuity_grant` (`author_key/admission.rs:348-488`) with the
   decrypted payload as `PayloadAvailability::Available` — giving all four a production caller.

**Invariants.** `capability/AGENTS.md` §possession-is-never-authority and
§step-3-asks-the-view-itself (the verifier asks the view; no precomputed answers);
`crypto/AGENTS.md` §aead (only `aead::open` reaches the cipher; production suite asserted);
`author_key/AGENTS.md` "Every check has a caller" + "Causal resolution, never arrival order" +
"§4.6 rotation forks are detected, never resolved" (no resolver, no tie-break);
`src/AGENTS.md` forbidden dependencies (no fe-identity/fe-database/fe-runtime edge).

**Flow tests** (crate unit tests in `compose.rs`; the end-to-end versions live in W4-5):
- `a_signed_operation_with_a_live_chain_and_a_decryptable_payload_is_admitted`.
- `the_same_operation_is_refused_after_its_scope_epoch_is_bumped` (view epoch advanced).
- `an_operation_by_a_retired_author_key_is_unauthorized` (rotation admitted through
  `LifecycleDispatcher` first — proves the dispatcher feeds the view).
- `a_wire_authorize_cannot_borrow_someone_elses_chain`: `ChainCapabilityVerifier` built for
  principal B is handed A's valid chain → refused (SPEC-3 §2.4 step 6). Mutation check: binding
  requester to the leaf audience turns it green-for-the-wrong-reason → red.
- `a_claim_naming_an_unbound_materializer_identifier_is_refused`.

---

### Slice W4-4 — Storage-side compositions, vocabulary unification, crate surface (M, sonnet)

**Owned files:** `fe-canonical-log/src/compose.rs` (new items), `fe-canonical-log/src/retention/crypto_shred.rs`,
`fe-canonical-log/src/crypto/scope_key.rs`, `fe-canonical-log/src/lib.rs`,
`fe-canonical-log/src/AGENTS.md` (§module-ownership, new §crate-surface, §trait-composition-map),
`fe-canonical-log/src/retention/AGENTS.md`, `fe-canonical-log/src/crypto/AGENTS.md` (pointer only).
**Crates:** fe-canonical-log.

**Steps.**
1. `ByteStoreSealedArtifactStore<B: ContentByteStore>` implementing `SealedArtifactStore`
   (`segment/store.rs:15`) over an injected byte store; `put_verified` recomputes the id
   (`verify_artifact_id`) and runs `assert_immutable_write` BEFORE delegating; also implements
   `ArtifactLookup` exactly like `InMemorySealedArtifactStore` does (`segment/hashseq.rs:265`).
   Document: a production `ContentByteStore` MUST be local-only and MUST NOT be the iroh blob
   store (iroh blobs are served to peers — that is canonical network exposure).
2. `SealedProofEnvironment<'a, A: ArtifactLookup, K: ScopeKeyStore>` implementing
   `ProofEnvironment` (`segment/checkpoint_proof.rs:60`) by fetching verified bytes and opening
   them through the existing per-lane open functions with keys resolved via
   `LaneScopeKeyResolver`. (Pure composition; no segment producer is added.)
3. `VerifyingCommitPipeline<V, S, E, P>` implementing `wire::commit::CanonicalCommitPipeline`
   (`wire/commit.rs:401`) via `admit_candidate` → `VerifiedLogStore::append`; maps
   `AppendError::StorageUnavailable` to the wire's indeterminate result, never to a reject.
4. Unify the two `ScopeKeyStore` traits: keep `crypto::scope_key::ScopeKeyStore` as the one
   custody trait; make `retention::crypto_shred::crypto_shred` take it (add the destroy/record
   methods it needs to the single trait, or a blanket `impl<S: crypto::ScopeKeyStore + ShredLedger>`),
   delete the second trait and the duplicate in-memory impl (`crypto_shred.rs:117`). Record in
   §unified-vocabularies.
5. Curate `lib.rs`: keep `pub mod` list, add `pub mod prelude` (or top-level `pub use`) re-exporting
   the stable downstream paths fe-database/fe-api actually import today (grep their `use
   fe_canonical_log::` lines; do not break them), plus the new compose types; a module doc line
   stating the dependency rule (`fe-canonical-log -> fe-sdk` only, see §Forbidden dependencies).
6. AGENTS: §module-ownership `compose` row → "W4 integration — implemented"; add
   §trait-composition-map (table: trait → production implementor → production caller, one row
   per trait in §2's gap table — this IS the "0 NO_CALLER" census); append new provisional rows
   (materializer-id domain from W4-3) to §Provisional wire numbering.

**Invariants.** §single-seal-entry-point (`seal_artifact` remains the only seal door);
§unified-vocabularies; SPEC-6 §2.1.6 immutability; `retention/AGENTS.md` crypto-shred order
(stop reissue → destroy → record). **(Critic, cross-plan)** `SealedProofEnvironment`'s match on
`LaneClass` is total with **no wildcard**, so when CDL-SPEC10 A2 adds `LaneClass::BulkArtifact`
the compiler forces that slice to handle it (A2 now owns that arm).

**Flow tests:**
- `crypto_shred_through_the_custody_store_makes_the_payload_unopenable` (seal, shred, open fails
  with key-unavailable) — exercises the unified trait end to end.
- `a_rewrite_of_an_existing_artifact_id_is_refused_by_the_byte_store_adapter`.
- `commit_submit_resubmitted_byte_identical_acks_already_present_not_a_second_append`.

---

### Slice W4-5 — Cross-spec integration test (M, sonnet)

**Owned files:** NEW `fe-canonical-log/tests/cross_spec_integration.rs`; may add fixture JSON
under `fe-canonical-log/tests/fixtures/` (new files only; `docs/spec/canonical-log/*.json` and
`.mjs` are read-only).
**Crates:** fe-canonical-log.

**Flow tests** (public API only; production compose types + in-memory stores; no DB, no
network), each named for the flow:
1. `golden_vector_envelope_admits_through_the_composed_verifier` — the SPEC-1 golden envelope
   (`docs/spec/canonical-log/operation-envelope-v1.json`) with its fixture chain
   (`tests/fixtures/capability-chain-v1.json`) admits; flipping one signature byte →
   `InvalidEnvelope`.
2. `a_capability_gates_append_and_is_refused_after_a_scope_epoch_bump`.
3. `a_valid_claim_still_reports_unresolved_reachability_without_header_or_decrypt_authority` —
   `verify_checkpoint_claim` passes, the SPEC-6 proof over `SignedCheckpointProofView` +
   `SealedProofEnvironment` returns `HeaderReachableOnly`/`Unresolved` when the header segment is
   withheld or the lane key is absent.
4. `a_sealed_payload_round_trips_and_fails_closed_when_its_aad_envelope_is_mutated`.
5. `commit_submit_ack_delta_flows_over_the_composed_verifier` — `handle_commit_submit` →
   `build_commit_ack` → `is_delta_authorized_for_subscription` over `VerifyingCommitPipeline` +
   `ChainCapabilityVerifier`; a subscriber of a sibling petal receives no delta.
6. `a_device_enrolment_gates_a_scope_key_wrap` — placed here only after W4-6 lands (see W4-6;
   the executor of W4-6 appends it).

---

### Slice W4-6 — Device-enrolment artifact (protocol half of CDL-R2) (M, opus)

**Owned files:** NEW `fe-canonical-log/src/crypto/device_enrolment.rs`,
`fe-canonical-log/src/crypto/mod.rs`, `fe-canonical-log/src/crypto/AGENTS.md`,
`fe-canonical-log/src/AGENTS.md` (register rows), `fe-canonical-log/tests/cross_spec_integration.rs`
(append test 6).
**Crates:** fe-canonical-log.

**Steps.**
1. `DeviceEnrolmentV1 { principal: Principal, device_key: DevicePublicKey, enrolled_at_ms: u64 }`
   and `DeviceEnrolmentRevocationV1 { principal, device_key, revoked_at_ms }`, deterministic CBOR
   (provisional keys 0..2), signed with `signing::sign_domain` (`signing.rs:57`) under
   `fe-device-enrolment-v1\0` / `fe-device-enrolment-revoke-v1\0`. `Signed*::decode_and_verify(bytes)`
   = canonical re-encode check + DID binding + signature (pattern of
   `checkpoint::decode_and_admit_checkpoint`, `checkpoint.rs:315`). Verified types are
   `#[non_exhaustive]` with that function as the only constructor.
2. `enrolment_record_id(principal, device_key) -> Hash32` (BLAKE3 derive-key, provisional
   domain) — one row per (principal, device) so a revocation overwrites the same key.
3. `EnrolmentRegistry` (pure, in-memory) implementing `DeviceEnrolmentView`: holds verified
   enrolments + revocations; **revocation dominates forever** for that (principal, device_key)
   regardless of timestamps (no LWW on authority — the D1 "auth never LWW" doctrine from
   `hexon-p2p-commons-decisions`); a revocation without a prior enrolment is retained (it still
   dominates a later-arriving enrolment). Answers: enrolled & not revoked → `Granted`; revoked →
   `Refused`; nothing known → `Unknown`.
4. `crypto/AGENTS.md:29` row: "Not built" → "**Structural** — self-signed enrolment artifact
   (R2, 2026-10-13); durable registry in fe-database `canon_log::device_enrolment`"; add
   §device-enrolment explaining why self-signed (only the principal can bind its own device; no
   Manager can enrol a device into someone else's identity) and why revocation dominates.

**Invariants.** `crypto/AGENTS.md` §what-is-structural (a row moves left only with code);
`AuthorizationRecord` three-state (`key_wrap.rs:104`); `src/AGENTS.md` §did_key (principal DID
via the crate's own codec); D-CL24 register discipline (every new key in the single register).

**Flow tests:** `a_wrap_to_an_enrolled_device_opens_and_a_wrap_to_an_unenrolled_one_cannot_be_requested`;
`a_revoked_device_stays_revoked_when_a_stale_enrolment_arrives_later` (arrival-order
independence: both orders converge); `an_enrolment_signed_by_another_principal_does_not_verify`.

---

### Slice W4-7 — Durable authority sources + production verifier instantiation (L, opus; security-sensitive)

**Owned files:** NEW `fe-database/src/canon_log/chain_store.rs`, NEW
`fe-database/src/canon_log/artifact_store.rs`, NEW `fe-database/src/canon_log/scope_key_custody.rs`,
NEW `fe-database/src/canon_log/author_lifecycle.rs`, NEW `fe-database/src/canon_log/verifier.rs`,
`fe-database/src/canon_log/canonical_epoch.rs` (add `ManagerPlusAuthorizationView` impl only),
`fe-database/src/canon_log/mod.rs`, `fe-database/src/schema.rs` (three tables),
`fe-database/src/canon_log/AGENTS.md`, NEW `fe-database/tests/canon_log_composed_admission_test.rs`.
**Crates:** fe-database.

**Steps.**
1. `ChainStore` (table `canon_capability_chain`: chain_id hex, leaf audience DID, epoch scope
   key, canonical bytes): `put_verified_chain(bytes)` decodes, asserts canonical round-trip and
   recomputes `chain_id` before storing; implements W4-3's `CapabilityChainSource`. Chains are
   access-controlled local data (SPEC-3 privacy table, `capabilities-and-revocation.md:396`): never
   replicated, never exposed through fe-api.
2. `SurrealArtifactStore` (table `canon_sealed_artifact`): local-only `ContentByteStore` +
   `PayloadCiphertextSource`; explicitly NOT the iroh blob store.
3. `SecretStoreScopeKeyCustody`: the unified `ScopeKeyStore` (W4-4) as an in-memory map
   write-through to `fe_identity::SecretStore` (account `canon-scope-key/<key_id hex>`), shred =
   delete + zeroize; loads at construction.
4. `DurableAuthorKeyLifecycle`: rebuilds `LineageIndex`/`DisavowIndex` by replaying lifecycle-kind
   rows of `verified_op_log` through W4-3's `LifecycleDispatcher` in
   `ordering::deterministic_causal_order`; implements `AuthorKeyLifecycleView` with
   `CausalOperationView` over the stored parents.
5. `impl ManagerPlusAuthorizationView for DurableAuthorizationView`: Manager+ from the role
   table as `principal_is_manager_plus` does (`canonical_epoch.rs:612-627`);
   `capability_permits_append_checkpoint` by verifying a stored chain for
   `append`/`checkpoint` at the verse scope (needs a `ChainStore` handle — add it as a field set
   by a second constructor, keeping `load` unchanged).
6. `canon_log::verifier::durable_candidate_verifier(db, secret_store, interim_anchor) -> ComposedCandidateVerifier<…>`
   — the ONE production instantiation, and the verifier `admit_and_append` callers use.
   Schema registry: `RegisteredSchemaSet` holding exactly the schemas this build interprets
   (initially the four SPEC-2 lifecycle schemas + W4-9's transform schema); everything else
   answers `UnknownSchema`/`UnknownKind`. **(Critic, cross-plan)** Expose one registration
   function (`RegisteredSchemaSet::register(kind, schema_hash, validator)`) so that CDL-SPEC10 B3
   adds the kind-5 observation schema by a call, not by editing this constructor's body. B3 owns
   that call.

**Invariants.** `canon_log/AGENTS.md` §interim-manager-authority (role key shape
`VERSE#<hex>…`, anchor-scoped answers, Unknown refuses), §epoch-state (CAS, required
`ManagerAuthorityView`), §storage-faults (indeterminate ≠ reject), §parallel-and-dormant
(nothing here is reachable from a live handler); `migration/AGENTS.md` §no-shadow-exposure.

**Flow tests** (`canon_log_composed_admission_test.rs`, real Surreal + real composed verifier;
fixtures sign with in-test keys and put the chain via `put_verified_chain`):
- `a_locally_signed_operation_admits_appends_and_replays_through_the_durable_verifier`.
- `an_operation_after_its_authors_rotation_is_refused_after_restart` — rotation admitted, DB
  thread restarted, `DurableAuthorKeyLifecycle` reloaded → old-key op `UnauthorizedOperation`
  (proves persistence, not process memory).
- `a_checkpoint_from_a_non_manager_signer_is_not_admitted` through `AdmittedCheckpoint::admit`
  with the durable view (`rebuild.rs:76`).
- `shredding_a_scope_key_turns_later_admission_into_quarantine_not_reject`.

---

### Slice W4-8 — Durable enrolment registry + replication + delivery view (CDL-R2 DB half) (L, opus; security-sensitive)

**Owned files:** NEW `fe-database/src/canon_log/device_enrolment.rs`,
`fe-database/src/canon_log/mod.rs`, `fe-database/src/canon_log/AGENTS.md`
(§device-enrolment + §interim-manager-authority paragraph), `fe-database/src/schema.rs`
(one table), `fe-database/src/handlers/replicated_row.rs` (one admission branch + one dispatch
arm), `fe-database/src/lib.rs` (dispatch two commands), `fe-runtime/src/messages.rs` (two
`DbCommand` + two `DbResult` variants), `fe-runtime/src/app.rs` (correlation routing list,
`:239`), `fe-test-harness/src/peer.rs` (exhaustive match, `:994`),
`fe-database/src/migration/mapping_inventory.rs` (add the two kinds to
`OTHER_DISPATCHED_MUTATION_KINDS`, `:49`), NEW `fe-database/tests/device_enrolment_test.rs`.
**Crates:** fe-runtime, fe-database, fe-test-harness.

**Steps.**
1. Table `canon_device_enrolment` (id: record id hex) with `principal_did`, `device_key_hex`,
   `artifact_cbor` (bytes), `revoked: bool`. Write path verifies through W4-6's
   `decode_and_verify` — a row is never stored from unverified bytes.
2. `DbCommand::EnrolLocalDevice { verse_id, correlation_id }` / `RevokeLocalDevice { verse_id, device_key_hex, correlation_id }`:
   the DB thread loads-or-generates the device key via `fe_identity::load_or_generate_device_keypair`
   (`fe-identity/src/x25519.rs:129`) on the DB thread's `secret_store`, signs with the node
   `NodeKeypair` it already holds (`fe-database/src/lib.rs:368`; `None` → typed error, never a
   placeholder signature), stores, and calls `replicate_row(..., "canon_device_enrolment", record_id, row_json)`
   (`lib.rs:155`) into that verse's replica. No new transport, no `WriteRowEntry` shape change.
3. Inbound: in `admit_inbound_row` (`replicated_row.rs:261`) add, BEFORE the bootstrap window
   (`:291-300`), a `canon_device_enrolment` branch that admits iff the artifact verifies AND
   `author_did == principal_did` (only that principal may write it), regardless of role (a Viewer
   must be able to enrol to receive keys); everything else → `Deny`. The empty `Doc::del` marker
   is `NotApplicable` (a revocation must be signed). `apply_row_mode` (`:184`) gains the arm:
   insert if absent; a verified revocation overwrites an enrolment; an enrolment never
   overwrites a revocation.
4. `DurableDeviceEnrolmentView` (loads the table into W4-6's `EnrolmentRegistry`; three-state
   answers) and `DurableDeliveryView` implementing `DeliveryAuthorizationView`
   (`key_wrap.rs:153`) = `DurableAuthorizationView` (issuer half, `canonical_epoch.rs:630`) +
   `DurableDeviceEnrolmentView` + `recipient_decrypt_authority` answered by verifying the
   recipient's stored `decrypt` chains through W4-7's `ChainStore` (`verify_chain_bytes` with
   verb `decrypt`; no stored chain → `Unknown`, chains that fail → `Refused`).

**Invariants.** `replicated_row.rs` doc "A3: admission precedes dispatch — a denied row is
never applied" (`:58-75`); `fe-database/src/AGENTS.md` §replication-backpressure
(`replicate_row` warns on drop, never `.ok()`); root `AGENTS.md` anti-pattern 6 (no silent send
errors); `crypto/AGENTS.md` three-state answers; `fe-sync/src/AGENTS.md` §distributed-query
"Creator copies are load-bearing" (do not filter responders here).

**Flow tests** (`device_enrolment_test.rs`, real DB thread + on-disk SurrealDB; inbound via the
`pub` `apply_replicated_row_handler`, `replicated_row.rs:42`, which is the production path):
- `a_local_enrolment_is_stored_signed_and_emitted_for_replication` — `EnrolLocalDevice` →
  `DbResult` ok; the table row verifies; a `ReplicationEvent` for `canon_device_enrolment` is on
  the channel.
- `a_peer_enrolment_is_admitted_only_from_its_own_principal` — table-driven: (author = principal,
  valid sig) → applied; (author ≠ principal, valid sig) → `Denied`; (author = principal, sig by
  another key) → `Denied`; (Viewer principal) → applied.
- `a_revocation_survives_a_late_enrolment_replay` — revoke then replay the old enrolment row →
  view answers `Refused`.
- `cdl_r2_delivery_acceptance` (the ledger's three outcomes through `issue_scope_key_wrap` /
  `open_scope_key` over the durable views): unenrolled device refused; non-Manager issuer
  refused; enrolled device + Manager issuer + recipient decrypt chain → wrap opens.

---

### Slice W4-9 — Equivalence-evidence harness (supports the CANON-DUAL-EMIT decision; does not wire it) (L, opus)

**Owned files:** NEW `fe-database/src/migration/transform_mapping.rs` (first reviewed mapping),
`fe-database/src/migration/mod.rs`, `fe-database/src/migration/AGENTS.md` (§equivalence-harness,
§transform-mapping), NEW `fe-database/tests/migration_equivalence_harness.rs`,
`fe-canonical-log/src/AGENTS.md` (provisional register rows for the intent schema).
**Crates:** fe-database (+ fe-canonical-log check only).

**Steps.**
1. One reviewed per-kind mapping per migration-plan §4.2.1-2 (`migration-plan.md:138-150`) for
   `UpdateNodeTransform` (single-row, non-D-CL20, quantizable via fe-sdk nano-units):
   provisional intent schema `node-transform-v1` (node id, position/rotation/scale as i64
   nano-units, NO previous value — §4.2 rule 3), scope = petal scope, a `CandidateDerivation`
   impl over `IntentSnapshot` (`migration/candidate.rs:9,113`) that builds and signs a real
   envelope (encrypted payload under the run's scope key), and a `ShadowMaterializer`
   (`migration/rebuild.rs:15`) whose comparator fields are the quantized transform.
   `MappingInventory::seeded()` stays all-Unmapped; only the harness's run charter calls
   `register_mapped` (`mapping_inventory.rs:153`).
2. Harness = a test binary that, for a harness-created test verse (§5.1 rule 2 — no legacy
   history claimed): emits the `branch_genesis` fixture first; runs a corpus (create node via
   legacy, N transform edits incl. concurrent-intent ordering, a tombstone, a denied write, a
   malformed candidate, a mid-run DB-thread restart) through `commit_operation_dual_emit`
   (`boundary.rs:72`) in `DualEmitShadow`; admits candidates through `admit_and_append` with the
   durable verifier; rebuilds three times (`replay_admitted_closure_three_times`,
   `rebuild.rs:72`); compares against the legacy projection export with `compare_projections`
   (`comparator.rs:112`); writes a `MeasurementReport` (`report.rs:61`) with numerators AND
   denominators to `target/migration-evidence/<run_id>.json` (gitignored location).

**Invariants.** `migration/AGENTS.md` §flag-isolation (env var only), §candidate-derivation (no
DB handle — structural), §shadow-ledger (append-only), §comparator (no float variant),
§report/§always-blocked (`blocks_promotion` stays true while the six D-CL20 kinds are unmapped —
do NOT "fix" it), §no-shadow-exposure; migration-plan §5.1 rule 7 (no peer contact).
**No change** to any file under `fe-database/src/handlers/` — the live path stays `LegacyOnly`.

**Flow tests** (the harness itself is the test): `transform_corpus_reaches_projection_parity_across_three_replays`
(asserts `compared_equal` for every transform intent, byte-identical roots ×3, zero
`legacy_succeeded_without_candidate` for the mapped kind, and that `blocks_promotion` is STILL
true because of D-CL20); `a_lossy_transform_mapping_is_detected_as_a_difference` (inject a
mapping that rounds → `compared_different`, proving the harness can fail).

---

### Review + docs pass (end of campaign, before the single sweep)

- `/security-review` on **W4-3..W4-8** (critic: W4-3/W4-4 compose chain verification, AEAD open
  and the commit pipeline, which are security boundaries in their own right; plus enrolment
  admission, chain store, scope-key custody and the replicated-row branch) and
  `/code-review high` on **W4-1, W4-2, W4-5 and W4-9** (critic: W4-1 edits six live handlers and
  W4-9 signs real envelopes; neither was in any review scope). Reviewers FIX their findings
  (standing rule). Verdict line recorded in `metadata.json` `G-WAVE-4` and the decisions record.
- Then the campaign-end gate in §1.

## 4. Docs to update

| File | Change |
| --- | --- |
| `fe-canonical-log/src/AGENTS.md` | `compose` ownership row; §trait-composition-map (the NO_CALLER census); §crate-surface; register rows: materializer-id domain, enrolment domains + keys + record-id domain, `node-transform-v1` schema keys |
| `fe-canonical-log/src/crypto/AGENTS.md` | row `:29` → Structural; new §device-enrolment |
| `fe-canonical-log/src/materialize/AGENTS.md` | `VerifiedCandidate`; new `AdmissionOutcome` variants and their spec sources |
| `fe-canonical-log/src/retention/AGENTS.md` | single custody trait |
| `fe-database/src/canon_log/AGENTS.md` | fix stale §storage-faults (`:94-110`); new §device-enrolment, §durable-authority-sources, §production-verifier; update §interim-manager-authority's last paragraph (enrolment half now exists) |
| `fe-database/src/migration/AGENTS.md` | §bypass-counter (per-kind), §boundary (counters landed; dual-emit still unwired), §equivalence-harness, §transform-mapping |
| `fe-database/src/AGENTS.md` | NEW §canon-log-materialization pointing at `canon_log/AGENTS.md` + `migration/AGENTS.md`; §log-first-commit left intact (it documents the live legacy path) |
| `fe-runtime/src/AGENTS.md` | the two enrolment commands (correlation routing) |
| track `metadata.json` (`G-WAVE-4`, `CDL-R2`), `conductor/decisions/canonical-data-log-20260808.md` | status + verdict + the PENDING-RATIFICATION items of §7 — written by the orchestrator/bookkeeping agent, not by this planner |

## 5. Risks

1. **Trait-signature ripple (W4-2).** Changing `CandidateVerifier` touches fe-database admission
   and its 1700-line test. Mitigation: alignment-only edits; the precondition-before-append
   order (`canon_log/AGENTS.md` §admission-order) is re-read by the reviewer.
2. **Enrolment admission is a new auth branch on the live P2P row path (W4-8).** It must run
   before the bootstrap window or a pre-manifest forged enrolment slips in. Security review is
   mandatory; the table-driven inbound test pins the four author/signature combinations.
3. **Device-key linkability.** Publishing an X25519 device key into a verse replica is a stable
   identifier visible to every replica member. Mitigation: publication only on an explicit
   command (no auto-publish, see §6); record in §device-enrolment.
4. **Shared cores + separate guards = drift** (RUNBOOK §5 house doctrine). `VerifyingCommitPipeline`
   (pure) and `admit_and_append` (fe-database) both sit on `admit_candidate`. Mitigation: the
   fe-database production path must call `admit_and_append`, never re-implement; the composition
   map states which is production.
5. **Requester binding for the wire verifier** is only as good as the future mount's connection
   authentication. Mitigation: per-connection constructor makes it impossible to build one
   without naming a requester; CANON-WS (NO_GO) owns the authentication.
6. **Scope keys in the OS keystore** (W4-7) — keyring write latency and platform differences;
   CI Linux has no keyring. Mitigation: the existing `SecretStore` trait already has a file/in-memory
   backend used by tests; tests use it.
7. **Provisional wire numbers multiply** (materializer id, enrolment, transform schema). All
   land in the single register (CDL-PROV), no interop claimed.
8. **Unknown-author-key semantics** (W4-3): if SPEC-2 §4 says `Unknown` must refuse, the default
   flips and genesis authors need a recorded lineage root. Executor must cite the spec text.
9. **Build pressure**: fe-database recompiles surrealdb-core; RUNBOOK §5 doctrine
   (`CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 -j4`, one builder, prune between slices).

## 6. Deferral audit

| Left out | Why (class) |
| --- | --- |
| Mounting `/ws/canonical`; fe-api instantiation of `ChainCapabilityVerifier` per authenticated connection; a production `CanonicalCommitPipeline` inside fe-api | **NO_GO** — CANON-WS (`doneness-ledger.md:264`) is canonical network exposure |
| Wiring `commit_operation_dual_emit` into live handlers | **Missing user decision** — CANON-DUAL-EMIT is NEEDS_RATIFICATION (`doneness-ledger.md:192`); W4-9 produces the evidence for it |
| Formal §6.1 validation run (owner-authorized corpus, device profiles, latency budget) | **Missing user decision** — migration-plan §6.1 rule 1 requires an owner-approved run charter (`migration-plan.md:302-306`); W4-9 is a dev-evidence run on a harness-created verse |
| Mapping the six D-CL20 kinds | **Forbidden** by D-CL20 / §4.2.5 until each has an approved atomic contract |
| Mapping the other ~36 non-D-CL20 kinds beyond `UpdateNodeTransform` | **Missing user decision** — each mapping only matters for a cutover the owner has not approved; one kind proves the evidence pipeline end to end. Trigger: owner go on CANON-DUAL-EMIT |
| Local capability-chain **issuance** (minting the local principal's root/leaf chains) | **Missing user decision** — the root authority anchor is an interim role-table projection until SPEC-2 authority history exists (`canon_log/AGENTS.md` §interim-manager-authority); issuance is the authoring half of dual-emit. W4-7 builds the chain store and verification so issuance is a pure producer addition |
| Segment production (packing the local log into sealed segments/manifests) and a durable `ProofEnvironment`/`ParentWalk`/`PayloadAdmission` | **NO_GO** — their only consumer is checkpoint bootstrap / seeding from received segments (SPEC-6 §4.2), i.e. relay/peer distribution. W4-4 ships the generic environment and W4-5 tests it |
| Auto-enrolling the local device when a verse replica opens (GUI trigger) | **Hard dependency + privacy** — the only consumer of enrolments is §10.2.4 wrap delivery, owner-gated and unbuilt (`crypto/AGENTS.md:8`); auto-publishing a device key with no consumer is pure linkability. Command-driven enrolment ships now |
| §10.2.4 wrap delivery transport | **NO_GO** (network) |
| CDL-ROTATION-ASSEMBLY | **Unscheduled by the owner** (`doneness-ledger.md:193`); W4 gives rotation *admission* a production caller, not *assembly* |
| CDL-FORK rotation-fork resolver | **Missing user decision** — D-CL24(2) |
| CDL-R-ACCEPTED residuals 3/4/5/7 | **Owner decision** recorded "open by decision" (`doneness-ledger.md:186`) |
| CDL-STATS | **Trigger-gated** — no selective-fetch consumer (`doneness-ledger.md:106`) |
| CDL-PROV ratification of provisional numbers | **Owner decision** — W4 only appends rows to the register |

Nothing in the 2026-08-08 brief is dropped silently: brief items 1a/1b/1f and the register
index are already done (§2); every other item is a slice above.

## 7. User decisions (each recorded with a default — none blocks the campaign)

1. **SPEC-4 §5 has no row for "capability chain / epoch state / payload key unavailable".**
   Default: quarantine per SPEC-2 `author-key-lifecycle.md:240-241`, recorded as
   PENDING-RATIFICATION erratum E-W4-1 in the decisions record. Reversal: make it a reject —
   one `disposition()` arm (cheap).
2. **Formal equivalence run charter** (§6.1 rule 1). Default: W4-9 ships dev evidence on a
   harness-created verse; the formal run waits for the owner's charter. No reversal cost.
3. **Provisional wire numbers** for the materializer-id domain, enrolment artifacts and
   `node-transform-v1`. Default: provisional, in the single register for CDL-PROV batch
   ratification. Reversal: renumber before any second implementation (cheap now — no consumers).

## 8. Assumptions (highest reversal cost first)

- R2 (2026-10-13, user) overrides the track spec's older "do not extend `WriteRowEntry`
  row-replication" line (`spec.md` "Status and boundary") for enrolment rows: they ride the
  existing `replicate_row` → `WriteRowEntry` path with no shape change. They are legacy-row-plane
  data (public keys + signatures), not canonical-log traffic · reversal: drop the replicate call
  and the admission arm (cheap).
- Revocation dominates forever per (principal, device_key); re-enrolment needs a new device key ·
  reversal: add a signed re-enrol counter (moderate — wire change).
- A Viewer may enrol (enrolment is gated by self-authorship, not role) · reversal: add a role
  floor in the admission branch (cheap).
- Enrolment rows are published per verse on command; the registry view is principal-keyed and
  accepts a valid row from any verse · reversal: verse-scope the view (cheap).
- `KeyState::Unknown` = un-rotated key (authority rests on the chain) — executor verifies
  against SPEC-2 §4 · reversal: one match arm (cheap) but needs lineage roots (moderate).
- `UpdateNodeTransform` is the first mapped kind (single-row, quantizable, highest-frequency
  edit) · reversal: pick another kind (cheap — harness is kind-generic).
- Scope keys live in the `SecretStore`, never in SurrealDB · reversal: moderate.

## 9. Critic amendments (2026-10-13)

Build order: PAS-P3P4 → PAS-P5 → **CDL-W4** → CDL-SPEC10 → role propagation.

**Deferral audit: every row stands.** Each one is classed as NO_GO (CANON-WS, CDL-NET), a missing
user decision (CANON-DUAL-EMIT, the §6.1 charter, issuance, CDL-FORK, CDL-PROV, R-ACCEPTED), the
D-CL20 prohibition, or a hard dependency on a NO_GO consumer (auto-enrolment). Nothing was moved
in.

**RUNBOOK §3 check (quoted both ways).**
- §3 "Carried from earlier": *"canonical-data-log network rollout / relay seeding / inbound P2P /
  `/ws/canonical` mount stay NO-GO (local Workstream G implementation is GO)"*.
- §3 R2 (2026-10-13, later and specific): *"CDL-R2 enrolment artifact: self-signed, replicated
  … replicates as an ordinary row; revocation = signed tombstone. Unblocks a production
  `DeviceEnrolmentView` in CDL Wave 4."*
- W4-8's inbound `canon_device_enrolment` branch rides the legacy row plane by explicit user
  decision R2. It is **not** canonical-log inbound P2P: no operation envelope, segment or
  `/ws/canonical` traffic is admitted. **Reconciled, no change.** The §8 first assumption already
  records it.

**Edits made.**

| # | Change | Class |
|---|---|---|
| C1 | `/security-review` scope widened to W4-3/W4-4 (chain verify, AEAD open, commit pipeline). `/code-review high` now covers W4-1 (six live handlers) and W4-9 (signs real envelopes), which were in no review scope. | missing review |
| C2 | W4-4: `SealedProofEnvironment`'s `LaneClass` match has no wildcard, so CDL-SPEC10 A2's lane 5 is compiler-forced (A2 owns that arm). | cross-plan collision |
| C3 | W4-7: `RegisteredSchemaSet::register` added, so CDL-SPEC10 B3 registers kind 5 with a call. | cross-plan ordering |

**Later-phase obligations (recorded here; owned by the later plan).**
- **Role propagation RP-2** turns `role` into a materialized cache and drops `role = "owner"`
  rows. `DurableAuthorizationView::load` reads `SELECT peer_did, scope, role FROM role`
  (`canonical_epoch.rs:540`) and never consults `verse.created_by`. RP-2 owns the fix so that
  W4-7's `ManagerPlusAuthorizationView` and W4-8's issuer half keep answering Owner-is-Manager+.
- **Role propagation RP-3/RP-4** must keep W4-8's enrolment branch **first** (verify, then
  author == principal, role-independent). It is never `Deferred AuthorRole` and never subject to
  the narrowed manifest-only bootstrap window's role gate. If RP-3 changes
  `apply_replicated_row_handler`'s signature, it re-points `fe-database/tests/device_enrolment_test.rs`.
- **Role propagation RP-2** edits `crud.rs` (owner-row removal). It must keep W4-1's six
  `record_bypass_for` first-statement lines byte-identical.
- **CDL-SPEC10 A2** owns the `BulkArtifact` arm in `compose.rs`; **B3** owns the kind-5 schema
  registration and must register through C3.
