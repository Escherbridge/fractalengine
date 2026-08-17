# canon_log — the SurrealDB half of the Canonical Fractal Data Log

Normative sources: `docs/spec/canonical-log/log-first-materialization.md` (SPEC-4) for §2 the
commit pipeline, §3 exactly-once append, §4 deterministic materialization, §5 the error
taxonomy, §6 rebuild and checkpoints; and `docs/spec/canonical-log/capabilities-and-revocation.md`
§5.1-§5.3 for persistent epoch state. The pure, storage-free half lives in `fe-canonical-log`;
this module implements the seams that crate deliberately leaves open.

## §parallel-and-dormant

**Nothing in the running engine calls this module.** The DB dispatch loop, every handler, and
`op_log::commit_operation` are untouched: the legacy op-log write path in `src/AGENTS.md`
§log-first-commit remains the only path a live command takes. `canon_log` is a parallel
implementation that will be cut over by a later, owner-gated slice under SPEC-8, and D-CL20 /
SPEC-4 §10 explicitly defer GLTF import, node duplicate/rename, and Verse/Fractal/Petal
creation until each has an atomic intent schema and a replay contract.

Dormant is a fact about wiring, not a licence for untested code: every gate here has a test,
and the two obligations `fe-canonical-log`'s `capability/AGENTS.md` §revalidation and
`branch/AGENTS.md` hand to Wave 3 are discharged in this module (see §obligations-discharged).

The four tables are appended to `schema.rs` and registered in `apply_all`, but deliberately
**not** added to `ALL_TABLE_NAMES`: SPEC-4 §1.4 makes the verified log the authority and the
SurrealDB projection derivative, so an admin "clear everything" that erased `verified_op_log`
would destroy history no rebuild could recover.

## §verified-log

`verified_op_log` stores the exact complete-envelope bytes, base64-encoded, keyed by
`op_id_hex`. Everything else on the row — kind, branch, parents, author key, wall/counter — is a
**durable index derived from those bytes**, present because §2.2 requires enough verified
metadata to find parents without decoding every envelope on every traversal.

`meta_of` therefore never trusts the index columns. It reads the bytes, recomputes
`BLAKE3(bytes)` and refuses a row whose bytes no longer hash to its own `op_id`, then decodes
and projects the envelope. A corrupted index column can slow a query down; it cannot change
what a materializer sees. `scope` and `schema_hash` have no columns at all for the same reason:
they exist only inside the signature preimage.

`append_admitted` takes a `&VerifiedEnvelopeMeta`, not a bare `op_id`. A `VerifiedEnvelopeMeta`
only exists downstream of `admit_candidate`, so §2.1's "validate before you append" is a
property of the signature rather than a rule a caller is asked to remember. The order inside is
also load-bearing: the BLAKE3 recomputation happens **before any row is read or written**, so a
mismatched claim cannot touch storage at all.

Exactly-once is enforced twice: `idx_verified_op_log_op_id` is UNIQUE, so a second row for one
`op_id` is impossible at the storage layer, and the write path read-compares first so a
byte-identical retry returns `AlreadyPresent` (success, §3.2) rather than an error.

## §storage-faults

`VerifiedLogStore::append` returns `Result<AppendOutcome, AppendError>`, and `AppendError` has
exactly two variants: `HashMismatch` and `IntegrityConflict`. **Neither means "the database was
unavailable."** The trait cannot express §2.3's indeterminate append at all, and the same is
true of `get_bytes`/`get_meta`/`parents_of`, which return bare `Option`.

Rather than lie in either direction, this module does three things:

1. The **inherent** API (`append_admitted`, `meta_of`, `parents`, `envelope_bytes`,
   `op_id_at_equivocation_key`) is the primary one and returns `VerifiedLogStoreError` /
   `StorageError`, which name the fault honestly. Everything inside `canon_log` uses it.
2. The **trait** impl exists for callers that only have the seam. On a substrate fault it
   refuses — never succeeds — because §2.3 requires the mutation to fail and SurrealDB to stay
   unmodified. `IntegrityConflict` is the refusal it uses, since that is the variant that
   forbids materializing either candidate; the reply is over-strong but conservative, and §3.4
   makes it recoverable by retrying the exact same immutable bytes. Read methods answer
   `None`, which is the pending-safe direction: replay halts the affected head rather than
   applying an operation whose history it could not read.
3. Either way the real cause is recorded and drainable through `take_storage_faults`, so a
   fault is observable rather than silently rounded off.

**This is a seam gap, not a design preference.** `AppendError` needs a
`StorageUnavailable`/`Indeterminate` variant for the trait to be honest; adding one is a
`fe-canonical-log` change and belongs in an erratum, not here.

