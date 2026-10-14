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

The canonical tables are appended to `schema.rs` and registered in `apply_all`, but
deliberately **not** added to `ALL_TABLE_NAMES`: SPEC-4 §1.4 makes the verified log the authority
and the SurrealDB projection derivative, so an admin "clear everything" that erased
`verified_op_log` would destroy history no rebuild could recover. The same holds for W4-7's
three durable authority sources (`canon_capability_chain`, `canon_sealed_artifact`,
`canon_scope_key_index`): clearing them silently turns admitted history into un-reopenable
ciphertext and quarantines every later candidate.

## §verified-log

`verified_op_log` stores the exact complete-envelope bytes, base64-encoded, keyed by
`op_id_hex`. Everything else on the row — kind, branch, parents, author key, wall/counter — is a
**durable index derived from those bytes**, present because §2.2 requires enough verified
metadata to find parents without decoding every envelope on every traversal.

`meta_of` therefore never trusts the index columns. It reads the bytes, recomputes
`BLAKE3(bytes)` and refuses a row whose bytes no longer hash to its own `op_id`, then re-runs
`signing::decode_and_admit` over them and projects the envelope. A corrupted index column can
slow a query down; it cannot change what a materializer sees. `scope` and `schema_hash` have no
columns at all for the same reason: they exist only inside the signature preimage.

The re-verification on **read** is not belt-and-braces. Content addressing alone cannot tell a
signed operation from self-consistent garbage, because a forger who writes a row picks both the
bytes and the address they hash to; `op_id_hex` is a name the row chose for itself. Only the
signature makes the row an operation, so every read that produces a `VerifiedEnvelopeMeta`
re-checks it, and a row that fails is unreadable rather than merely suspect. `replay` then adds
a `BLAKE3` re-check at its second read of the same row, which is enough there because the
closure load already verified the signature for that exact address.

### Two doors; the guard is on read

**This section previously claimed "there are exactly two entrances to `verified_op_log`, and
both verify [a signature]," and justified `append_admitted` on the grounds that "a
`VerifiedEnvelopeMeta` only exists downstream of `admit_candidate`." That justification was
false, and so is the fact it argued from.** Since CDL-R1 (2026-10-11) `VerifiedEnvelopeMeta`
is `#[non_exhaustive]`, so no crate outside `fe-canonical-log` can build one by struct literal
(a `compile_fail` doctest on the type pins this; this crate's tests now mint theirs from a
real signed envelope via `verify_from_bytes`). But the unverified projection
`fe_canonical_log::materialize::verified_envelope_meta_from(&CompleteEnvelope, op_id)` is
still `pub` (RUNBOOK 1694d07 §10a residual 5, `CDL-R-ACCEPTED`): any caller holding a decoded-but-unadmitted envelope can mint
one with a chosen `op_id` and call `append_admitted` with it. The seal narrows the forging
door; it does not close it.

- `append_admitted(&VerifiedEnvelopeMeta, bytes)` verifies content-addressing ONLY —
  `Hash32::of(bytes) == meta.op_id` — and writes `meta`'s fields straight into the index
  columns. It trusts its `meta` argument. A caller that hand-builds one can make it write a row
  with attacker-chosen index columns and unsigned bytes.
- `append_received(claimed_op_id, bytes)` is the door for bytes with no admission decision
  behind them, and the one call site that actually runs `signing::decode_and_admit` — canonical
  re-encoding, the §3.2 author binding, the §5.1 signature, the §6 structural rules, and the
  production payload suite — before handing the derived `meta` to `append_admitted`.

The signature guarantee does not live at either append door. It lives on **read**: `meta_of`
re-runs `decode_and_admit` against the stored bytes and re-derives every field from them (see
§verified-log above), so a row written through a forged `meta` is unreadable rather than merely
suspect — forged index columns are ignored, and bytes that do not carry a valid signature are
refused. In production the only caller of `append_admitted` is `admission::admit_and_append`,
which always passes a `decode_and_admit`-derived `meta`, so the property holds today — but by
the read-side mechanism, not by any check the append door itself makes.

