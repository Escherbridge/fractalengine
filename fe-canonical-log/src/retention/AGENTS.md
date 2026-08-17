# src/retention — quarantine, GC leases, tombstone retention, crypto-shredding

SPEC-5 §4, §5.1-§5.5. Pure logic only: no persistence, no networking, no AEAD execution. Wave
3 implements `quarantine::QuarantineStore`, `crypto_shred::ScopeKeyStore`, and drives
`leases::GcLeaseRegistry` from `fe-database`.

## §equivocation — quarantine both, materialize neither (M1)

`quarantine::QuarantineReason` is a `pub use` of the crate-wide `compose::QuarantineReason`; this
module defines no enum of its own. See `src/AGENTS.md` §unified-vocabularies for why three
parallel quarantine enums were collapsed into one.

The variants this module reaches are `MissingParent`, `UnknownSchema`, `UnknownKind`, and
`AuthorEquivocation`. The first three have promotion paths (`missing_parent_promotion_ready`,
`unknown_schema_promotion_ready`, `unknown_kind_promotion_ready`) driven by injected views this
module never queries a database through, and each returns `false` for every reason but its own —
a test pins the full cross-product, because a helper that quietly accepted a neighbouring reason
would promote a candidate whose actual precondition never arrived.
`AuthorEquivocation` deliberately has **no** promotion path anywhere in this module —
both helpers return `false` for it unconditionally, and a test pins that. It now names both
conflicting operations (`first_op_id`, `second_op_id`) rather than "the other one", because an
asymmetric reason invited a reader to treat the named side as the loser. Per SPEC-1 §3.4 and
`envelope::EquivocationKey`'s own doc, releasing an equivocation quarantine requires an
authorized resolution operation, which belongs to SPEC-4's materializer, not this crate. Do not
add a generic "resolve" path here that would let a receiver pick a winner.

The reason carries no `op_id` of its own: `QuarantineRecord` is keyed by `claimed_op_id`, so a
copy inside the reason could disagree with the key it is stored under.

## §budget-partition — one flood may not evict another's backlog (G1)

Erratum G1, SPEC-5 §4 rule 4. The entry and byte budgets are partitioned by
`QuarantineReasonClass` — `MissingParent`, `UnknownSchema`, `UnknownKind`, and one residual
`Other` — and `QuarantineBounds::per_reason` carries a `ReasonBudget` for each. `admit_candidate`
checks the candidate's own class **before** the pool-wide bounds, so an exhausted class is
declined by name while the pool still has room; `evict_expired_or_over_capacity` runs a per-class
pass before the pool-wide one, so an over-budget class sheds only its own records even when
those records are the newest in the pool.

The defect this closes is availability, not correctness. Under D-CL2 headers replicate
verse-wide with no version gate: an un-upgraded peer receives every operation of a kind it
cannot interpret, and against one shared pool that traffic evicts the peer's own legitimate
`MissingParent` backlog oldest-first. Every candidate involved is validly signed, so no
authentication check catches it.

`PerReasonBudgets` is a struct with a field per class rather than a `BTreeMap` keyed by class
precisely because a map can omit a class, and the omission is invisible — the reader cannot tell
whether the missing class was meant to be unbounded or zero. `QuarantineStore::class_len` and
`class_total_bytes` have default implementations derived from `entries()`; Wave 3's
`fe-database` store should override both with an indexed query rather than scanning the pool on
every admission.

## §reserved-policy — bounds are never invented here (M7, D-CL24)