## §admission-order

`admit_and_append` runs SPEC-4 §2 in order: envelope verification, SPEC-1 §6.2 same-verse
parents, the D-CL25 author-equivocation check, authorization, the caller's cheap precondition,
payload verification, then the durable append.

The precondition sits between authorization and payload verification, which `admit_candidate`
has no parameter for. Instead of copying that function's parent and equivocation logic here —
two implementations of one rule is how they drift apart — the caller's `CandidateVerifier` is
wrapped in `PreconditionGuardedVerifier`, whose `verify_authorization` calls the inner one and
then the precondition. `admit_candidate` stays the single implementation of the rules it owns.

`AdmissionOutcome::PreconditionFailed` carries no payload, but §5 requires the *original*
command error to reach the caller, so the guard stashes it and `AdmissionRejection::Rejected`
carries it through.

The precondition is a **required parameter**, never an `Option`. Skipping it is spelled
`NoPrecondition` at the call site, which is a visible decision; an omitted `Option::None` is
not. `PreconditionFn` adapts a closure so `handlers::preconditions::require_node_exists`,
`require_petal_scope`, and their siblings pass through unchanged.

No durable append happens on any reject or quarantine, and that is structural rather than
asserted: `store.append_admitted` is unreachable until `admit_candidate` returns `Ok`.
`missing_parent`, `unknown_schema`, and `opaque_payload` surface as
`AdmissionRejection::Quarantined`, because §5 note 2 forbids provisionally applying an
availability state.

## §atomic-apply

`commit_with_marker` builds ONE SurrealQL script — `BEGIN TRANSACTION`, the materializer's
statements, the marker `CREATE`, `COMMIT TRANSACTION` — so §3.5's "the SurrealDB mutation and
its apply marker MUST commit atomically" is a single round trip rather than two hopeful ones.
No other code in this workspace uses a multi-statement SurrealQL transaction, so
`a_failed_statement_rolls_back_the_projection_and_the_marker_together` proves the property
against the real embedded engine with an induced `THROW`.

**If that test ever fails, §3.6 applies and is not optional:** the projection may not be
described as exactly-once, and the caller must rebuild it from the last valid checkpoint or
empty state through `rebuild::rebuild_projection`. That path exists and is tested precisely so
the fallback is a code path rather than a paragraph.

`ProjectionMutation::ReferencedArtifactUnavailable` has no representation in
`CommittableMutation`, whose only constructor is `try_from_projection`. An availability state
therefore *cannot* advance an apply marker — there is no value a caller could build to make it
happen. That is the D-CL28 gate G2 rule enforced by construction rather than by review.

`is_applied` is true for `excluded:*` as well as `applied`, because §4.2's eligibility rule is
about an operation having been *processed*, and a recorded exclusion is processed. Use
`disposition_of` where the distinction matters.

## §binding-projection

`cbor_to_json` maps the log crate's canonical value model onto SurrealDB bindings: unsigned and
negative integers become numbers, byte strings become **lowercase hex**, text stays text,
arrays and maps recurse, integer map keys become their decimal text, and there is no float
case at all — §4.4 forbids floating-point canonical state. A materializer that wants different
bytes-to-column semantics emits its own conversion inside the statement rather than changing
this projection, which must stay stable because two peers' projections have to match.

Binding names starting `__canon_` are reserved for the marker row and refused before anything
is sent, so a materializer cannot shadow the marker's own values.

## §replay-eligibility

`replay_to_frontier` walks `parents_of` from every selected head, computes causal completeness
in topological order, then reduces in `deterministic_causal_order` — `(wall_ms, counter,
author_public_key)` per §4.3 — so the result depends on the DAG and nothing else. Four
insertion permutations of the same DAG produce the same projection root; that is
`causal_and_concurrent_order_is_arrival_independent`.

Eligibility is read from the **durable marker**, not from an in-process set. That is what makes
replay resumable: a pass interrupted halfway resumes exactly where the markers say it stopped,
and a child stays pending until its parents carry markers for this same materializer version.
It also means pending propagates downward for free — an operation left pending has no marker,
so its children fail the parent check on the next pass without any bookkeeping.

A head whose closure is incomplete is **halted**, never skipped (§5 note 1): it is reported in
`halted_heads` with the ancestor that blocks it, and `ReplayOutcome::is_complete` is false, so
§2.6 forbids presenting the projection position as committed. An already-marked operation is
skipped without re-invoking `reduce`.

## §rebuild-and-checkpoints