`VerifiedLogStore::append`, the trait seam, is a thin wrapper over `append_received`. It used to
reach the log through `CompleteEnvelope::decode_canonical` plus `verified_envelope_meta_from` —
neither of which verifies anything, as the latter's own doc-comment says — which made the trait
an unguarded second door onto the same table. It is not one now.

`append_received`'s BLAKE3 recomputation happens **before any row is read or written**, so a
mismatched claim there cannot touch storage at all; `append_admitted`'s recomputation is the
same content-addressing check, run again on whatever `meta` it was actually given.

Exactly-once is enforced twice: `idx_verified_op_log_op_id` is UNIQUE, so a second row for one
`op_id` is impossible at the storage layer, and the write path read-compares first so a
byte-identical retry returns `AlreadyPresent` (success, §3.2) rather than an error.

## §storage-faults

`VerifiedLogStore::append` returns `Result<AppendOutcome, AppendError>`. Since the W2 erratum
landed, `AppendError` names every outcome honestly: `HashMismatch` and `IntegrityConflict`
(SPEC-4 §3.3, a permanent blacklist of the `op_id`), plus two **indeterminate** variants that
condemn nothing — `NotAnEnvelope` (bytes this build cannot admit here: undecodable, unsigned,
structurally invalid, or a protocol version it predates) and `StorageUnavailable` (the store
could not answer). The trait impl maps `append_received`'s `VerifiedLogStoreError::NotAnEnvelope`
and `::Storage` onto exactly those two; it never answers `IntegrityConflict` for a local
refusal, because that would poison a legitimate identity network-wide and forever.

The **inherent** API (`append_admitted`, `append_received`, `meta_of`, `parents`,
`envelope_bytes`, `op_id_at_equivocation_key`) is still the primary one and returns
`VerifiedLogStoreError` / `StorageError`; everything inside `canon_log` uses it. The trait's
read methods (`get_bytes`/`get_meta`/`parents_of`) still answer bare `Option`, and `None` on a
fault is the pending-safe direction: replay halts the affected head rather than applying an
operation whose history it could not read. Either way the real cause stays drainable —
substrate faults through `take_storage_faults`, refused appends through `take_append_refusals`.

A related fail-open, closed here rather than deferred: `EquivocationIndex::op_id_at` returns
`Option<Hash32>`, and `None` **admits**. Answering `None` on a storage fault therefore lets a
D-CL25 equivocation through exactly when the substrate is sick. `admit_and_append` now requires
its evidence sources to implement `admission::EvidenceAvailability` and refuses with
`AdmissionRejection::EvidenceUnavailable` when a fault was raised while deciding the candidate,
so the fail-open direction is unreachable. That gate is on the `Ok` path only: a genuine §5.1
reject is a decision about the candidate's own bytes and must stay permanent rather than
becoming retryable.

The W4-7 durable sources follow the same rule: a fault is a typed `StorageError` from their
`async` doors, and a stored row that fails its own re-verification on load (bytes that do not
hash to the row's id, a non-canonical chain, key material that does not derive the indexed
`key_id`) is **not held** and is drainable from `take_load_faults` — so an operation that
needed it quarantines as missing evidence, never admits and never rejects.

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
asserted: `store.append_admitted` is unreachable until `admit_candidate_with_facts` returns
`Ok`. `missing_parent`, `unknown_schema`, `unknown_kind` (erratum G1), `opaque_payload`, and
`AuthorizationEvidenceUnavailable` (a capability chain, epoch state, or payload key this
process cannot yet resolve — erratum E-W4-1, owner-confirmed 2026-10-13) surface as
`AdmissionRejection::Quarantined`, because §5 note 2 forbids provisionally applying an
availability state.