`QuarantineBounds` (`max_entries`, `max_total_bytes`, `max_age_ms`, `max_parent_depth`,
`retry_cadence_ms`, and every `per_reason` budget) and every GC-lease duration are plain fields
on caller-constructed structs.
Neither type carries a `Default` impl or an associated constant. `admit_candidate` fails closed
on any bound rather than evicting existing history; `evict_expired_or_over_capacity` is the only
function that removes local quarantine copies to satisfy a bound, and it never runs implicitly —
a caller (Wave 3's GC driver) must invoke it on its own cadence.

## §crypto-shred — key destruction, not byte deletion

`crypto_shred::crypto_shred` runs stop-reissue, then destroy-key-and-wraps, then
record-disposition, in that order (§5.5.4). `DestructionDisposition` has no byte-array field —
it structurally cannot carry the destroyed key. Per §10.3.3, "remove controlled wraps" is
**best-effort local-store deletion only**: this module makes no claim about immutable
ciphertext, uncontrolled key copies held elsewhere, or any replicated segment. A caller that
reads `crypto_shred` returning `Ok` as "the payload is now unrecoverable everywhere" is reading
more into it than the function promises.

## §tombstone — bound to the real checkpoint verification

`tombstone::CheckpointVerdict` is **deleted**. It was a two-variant caller-supplied stand-in, and
SPEC-5 §5.4 rule 2 permits tombstone compaction only after a replay-verifiable checkpoint proves
the suppression's effect — a rule that `may_compact_tombstone` could not enforce while any caller
could satisfy it by typing `Verified`: no signature, no replay, no frontier commitment.

`checkpoint::compaction_decision` is now the single gate for both §2.2 rule 2 multi-head
compaction and §5.4 rule 2 tombstone compaction. It takes the real
`checkpoint::CheckpointVerification` from `verify_checkpoint_claim`, plus the
`TombstoneRetentionRecord`s themselves as a required parameter, and runs `assert_no_resurrection`
over each one. `BootstrapCoverage` therefore no longer carries an
`every_suppression_effect_is_checkpoint_proved` bool: that evidence is inspectable from the
records, so it is inspected rather than asserted. The one bool that remains,
`every_head_has_retained_bootstrap_path`, is genuinely the caller's — only it knows what it still
stores.

`may_compact_tombstone` survives as the single-record spelling and returns the same
`CompactionDecision`, delegating to `compaction_decision` with `slice::from_ref`. It is not a
second, weaker path; a test asserts the two agree.

This module therefore does depend on `crate::checkpoint` now. The wave-2 decoupling rule served
its purpose while the slices ran in parallel; keeping it after the integration wave would have
left the dormant gate in place, which M5 forbids.

`tombstone.rs` reproduces, for the canonical path, the non-resurrection invariant
`fe-database/src/merge.rs:49-109` enforces for the legacy replicated-row path (a local
tombstone dominates an incoming live row). That file is read-only from this slice's
perspective; it is not imported or edited.

## §leases — no numeric default, and no ambient trust

`leases::GcLeaseRegistry` never advertises a lease until `acknowledge` (or `renew`) has run;
`accept` alone leaves it `PendingAcknowledgement`. `authorize_fetch` only ever returns `Ok(())`
or a typed error — it has no return path that could carry a decrypt key or an append grant, so
"authorize_fetch never grants append or decrypt" is structural, not a convention a future edit
could accidentally violate through this function's signature.

`gc_eligibility` is a method on `GcLeaseRegistry`, not a free function, because the SPEC-5 §5.3
rule 3 eligibility rule is about lease *state* and the registry is the only thing that holds it.
The previous free function ignored its own `artifact` parameter, consulted neither
`GcLeaseDescriptor::expires` nor `.legal_hold` nor `LeaseState`, and reduced to "are these two
slices empty and these two bools false" — a caller-obligation restatement wearing the name of a
gate.

It now resolves everything it can: it walks its own leases, keeps only those whose committed
artifact set contains the artifact, and among those counts a lease as blocking only when
`LeaseState::holds_retention_obligation()` and `now < expires`. `PendingAcknowledgement` counts as
an obligation (accept already committed the holder); `FailedDurable` and `Expired` never do. A
legal hold blocks regardless of state or expiry — a hold outlives the lease that carried it.
Blocking priority, in order: legal hold, then active lease, then branch replay, then tombstone
retention.

Artifact-set membership arrives as the **required** `ArtifactSetMembership` trait parameter rather
than as a caller pre-filter, because `artifact_set_commitment` is an opaque commitment only the
segment slice's manifest/HashSeq machinery can open. A required seam cannot be satisfied by
handing over an empty slice; a pre-filter could be.

`now` is always a caller parameter. This crate never reads a clock.

## §interpretation gaps for owner review

Two places where §4/§5 prose left an implementation choice to this slice, flagged here rather
than silently decided:

- **§4 rule 3 lists `max_age_ms` alongside `max_entries`/`max_total_bytes`/`max_parent_depth`
  as bounds whose exhaustion must "decline further quarantine admission."** Count, bytes, and
  parent-depth are properties of the CANDIDATE being admitted, so `admit_candidate` checks them
  synchronously and fails closed. Age is a property of elapsed wall-clock time acting on
  records ALREADY held, so this module surfaces it through
  `evict_expired_or_over_capacity` instead — consistent with §4 rule 5's explicit allowance
  for expiry eviction removing only the local quarantine copy. `admit_candidate` does not
  itself reject a new candidate merely because an unrelated existing record has aged out.
- **§5.3 rule 1's descriptor field list ("lease identifier, holder principal, authorized
  issuer, verse/scope, artifact-set commitment or manifest root, issue/expiry times, and any
  legal-hold or replacement condition") includes an optional "replacement condition."**
  `GcLeaseDescriptor` carries `legal_hold` but no `replacement_condition` field — the prose
  gives no further shape for it and no sibling slice's brief defines one either. Left out
  rather than invented; add it when a concrete replacement-lease workflow is specified.
- **`ArtifactSetMembership` has no implementation in this crate.** Wave 3 implements it over the
  segment slice's manifest/HashSeq machinery, which can open an `artifact_set_commitment`. Until
  it does, the trait is a required parameter with test doubles only — which is the point: the
  eligibility gate cannot be reached without someone answering "does this lease's set include this
  artifact."

## §no-wire-numbering-here

Nothing in this module is CBOR-encoded, signed, or hashed as a wire artifact: quarantine
records, lease descriptors, and tombstone-retention records are local process state a caller
persists however it likes. There is therefore no provisional wire-numbering table in this
document. Every number this crate invented now lives in one place, `src/AGENTS.md`
§"Provisional wire numbering".