`rebuild_projection` treats a checkpoint as an accelerator and never an authority (§6.2). It
validates the offered `CheckpointBinding` against the caller's **own** branch, frontier,
segment manifest, and materializer version; a binding that fails is recorded in
`RebuildReport::rejected_checkpoint` and then ignored, and the empty-state path runs instead.
There is no signature parameter anywhere on this path, so "the Manager+ signature matched" can
never stand in for the check §6.4 requires.

The empty-state path clears the materializer's own rows through the `ProjectionSurface` seam
and then clears that projection's markers, so §6.1's "no unpublished SurrealDB row may be
required for correctness" is actually exercised rather than assumed. Marker clearing exists for
exactly this one caller; §5 note 4 forbids advancing or retracting a marker by hand to repair a
projection.

`dag_view` is a required parameter because a completed replay is the only admissible evidence
for `VerseDagView::frontier_is_replay_verified`. Making it a parameter means the evidence is
recorded wherever a rebuild happens, instead of some later caller assuming it.

## §epoch-state

`scope_epoch_state` persists `current_epoch` and the admitted bump operation IDs per exact
scope, keyed by the hex of the canonical CBOR scope map — the same bytes the signature covers,
so two peers derive the same key.

`admit_epoch_bump` takes an `op_id` that must already be in the verified log and reads the
declared epoch from **that operation's own signed capability reference**. Nobody can bump an
epoch with a number the author never signed, and nobody can bump one with an operation that was
never appended. It then enforces §5.1: kind 4 only, exactly one parent, `e -> e + 1` with no
skips and no decreases.

§5.1 rule 6 needs the DAG, not just the counter. A second bump declaring the same `e` is
auditable evidence when it is *concurrent* with the bump that already applied, and stale when it
*follows* it — so the module walks the candidate's transitive parent closure and rejects it as
stale exactly when an already-admitted bump is in that closure. A concurrent duplicate is
retained and the epoch does not move.

## §interim-manager-authority

`DurableAuthorizationView` answers the epoch half from `scope_epoch_state`, which is real. The
Manager+ half is **interim**: SPEC-2's authority history does not exist yet, so it reads the
legacy `role` table through `interim_authority_scope_key` and
`fe_policy::RoleLevel::is_at_least(RoleLevel::Manager)`.

Two things keep the interim honest:

- The role key is `VERSE#<hex>[-PETAL#<hex>][-RESOURCE#<hex>]`, deliberately **not**
  `crate::build_scope`'s `VERSE#/FRACTAL#/PETAL#` string. The two hierarchies are different
  shapes, and quietly reusing one for the other would let an unrelated fractal row answer an
  epoch-scope question.
- `authority_is_manager_plus` answers only for one configured `interim_authority_anchor`, and
  `AuthorityState::Unknown` for every other `issuer_authority_id`. An unrecognised anchor is an
  unanswered question, and an unanswered question refuses — which is the distinction
  `AuthorityState`'s three states exist to preserve. With no anchor configured, every chain
  rooted anywhere is denied.

`version()` comes from a process-monotonic counter incremented on every load, so a §5.3 cache
key recorded against an older view can never satisfy a newer one. It resets on restart, which
is correct: the caches it guards do not survive a restart either, and §5.3 rule 5 requires a
fresh durable read at startup regardless.

## §obligations-discharged

| Obligation | Source | Where |
| --- | --- | --- |
| Call `RevalidationGate::on_epoch_bump` on every admitted bump | `capability/AGENTS.md` §5.3 obligation 2 | `canonical_epoch::SurrealScopeEpochStore::admit_epoch_bump` — the gate is a required `&mut` parameter and the call is inside, so no admission path can skip it |
| Read durable epoch state at startup; a missed notification is a cache miss | §5.3 obligation 4 | `canonical_epoch::DurableAuthorizationView::load`, whose unknown-scope answer is `None`/`Unknown`, never an allow |
| Implement `VerseDagView` over `fe-database` | `branch/AGENTS.md` | `append_store::SurrealVerseDagView` — admission and equivocation from `verified_op_log`, genesis from admitted kind-2 rows (ambiguous genesis answers `None`), replay-verified recorded only by an actually-completed rebuild |

Still open, and not this module's to close: `CacheKey`/`PinnedSession` wiring in `fe-api` and
`fe-network` (§5.3 obligations 1 and 3), and the SPEC-2 authority history that replaces
§interim-manager-authority.

## Conformance tests

SPEC-4 §8 names thirteen tests. Eleven are in
`fe-database/tests/canon_log_materialization_test.rs` under their exact spec names; tests 12
and 13 belong to the pure reduction contract and live in `fe-canonical-log`
(`materialize/traits.rs`), because they are about what a reduction may read, not about storage.