Since W4-2 the verifier works on a `VerifiedCandidate` (facts + the verified envelope) rather
than bare facts, and `admit_and_append` takes the caller's `payload_ciphertext: Option<&[u8]>`
and passes it to `verify_payload` untouched; the ciphertext is never appended to
`verified_op_log`. The payload facts the verifier extracts come back on
`AdmittedAppend::payload_facts` and are not persisted here. `replay::replay_to_frontier` has no
decrypted-payload source yet and hands every reduction `None` — see
`fe-canonical-log/src/materialize/AGENTS.md` §payload-facts for why that is a pending state,
not an exclusion.

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

`rebuild_projection` treats a checkpoint as an accelerator and never an authority (§6.2), and it
spends that sentence twice.

**First, an unverified checkpoint cannot reach it.** The parameter is an `AdmittedCheckpoint`,
whose fields are private and whose only constructor — `AdmittedCheckpoint::admit` — runs
`checkpoint::decode_and_admit_checkpoint` (canonical re-encoding plus the SPEC-5 §3.1 rule 4
signature), asks the two §3.2 authority questions through `ManagerPlusAuthorizationView`, and
then derives the §6.3 binding through `compose::checkpoint_binding_for_selection`, which
validates it against the caller's own branch, frontier, segment manifest, and materializer
version. A signature never substitutes for the binding comparison and the binding comparison
never substitutes for the signature.

The parameter used to be a bare `CheckpointBinding`. That type carries no signature at all and
every field in it is derivable by anyone who can see the branch, so **offering one was free**,
and offering one suppressed the empty-state reset. This paragraph previously claimed the
absence of a signature parameter was the safety property; it was the vulnerability.

**Second, an admitted checkpoint still has to reproduce.** The accelerated pass replays the
tail, then the surface's projection root is compared against the root the checkpoint CLAIMS. On
any disagreement — or on a replay that did not resolve every selected head — the projection is
thrown away and rebuilt from empty state, and the refusal is reported in
`RebuildReport::refused_accelerator`. The accelerated attempt necessarily writes its tail before
that comparison can be made (a projection root is a commitment to projected state), and the
empty-state fallback clears exactly those rows, so a refused accelerator leaves no trace.

The empty-state path clears the materializer's own rows through the `ProjectionSurface` seam
and then clears that projection's markers, so §6.1's "no unpublished SurrealDB row may be
required for correctness" is actually exercised rather than assumed. Marker clearing exists for
exactly this one caller; §5 note 4 forbids advancing or retracting a marker by hand to repair a
projection.

`dag_view` is a required parameter because a completed replay is the only admissible evidence
for `VerseDagView::frontier_is_replay_verified`, and `record_replay_verified` is `pub(crate)`
so `rebuild_projection` is the only caller that can exist. **Only the from-empty pass records
it.** A checkpoint-accelerated pass replayed a tail on top of rows this process did not derive
from verified operations, so presenting its frontier as replay-verified would be presenting a
signature as a replay. The report says which happened in `replay_verified_recorded`.

## §epoch-state

`scope_epoch_state` persists `current_epoch` and the admitted bump operation IDs per exact
scope, keyed by the hex of the canonical CBOR scope map — the same bytes the signature covers,
so two peers derive the same key.

`admit_epoch_bump` takes an `op_id` that must already be in the verified log, reads that row
back through `signing::decode_and_admit`, and reads the declared epoch from **that operation's
own signed capability reference**. Nobody can bump an epoch with a number the author never
signed, nobody can bump one with an operation that was never appended, and an unsigned envelope
authorizes nothing here even if it reached the table by some other route.

`authority: &dyn ManagerAuthorityView` is a **required parameter**, and the SPEC-3 §5.1
Manager+ question is asked before any state is read or written. Until this remediation the
function performed no authority check at all and took no parameter that could have answered
one, so any appended kind-4 envelope moved the epoch — which, composed with the unguarded trait
append door above, meant one unsigned envelope could advance an epoch and lock out every
legitimate actor. `AuthorityState::Unknown` refuses exactly as `NotManagerPlus` does: an
unanswerable authority question is never an allow (§5.3 rule 5).

