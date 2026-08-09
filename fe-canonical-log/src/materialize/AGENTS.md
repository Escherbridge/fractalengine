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

The brief listed nine variants "one per §5 row" without settling which of the two remaining ones
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
  defines the trait only and does not import either sibling's concrete types.
- `SignedCheckpointClaim` (`W2-branch-checkpoint`) and `CheckpointView` (`W2-segment`) both need
  to be bound to `CheckpointBinding` eventually; this module deliberately does not import either.
