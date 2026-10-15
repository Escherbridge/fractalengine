# materialize (SPEC-4)

Pure identity, admission-error taxonomy, checkpoint binding, and trait contracts for the
log-first materializer. No SurrealDB, no I/O: this crate is a leaf, and the storage-backed half
of SPEC-4 lives in `fe-database/src/canon_log/` (a later wave) implemented against the traits
in `traits.rs`.

## Why `MaterializerVersion` is hand-authored

§1.5 requires a materializer change to be an explicit author decision: the moment reduction
logic changes, the version must change, and the moment it does not, the version must not. Tying
it to `CARGO_PKG_VERSION` or `env!` would silently rev the identity on every crate release --
including releases that never touched a `CausalMaterializer::reduce` implementation -- which
would invalidate every checkpoint and force a full rebuild for no reason, or worse, tie two
different reduction logics to the same identity across an unrelated crate downgrade. `identity.rs`
has no `env!`/`CARGO_PKG_VERSION` reference by construction; the acceptance grep checks this.

`ProjectionIdentity` and `ApplyMarkerKey` both fold `MaterializerVersion` into their equality, so
a version bump is, by construction, a distinct projection and a distinct apply-marker namespace
for the same branch -- this is what D-CL19 means by "part of checkpoint identity."

## Why this crate stays I/O-free

`VerifiedLogStore`, `CandidateVerifier`, `CausalMaterializer`, `EquivocationIndex`, and
`ParentVerseLookup` are trait seams, not implementations. `fe-database/src/canon_log/` is the
only place SurrealDB queries for this path may live (see the workstream's architecture
decisions). Keeping this crate storage-free is what lets every property below be unit-tested
with in-memory fakes in milliseconds, and what keeps `fe-api`/`fe-sync` from inheriting an
embedded database transitively through a types crate.

## G2 -- a reduction writes references, it never reads artifacts

Erratum G2, SPEC-4 §4 rules 8-9. `meta`, `envelope_bytes`, and — since W4-2 — the operation's
own `payload_facts` are the ONLY admissible inputs to `CausalMaterializer::reduce` (§4 rule 8:
"the admitted operation's own verified bytes and extracted facts"; see §payload-facts). A reduction may write a deterministic *reference* to an external
artifact — a snapshot component, a segment shard, a D-CL26 columnar observation hexon — where
the referenced identity is a content address derived from the same signed bytes. It may not
dereference that artifact and fold its contents, length, statistics, presence, or absence into
the projected value.

Which artifacts a peer holds is local availability, and under D-CL2 sparse payload replication
it differs between peers by design. A reduction that reads one makes the projection a function
of local storage rather than of the admitted operation set, so §4 rule 5 (identical state
everywhere) and §6 rule 1 (rebuildable from verified history alone) both stop holding — and the
failure is invisible on the machine that first materialized it, because there the artifact is
present.

`ProjectionMutation::ReferencedArtifactUnavailable` exists so the third outcome is
*representable*. The enum previously offered only `Apply` and `Excluded`; a materializer that
could not resolve a reference therefore had to spell an availability gap as a settled decision,
and `Excluded` is terminal evidence under §4.6. Two peers would then disagree about a result one
of them never computed. The variant is deliberately not a `Result`: the caller must handle it as
one of the reduction's ordinary outcomes and leave the apply marker where it is, never advance
past it with a default, an empty reference, or a partial value.

## §verified-candidate -- the verifier sees the envelope, not just the facts

`CandidateVerifier` used to receive only `VerifiedEnvelopeMeta`, which carries no `CapabilityRef`
and no `PayloadRef`: an honest `verify_authorization` could not find the capability chain, and
an honest `verify_payload` could not rebuild the §5.2 AAD or check the ciphertext hash. W4-2
made `verify_envelope` return a `VerifiedCandidate { meta, envelope }` and passes that to the
other two methods.

`VerifiedCandidate` is sealed the same way `VerifiedEnvelopeMeta` is (`#[non_exhaustive]`, a
`compile_fail` doctest): outside this crate the only constructors are
`VerifiedCandidate::verify_from_bytes` and `verify_candidate`, both of which run
`signing::decode_and_admit`. Holding one therefore proves the mandatory ingress ran on some
bytes, and the envelope cannot drift from the facts because both come out of that one decode.
There is no hazardous projection door for it, unlike `verified_envelope_meta_from`.

`admit_candidate` keeps returning `VerifiedEnvelopeMeta` so existing callers keep their types;
`admit_candidate_with_facts` is the same pipeline returning `AdmittedCandidate { candidate,
payload_facts }`, and `admit_candidate` is a thin projection of it — one implementation of the
parent and equivocation rules, not two.

## §admission-vocabulary -- `UnknownKind` and `AuthorizationEvidenceUnavailable`