It then enforces §5.1: kind 4 only, exactly one parent, `e -> e + 1` with no skips and no
decreases. The write is a **compare-and-swap** on the epoch the decision was made against
(`WHERE scope_key = $key AND current_epoch = $expected`); a plain `UPDATE` let two concurrent
admissions each read `e`, each write `e + 1`, and the second silently discard the first's
evidence row, which §5.1 rule 6 requires retained. Matching zero rows is
`EpochBumpRejection::ConcurrentModification`, and nothing was changed.

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

**Recipient-side key-wrap issuer check (CDL-R2, 2026-10-11).** `DurableAuthorizationView` also
implements `fe_canonical_log::crypto::IssuerAuthorityView`, so `open_scope_key` has a production
answer to "is this wrap's issuer Manager+ for that epoch scope". It asks the question exactly as
`admit_epoch_bump` asks SPEC-3 §5.1 — `principal_is_manager_plus(issuer, scope, epoch)` against
the role table — and does NOT route through `authority_is_manager_plus`: the wrap's
`CapabilityRef.chain_id` names a certificate chain, not an `issuer_authority_id`, so feeding it
to the anchor check would be a second, invented mapping that refuses every real wrap. The
`chain_id` is not resolved for the wrap check; the signed reference's epoch is checked by
`open_scope_key` itself. (W4-7's `chain_store::ChainStore` now exists; W4-8's
`DurableDeliveryView` uses it for the recipient-decrypt half, not for this issuer half.) `ManagerPlus`→`Granted`,
`NotManagerPlus`→`Refused`, `Unknown`→`Unknown`; both non-`Granted` answers refuse. Test:
`opening_a_wrap_asks_the_durable_view_whether_its_issuer_is_manager_plus`. The
`DeviceEnrolmentView` half is decided (owner R2, 2026-10-13: self-signed, replicated) and the
protocol artifact exists (`fe-canonical-log` `crypto::device_enrolment`, W4-6); its durable
registry is W4-8's `canon_log::device_enrolment`, not yet landed.

**Checkpoint authority (W4-7).** `DurableAuthorizationView` also implements
`checkpoint::ManagerPlusAuthorizationView`, so `rebuild::AdmittedCheckpoint::admit` has a
production answer. `is_manager_plus_for_checkpoint` asks `principal_is_manager_plus` at the verse
scope, exactly as above. `capability_permits_append_checkpoint` resolves the claim's
`capability.chain_id` in the `ChainStore` handed to `load_with_chain_store` (`load` keeps no
store and answers `false`) and runs SPEC-3 §2.4 for `append/checkpoint` with the signer as
requester under the live policy — so holding someone else's valid chain is not a capability.
`authorization_view_root` answers `None`: no authorization-view root is derivable until SPEC-2's
authority history exists, and `verify_checkpoint_claim` treats `None` as "cannot derive",
never as a match.

`version()` comes from a process-monotonic counter incremented on every load, so a §5.3 cache
key recorded against an older view can never satisfy a newer one. It resets on restart, which
is correct: the caches it guards do not survive a restart either, and §5.3 rule 5 requires a
fresh durable read at startup regardless.

## §durable-authority-sources

W4-7 gives every seam the composed verifier reads a durable, local-only implementor. Each one is
a **startup snapshot plus write-through**: the `fe-canonical-log` seams are synchronous and the
DB thread is async, so a source loads its table once (re-verifying every row from its own bytes)
and answers from memory; its `async` doors write the table first, then the cache. Clones of the
chain and artifact stores share one cache.

| Source | Table | Seam(s) | Write door |
| --- | --- | --- | --- |
| `chain_store::ChainStore` | `canon_capability_chain` | `CapabilityChainSource` | `put_verified_chain` — canonical round-trip of the chain and every certificate, `chain_id` recomputed, never caller-named |
| `artifact_store::SurrealArtifactStore` | `canon_sealed_artifact` | `ContentByteStore`, `PayloadCiphertextSource` | `put_payload_ciphertext`, `put_sealed_artifact` (SPEC-6 §5.1 id + immutable-rewrite checks first); `ContentByteStore::put` queues for `persist_pending` |
| `scope_key_custody::SecretStoreScopeKeyCustody` | `canon_scope_key_index` (non-secret) | `ScopeKeyStore` + `ShredLedger` | `take_custody`, `shred`; trait mutations queue for `persist` |
| `author_lifecycle::DurableAuthorKeyLifecycle` | none — derived from `verified_op_log` | `AuthorKeyLifecycleView` | `observe_appended` after each append; `replay_verified_log` at startup |