Two `AdmissionOutcome` variants close drift between this enum and the specs (W4-2):

- **`UnknownKind { operation_kind }`** — the SPEC-4 §5 `unknown_kind` row (erratum G1). A
  validly signed operation of a kind this build has no structural rule, schema, and reduction
  for is a Quarantine, never `InvalidEnvelope`: an upgraded peer may hold it valid, and the
  kind arrives by build upgrade, so the same bytes must admit unchanged after the upgrade.
  `verify_envelope` is where an implementer raises it (`signing::decode_and_admit` lets kinds
  5+ through structurally).
- **`AuthorizationEvidenceUnavailable { reason }`** — SPEC-2 `author-key-lifecycle.md` rule 5
  ("a missing parent, schema, payload key, capability chain, or epoch state remains
  quarantined"). SPEC-4 §5 has no row for it; erratum **E-W4-1**, the quarantine default
  **owner-confirmed 2026-10-13 (R7)**. Reversal is one `disposition()` arm.

`reason` is an `AuthorizationEvidenceGap`, a newtype over the crate's single
`compose::QuarantineReason` (§unified-vocabularies: wrap, never duplicate — `reason()` answers
in that vocabulary). Its field is private, so `AuthorizationEvidenceGap::new` — which hands back
every reason other than `CapabilityChainUnavailable`, `EpochStateUnavailable`, and
`PayloadKeyUnavailable` — and the three named constructors are the only doors; an enum-variant
field alone could not enforce that, since variant fields are always public. It is stored as a
private one-byte tag rather than a whole `QuarantineReason`, which would have grown every
`AdmissionOutcome` (and `fe-database`'s `AdmissionRejection`) past clippy's `result_large_err`. `new` is a total match with no wildcard, so a future `QuarantineReason` is classified
before it compiles. A refused chain (`Unauthorized`) is never an evidence gap: that is
`UnauthorizedOperation`, a reject.

## §payload-facts -- how a reduction reads its own payload (critic C4, SPEC-10 §12)

`CausalMaterializer::reduce` takes `payload_facts: Option<&PayloadFacts>`. SPEC-10 §12 names
the seam: a kind-5 reduction needs its admitted payload's extracted facts (§4.4 rule 2), and so
does every payload-bearing kind, including W4-9's `node-transform-v1`. It is widened once, here,
so CDL-SPEC10 B1/B3 consume this signature instead of adding a second, narrower one.

- **Where facts come from.** `CandidateVerifier::verify_payload(candidate, payload_ciphertext)`
  returns `Option<PayloadFacts>`: the implementer authenticates the ciphertext (hash, length,
  `crypto::aead::open` under the §5.2 AAD), validates the plaintext against the schema, then
  calls `PayloadFacts::from_validated_plaintext` last. That constructor refuses the no-payload
  form and non-canonical CBOR, and binds `op_id`/`schema_hash` from the candidate, so a
  reduction can check the facts are its own (`facts.op_id == meta.op_id`).
  `admit_candidate_with_facts` enforces that binding itself: facts whose `op_id` or
  `schema_hash` differ from the admitted candidate's are `InvalidPayload`, whatever verifier
  produced them (CDL-W4 review-and-fix).
- **What facts are.** Values only — the decoded canonical-CBOR plaintext. No artifact id to
  dereference, no ciphertext, no key, no handle. G2 holds because there is nothing in the
  argument a reduction could fetch with.
- **When facts are absent.** `None` means the no-payload form OR "the caller holds no decrypted
  payload for this operation". A reduction that needs facts and gets `None` returns
  `ReferencedArtifactUnavailable { artifact_id: PayloadRef::ciphertext_hash }`: the payload
  ciphertext is a "payload artifact" in §4 rule 8's list, the position stays pending replay,
  and the apply marker does not move. Never `Excluded`, never a default.
- **Replay-time facts (CDL-SPEC10 B2b, 2026-10-14; closes the former interim gap).** The
  durable replay path re-derives facts through a source, never through `reduce`:
  `fe-database` `canon_log::replay::replay_to_frontier_with_facts` /
  `rebuild::rebuild_projection_with_facts` take a `ReplayPayloadFacts`, and
  `VerifierReplayFacts` answers it with the verifier's own `verify_payload` (the W4-7 ciphertext
  store, scope-key custody, and `RegisteredSchemaSet`), bound to the stored bytes' `op_id` and
  `schema_hash` exactly as admission binds them. The reduction still receives only
  `Option<&PayloadFacts>`, so G2 is untouched. The legacy `replay_to_frontier` /
  `rebuild_projection` (every pre-B2b call site) pass the `NoReplayPayloadFacts` wrapper and
  keep their `None` outcome. When the source cannot re-derive (a crypto-shredded or absent scope
  key, a missing ciphertext, an unregistered schema), the position is pending under the distinct
  `PendingReason::PayloadFactsUnavailable`, never `Excluded`, and a row projected before the
  shred is kept: replay skips marked operations before any lookup, and a from-empty rebuild that
  would discard such a row is withheld. Full design: `fe-database/src/canon_log/AGENTS.md`
  §replay-payload-facts; spec side: `observation-plane.md` Errata (2026-10-14).

## M2 -- `VerifiedEnvelopeMeta` carries `scope`

The brief's field list for `VerifiedEnvelopeMeta` omitted scope; amendment M2 requires it.
Four downstream MUSTs are unimplementable without it: SPEC-1 §6.2 same-verse parent checking
(implemented here as `traits::validate_same_verse_parents`, exercised through
`traits::admit_candidate`), SPEC-2 disavow scope matching, SPEC-2 scope-propagated lineage
resolution, and SPEC-3 permission cells (the latter two are the author-key and capability
slices' job, not this one's -- they consume `VerifiedEnvelopeMeta.scope` once the integration
wave wires the concrete `CandidateVerifier`).

## M1 -- author equivocation is a first-class admission outcome

`AdmissionOutcome::AuthorEquivocation { op_id, conflicting_op_id }` represents D-CL25 /
SPEC-1 §3.4: two distinct `op_id`s sharing one `EquivocationKey`. Its disposition is
`Quarantine`, never `Reject` -- rejecting would let an attacker force one of the two candidates
to be silently dropped by controlling arrival order, which is exactly the fork the equivocation
rule exists to prevent. Both must be retained as evidence and neither materialized until an
authorized resolution operation runs; that resolution operation does not exist in this wave and
is explicitly out of scope here (SPEC-2's author-key lifecycle slice owns anything close to it,
and even it retains unresolved forks rather than resolving them -- see
`fe-canonical-log/src/author_key/AGENTS.md`).

`traits::check_author_equivocation` and `traits::validate_same_verse_parents` are pure
functions, but per the no-dormant-gates rule (M5) they are not orphaned: `traits::admit_candidate`
is a real, tested caller of both, composed only from the `CandidateVerifier`, `EquivocationIndex`,
and `ParentVerseLookup` seams this module defines. `fe-database/src/canon_log/admission.rs`
(a later wave) is expected to either call `admit_candidate` directly or reproduce its exact
step order against its concrete, storage-backed trait implementations -- either way, both checks
have a real caller before a byte is durably appended.

## `AdmissionOutcome` disposition classification

The brief listed nine variants "one per §5 row" (W4-2 later added `UnknownKind` and
`AuthorizationEvidenceUnavailable`, both Quarantine — §admission-vocabulary) without settling
which of the two remaining ones
(`MaterializationFailed`, `CheckpointMismatch`) are §5.1 rejects or §5.2 quarantines. Neither
fits: both represent an operation that is *already durably appended* -- rejecting it after
append would violate exactly-once durability (§3.1), and quarantining it would misfile a
materialization-time failure as a pre-admission availability gap. `AdmissionDisposition` adds a
third value, `DeferredMaterialization`, for exactly these two. `is_reject()`/`is_quarantine()`
are convenience predicates over `disposition()`; neither returns `true` for the deferred
category, and the disposition assignment is exhaustively pattern-matched (not a default branch),
so a future `AdmissionOutcome` variant fails to compile until classified.

## Checkpoint triplication -- this module's slice of the resolution

Three specs each describe a "checkpoint" type. The workstream's resolution: `W2-branch-checkpoint`
owns the only concrete signed claim type (`SignedCheckpointClaim`); `W2-segment` defines an
abstract `CheckpointView` trait it merely consumes; **this module owns only the §6.3/§6.4
binding validator** and is forbidden from defining a claim type or re-hashing a frontier.
`checkpoint_binding.rs` therefore:

- imports `SortedFrontier` from `frontier.rs` and calls `.commitment()` -- it does not
  reimplement frontier hashing;
- has no signature field, no signing-key parameter, and no verifier trait for a Manager+
  signature -- `CheckpointBinding::validate` cannot be handed a signature to trust instead of
  recomputing the frontier commitment, which is the point of §6.4 ("a matching Manager+
  signature alone is never proof");
- does **not** compare `projection_root_hash` against an independently recomputed value.
  Proving the root matches requires an actual replay, which is the sibling `ReplayVerifier`
  seam's job (SPEC-5/SPEC-6), not something a pure binding-equality check can do without I/O.

Binding `CheckpointBinding` to `SignedCheckpointClaim` and to `CheckpointView` happens in
`W4-integration/compose.rs`, not here.

## §4.7 invalidation trigger -- unmodeled by design

§4.7 describes a projection-invalidation *trigger* mechanism but the approved spec set does not
pin down what fires it. Rather than invent one, this module models only forward replay over an
explicitly supplied frontier (`ordering::deterministic_causal_order` plus
`VerifiedLogStore`/`CausalMaterializer`, driven by whatever frontier the caller selects). A
future slice that specifies the trigger can add it without touching any type here.

## Ordering is a pure function, not a scheduler

`ordering::deterministic_causal_order` takes the *closed set* of operations to order -- it does
not fetch parents itself and does not care what order they were pushed into the input slice.
Determinism follows from breaking ties by `(wall_ms, counter, author_public_key, op_id)`
at every step, not from the caller doing anything special; the permutation test in
`ordering.rs` is the property proof.

## Deferred to Wave 3 (fe-database, storage-backed)

The eleven SPEC-4 §8 named conformance tests all require durable state (an embedded SurrealDB,
crash/replay across a real store) that this I/O-free crate cannot provide. They are implemented
against `fe-database/tests/canon_log_materialization_test.rs` in the `W3-db-canon-log` slice
under their exact names:

- `append_before_projection`
- `strict_append_failure_leaves_projection_unchanged`
- `append_and_apply_idempotence`
- `crash_between_append_and_apply_replays`
- `crash_after_atomic_apply_does_not_double_apply`
- `causal_and_concurrent_order_is_arrival_independent`
- `invalid_midstream_candidate_blocks_complete_claim`
- `materializer_failure_stays_pending_until_replay`
- `checkpoint_binds_frontier_manifest_and_materializer`
- `empty_rebuild_matches_checkpoint_replay`
- `analytics_source_identity_is_reproducible`

This module's own `ordering.rs` tests (for example
`order_is_identical_across_every_insertion_permutation`) are deliberately named *differently*
from `causal_and_concurrent_order_is_arrival_independent` even though they exercise the same
property in-memory: they are unit-level decision support for the pure ordering function, not a
substitute for the named DB-backed conformance test, and must not be miscounted as satisfying it
in the wave-4 named-conformance census.

## Needs from other slices (resolved in the integration wave, not here)

- The concrete `CandidateVerifier` implementation needs the capability slice's chain-verification
  entry point (`W2-capability-chain`) and the author-key slice's `admit_rotation`/`admit_disavow`
  admission surface (`W2-author-key-lifecycle`) to build `verify_authorization`. This module
  defines the trait only and does not import either sibling's concrete types. Since W4-2 the
  trait hands the implementer the whole verified envelope (§verified-candidate), so that
  composition (W4-3 `ComposedCandidateVerifier`) is buildable without widening it again.
- `SignedCheckpointClaim` (`W2-branch-checkpoint`) and `CheckpointView` (`W2-segment`) both need
  to be bound to `CheckpointBinding` eventually; this module deliberately does not import either.

## §g6-source-identity -- observation provenance vocabulary (CDL-SPEC10 A2, erratum G6)

SPEC-10 §7 and SPEC-4 §7 rule 5. `identity.rs` holds the vocabulary a kind-5 observation batch
(CDL-SPEC10 B1) signs and an analytics result records. Wire numbers: `src/AGENTS.md`
§"Provisional — SPEC-10 §7 observation source identity".

- **`SourceClass` is re-exported from `fe_sdk::source_class`, never redefined.** fe-runtime can
  see fe-sdk but not this crate, and the ingest doors (CDL-SPEC10 A3) carry the class on
  `DbCommand`; two enums would drift. The wire value is `SourceClass::wire()`; zero is never a
  class, so a zero-initialized map cannot read as a well-formed source.
- **`ObservationSource` invariants hold by construction.** Fields are private; `new` and
  `from_cbor` are the only doors and both enforce rule 3 (decoder iff class `decoder`; `decoder`
  needs a `format_id`) and rule 4 (`fe_sdk::source_class::is_valid_format_id`, the one grammar
  check). `ingest_principal` is length-checked only: deriving it from the door's `did:key` is the
  door's job (rule 5), not the codec's.
- **Canonical order.** `Ord` compares canonical bytes, the order a signed `sources` list is
  sorted in, so B1 cannot invent a second order.
- **`source_id_for(class, label)`** binds the class into the hash, so a device and a decoder that
  share a label stay distinct (§7.2).
- **`AnalyticsSourceIdentity`** extends the SPEC-4 §7 rule 1 tuple (or a `CheckpointBinding`
  that binds it) with the observation input set. The set is private and kept sorted and
  deduplicated on every insert, so the identity is canonical whatever the read order. It is an
  in-process value with no wire encoding; the "unavailable listed batch fails the computation"
  half of rule 5 (SPEC-10 conformance test 19) belongs to the analytics read path that consumes
  a hydrated catalog (CDL-SPEC10 B3), not to this value type.