**Local-only, never served.** Chains are access-controlled local data (SPEC-3 privacy table):
no `replicate_row`, no fe-api route. The artifact store is deliberately not the iroh blob store,
whose content is served to peers — that is the canonical network exposure that stays NO_GO.
Nothing here is reachable from a live handler (§parallel-and-dormant).

**Scope keys never enter SurrealDB.** Material lives in `fe_identity::SecretStore` under service
`fractalengine`, account `canon-scope-key/<key_id hex>`, and in the custody's zeroizing map.
`SecretStore` cannot enumerate and must never gain `list` (critic C8/C10), so SurrealDB keeps a
non-secret index — scope, epoch, `key_id`, state. On load a key is held only when its material
is present AND derives the indexed `key_id`. Write order is chosen so a crash fails closed:
insert files the secret before the index row (worst case an unindexed, never-loaded secret —
"key lost"), and shred writes `destroyed` before deleting the secret (worst case a residual
secret that is never reloaded and is deleted again on the next load — never "key resurrected").
A destroyed or issuance-stopped `(scope, epoch)` refuses re-keying (`IssuanceStopped`). The hex
text handed to and from the keystore is overwritten before it is freed; fe-database has no
`zeroize` dependency, so that wipe is best-effort.

**The lifecycle is derived, not stored.** `DurableAuthorKeyLifecycle` holds the
`LifecycleDispatcher` and a causal DAG of every `verified_op_log` row. Replay walks rows in
`deterministic_causal_order`; every row joins the DAG (everything in the verified log is
admitted), and a row whose schema is one of the four SPEC-2 lifecycle schemas is re-opened and
dispatched. Re-opening runs `admit_candidate_with_facts` with a private `StoredOperationOpener`
that delegates ingress and payload to the real verifier and answers authorization from the
row's presence in the verified log — the same trust `replay_to_frontier` places in it. Only
bytes read back under their own `op_id` ever reach it. A lifecycle row that entered through the
unauthorized `append_received` door is therefore dispatched too; that is bounded rather than
an escalation, because a rotation needs the predecessor's own envelope signature AND the
successor's possession proof, so it can only retire its own signer's key, and a disavow or
grant is still judged by `issuer_role` against the role table. A lifecycle row that cannot be re-opened
(its payload key was shredded, its ciphertext was never retained) is reported in
`LifecycleReplayReport::held` and does not move the lineage, which is SPEC-2's own quarantine
of a payload-key-unavailable lifecycle operation.

Two interim answers SPEC-2 forces, both to be replaced with SPEC-2's authority history:

- **`CapabilityAuthorityView::predecessor_authorized`** answers `Authorized` only for exactly
  the dispatched operation's own `(author key, scope, epoch)`, whose append chain the composed
  verifier verified before the append; any other question is `Unauthorized`. `issuer_role`
  reads the interim role table (`DurableAuthorizationView::role_level`); no row is
  `Unavailable`, which SPEC-2 holds rather than refuses. Replay reads today's roles, not the
  roles at the time of the original admission.
- **Lineage roots.** `LineageIndex` treats a key as active only via an incoming rotation or a
  registered root, so with no roots every first rotation fails `PredecessorNotActive`. Before a
  rotation is dispatched its author is registered as a root at the operation's verse — its
  authority there rests on the chain admission just verified (SPEC-2 §4 rule 2). Root
  registration cannot un-retire a key: `active_key_at` checks the rotation walk first.

## §production-verifier

`verifier::durable_candidate_verifier(db, secret_store, interim_authority_anchor)` is THE one
production `ComposedCandidateVerifier`: `ChainStore`, `DurableAuthorizationView` (with the chain
store), `DurableAuthorKeyLifecycle`, `SecretStoreScopeKeyCustody`, `SurrealArtifactStore`,
`RegisteredSchemaSet`, `SystemClock`. It returns a `LoadedVerifier` carrying the startup
`LifecycleReplayReport` so a held lifecycle row is visible, not silent. Like
`DurableAuthorizationView`, it is a snapshot: rebuild it after an admitted epoch bump or a role
change.

`verifier::admit_and_append_durably` is its admission door: `admit_and_append` (SPEC-4 §2 order,
unchanged), then the payload ciphertext retained in the artifact store, then
`observe_appended`, so the next admission already sees a rotation this one recorded. Calling
`admit_and_append` directly with this verifier is legal but leaves the lifecycle stale until the
next rebuild, and leaves the ciphertext unretained for replay. A failure after the append is
`CiphertextNotRetained` / `LifecycleNotObserved`, both of which the caller fixes by retrying the
same call — the append answers `AlreadyPresent`.

`RegisteredSchemaSet` interprets exactly the four SPEC-2 lifecycle schemas (kind 1, each
validated by its own canonical decoder) until something calls `register(kind, schema_hash,
validator)` — on a built verifier too (`verifier.parts().schemas.register(...)`), so CDL-SPEC10
B3 adds kind 5 by a call, not by editing the constructor (critic C3). A repeat registration is
refused, never substituted. An application kind with no schema is `UnknownKind`; a known kind
with an unregistered schema is `UnknownSchema` (SPEC-1 §6 rule 7, SPEC-4 §5). The four lifecycle
schema hashes are provisional — `BLAKE3("fe-schema-<name>" || 00)` after SPEC-10 §4.1 rule 3 —
and recorded in `fe-canonical-log/src/AGENTS.md` §Provisional wire numbering.

Tests: `fe-database/tests/canon_log_composed_admission_test.rs`, through the real DB thread's
shared handle over an on-disk store that is joined and reopened.

## §obligations-discharged

| Obligation | Source | Where |
| --- | --- | --- |
| Call `RevalidationGate::on_epoch_bump` on every admitted bump | `capability/AGENTS.md` §5.3 obligation 2 | `canonical_epoch::SurrealScopeEpochStore::admit_epoch_bump` — the gate is a required `&mut` parameter and the call is inside, so no admission path can skip it |
| Read durable epoch state at startup; a missed notification is a cache miss | §5.3 obligation 4 | `canonical_epoch::DurableAuthorizationView::load`, whose unknown-scope answer is `None`/`Unknown`, never an allow |
| Ask the SPEC-3 §5.1 Manager+ question on every epoch bump | SPEC-3 §5.1 | `canonical_epoch::SurrealScopeEpochStore::admit_epoch_bump` — `authority: &dyn ManagerAuthorityView` is a required parameter and the question is asked before any state is touched |
| Implement `VerseDagView` over `fe-database` | `branch/AGENTS.md` | `append_store::SurrealVerseDagView` — admission and equivocation from `verified_op_log`, genesis from admitted kind-2 rows (ambiguous genesis answers `None`), replay-verified recorded only by an actually-completed **from-empty** rebuild, through a `pub(crate)` setter |
| Give `CandidateVerifier`, `ManagerPlusAuthorizationView`, `AuthorKeyLifecycleView` and scope-key custody a durable production implementor | campaign plan W4-7 / ledger CDL-W4 | §durable-authority-sources, §production-verifier |

Still open, and not this module's to close: `CacheKey`/`PinnedSession` wiring in `fe-api` and
`fe-network` (§5.3 obligations 1 and 3), and the SPEC-2 authority history that replaces
§interim-manager-authority and the interim lifecycle answers in §durable-authority-sources.

## Conformance tests

SPEC-4 §8 names thirteen tests. Eleven are in
`fe-database/tests/canon_log_materialization_test.rs` under their exact spec names; tests 12
and 13 belong to the pure reduction contract and live in `fe-canonical-log`
(`materialize/traits.rs`), because they are about what a reduction may read, not about storage.
