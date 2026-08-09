# Canonical log branch, checkpoint, and retention state machine v1

**Status:** Draft — owner approval required before implementation.

This document defines branch selection, signed checkpoint claims, bounded
quarantine, and local retention obligations for the Canonical Fractal Data Log.
It implements D-CL1, D-CL2, D-CL4, and D-CL12. It depends on the operation
artifact in [operation-envelope.md](operation-envelope.md), authorization in
[capabilities-and-revocation.md](capabilities-and-revocation.md), and the
projection contract in
[log-first-materialization.md](log-first-materialization.md).

It does not authorize network wiring or choose commercial, legal, or
user-facing retention durations. The D-CL17 encryption and scope-key lifecycle
is fixed by SPEC-3 section 10; implementation remains gated as stated below.

## 1. Conformance vocabulary and invariants

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **branch** is a selected causal history within one verse DAG. A verse has
   one branch registry; it MUST NOT create independent petal DAGs. Every
   operation still carries its verse/petal/resource scope as required by
   SPEC-1.
3. A **frontier** is the strictly byte-sorted, duplicate-free set of admitted
   operation IDs in a branch with no admitted child in that branch's selected
   causal history. A frontier is a set, not an arrival-order winner.
4. A **branch selection** is a branch ID, its frontier, and the causal closure
   of that frontier. A projection is derived only from a branch selection that
   is eligible under SPEC-4. It MUST NOT be derived from a mutable SurrealDB
   row, relay inventory, or local receipt order.
5. The **header plane** contains only payload-free operation headers and
   supporting branch/checkpoint metadata. Under D-CL2 it is available
   verse-wide to authorized verse members. The **payload plane** contains
   scope-encrypted, petal-affine payloads, snapshots, segments, shards, tiles,
   and assets. Fetching either plane remains subject to SPEC-3 capability
   checks.
6. A branch mode changes selection and materialization behavior; it MUST NOT
   expand the caller's `fetch`, `decrypt`, `materialize`, `append`, or `seed`
   authority, and it MUST NOT bypass epoch revocation.
7. A local store MAY retain ciphertext or header evidence that it cannot
   decrypt. Possession does not make an artifact admitted, materialized,
   checkpoint-valid, or safe to disclose.
8. A branch control record, checkpoint, lease, or garbage-collection action
   MUST NOT rewrite an immutable operation, change its `op_id`, remove a DAG
   edge, or turn a rejected/quarantined candidate into an admitted operation.

## 2. Branch state machine

### 2.1 Modes

| Mode | Required behavior | Prohibited behavior |
| --- | --- | --- |
| `tracking` | Maintain the verified branch frontier as new eligible operations arrive; deterministically materialize the authorized, complete causal closure under SPEC-4. Concurrent heads remain a frontier. | Choosing a head by receipt time, local wall clock, signer preference, or a mutable database row. |
| `paused` | Retain the declared branch selection and MAY receive/retain authorized headers, ciphertext, and manifest evidence for a later resume. It MUST NOT advance the branch's exposed materialized projection or emit a commit/analytics position for newly received operations. | Treating receipt, partial payload access, or a later resume as an implicit materialization of the paused history. |
| `detached` | Pin an explicit branch selection and materialize only that immutable causal closure. It never follows later tracking updates automatically. | Advancing to a different frontier, or making detached work visible on a tracking branch without the explicit multi-parent reintegration operation. |

1. A `tracking` branch that encounters a missing parent, unknown schema,
   unauthorized operation, invalid payload, opaque required payload, or failed
   materialization MUST follow the SPEC-4 error disposition. It MUST NOT claim
   a complete projection beyond the unresolved point.
2. A `paused` branch MAY advance its *received* evidence frontier for later
   inspection, but its materialized frontier remains the last explicitly
   committed selection. A resumed branch MUST run normal admission and
   deterministic replay; it MUST NOT apply queued data in arrival order.
3. A `detached` branch selection is immutable once pinned. Creating another
   detached view requires another explicit selection; replacing its frontier in
   place is prohibited.
4. Detached work may re-enter a tracking branch only through the SPEC-1
   `detached-to-tracking merge` operation (`operation_kind = 3`). The operation
   MUST meet SPEC-1's multi-parent rule and D-CL12's deterministic CRDT
   reduction rule. A UI mode toggle, database copy, or relay acknowledgement
   is not a merge.
5. Transitions from `tracking` to `paused` and from either mode to `detached`
   record the exact selected frontier. A transition to `tracking` is eligible
   only after the target selection is fully re-evaluated under SPEC-4.

### 2.2 D-CL19 frontier and branch-control contract

1. A branch's `frontier_commitment` is
   `BLAKE3(op_id[0] || op_id[1] || ... || op_id[n-1])`, where the non-empty
   head list is sorted lexicographically by complete 32-byte `op_id`. The list,
   not an effective/synthetic head, is authoritative. A checkpoint stores this
   digest and verifies it against the exact selected frontier.
2. A multi-head tracking frontier may checkpoint, bootstrap, compact, and GC
   when its manifest, authorization view, materializer result, replay proof,
   lease, tombstone, and retained bootstrap-path obligations all verify. It
   MUST NOT collapse, omit, or receipt-order the concurrent heads.
3. Branch create, pause, retarget, and detach are verse-scoped normal intent
   operations (`operation_kind = 1`) with a registered branch-control schema.
   Their header scope is exactly `(verse_id, null, null)` and their author MUST
   hold a current Manager+ `append/op` capability that permits the schema and
   operation kind. A UI action, relay acknowledgement, or mutable row is not a
   branch control operation.
4. The decrypted branch-control payload is a deterministic-CBOR map with four
   fields: `{0: action, 1: target_branch_id, 2: selected_frontier,
   3: source_branch_id}`. `action` is `0 = create`, `1 = pause`,
   `2 = retarget`, or `3 = detach`; `selected_frontier` is a strictly sorted,
   duplicate-free non-empty `Hash32` array; and `source_branch_id` is `null`
   only for create. Create requires a matching admitted `branch_genesis`;
   pause records the target's current selected frontier; retarget adopts the
   supplied replay-verified frontier; detach creates an immutable target
   selection from the supplied source. Every referenced branch and frontier
   member must be in the same verse DAG.
5. The control operation itself is part of the immutable verse history. Its
   materialized registry effect is derived only after normal admission and
   deterministic replay; failed or unauthorized control operations do not
   alter a branch selection.

## 3. Signed, replay-verifiable checkpoints

### 3.1 Checkpoint claim artifact

1. A checkpoint is an accelerator claim under D-CL4, never a source of truth.
   Its canonical claim uses the deterministic CBOR profile and identifier
   types from SPEC-1. A snapshot is separate scope-encrypted payload-plane
   data; plaintext snapshot bytes MUST NOT appear in a checkpoint claim.
2. A V1 claim MUST bind at least the following values:

   | Binding | Requirement |
   | --- | --- |
   | `checkpoint_version` | MUST be `1`. |
   | `verse_id`, `branch_id` | Identify one verse and one registry branch. |
| `frontier_commitment` | `BLAKE3` over the exact lexicographically sorted selected-frontier `op_id` list. |
   | `segment_manifest_id` | Identifies the immutable manifest whose closure is asserted to cover the selection. |
   | `materializer_id`, `materializer_version` | Identify the exact deterministic projection rules and schema interpreter set. |
   | `projection_root_hash` | BLAKE3 commitment to the canonical projection/export produced by those rules. |
   | `authorization_view_root` | Commitment to the epoch-bump and authority facts used for replay; a replay that derives another root rejects the claim. |
   | `snapshot_manifest_id` | `null` or the immutable manifest of scope-affine encrypted snapshot components. A non-null value is an accelerator only. |
   | `signer`, `capability`, `issued_hlc` | Canonical principal, SPEC-1 capability reference, and explicit HLC evidence. |

3. The unsigned checkpoint claim is the deterministic-CBOR map containing all
   bindings in rule 2, with no unknown or duplicate fields. Its V1 field
   numbers are registered with the implementation package; its canonical
   frontier encoding is the D-CL19 digest in section 2.2.
4. Its signature preimage is

   ```text
   ASCII("fe-checkpoint-v1") || 00 || unsigned_checkpoint_claim
   ```

   The signature is deterministic Ed25519 by `signer.public_key`; the derived
   `checkpoint_id` is BLAKE3 of complete canonical claim bytes including the
   signature. The DID/public-key binding rules are the same as SPEC-1.
5. The signer MUST be Manager+ in the authorization view reconstructed for the
   selected history and MUST present an unexpired capability with
   `append/checkpoint` for the exact verse scope. A relay with only `seed`, or
   a materializer with only `materialize`, cannot create a valid checkpoint.
6. A claim MAY have `snapshot_manifest_id = null`. A snapshot component, when
   present, MUST be scope-affine, encrypted under that component's scope key,
   and packed separately from other payload scopes. A verse-wide plaintext or
   single petal-crossing encrypted snapshot is forbidden by D-CL1 and D-CL2.

### 3.2 Verification and use

1. A consumer MUST decode and re-encode a checkpoint claim byte-for-byte,
   verify its BLAKE3 ID, DID/key binding, signature, Manager+ authority,
   capability, expiration, scope epoch, and all bindings before considering it
   as an accelerator.
2. The consumer MUST verify the segment-manifest reachability proof specified
   by SPEC-6 and must derive the selected causal closure from immutable
   operation IDs, not from a relay's asserted inventory.
3. A consumer with authorized payload access for every scope represented by
   the selection MUST be able to replay from an earlier verified checkpoint or
   empty projection, derive the same authorization-view and projection roots,
   and compare them with the claim. This is the D-CL4 replay-verification
   requirement.
4. A consumer authorized for only a petal/resource payload may verify the
   claim's canonical header, reachability proof, and its own snapshot/payload
   components, but MUST NOT present the whole-verse checkpoint as independently
   replay-verified. It remains an untrusted accelerator for inaccessible
   scopes.
5. A signature or Manager+ status never repairs a mismatch. On a manifest,
   authorization, materializer, snapshot, or replay mismatch, the consumer
   MUST reject the checkpoint, retain verified log history, and rebuild from
   an earlier verified checkpoint or empty state as required by SPEC-4.
6. A checkpoint does not authorize a nonconforming peer to erase source
   operations. GC eligibility is separately constrained by section 5, including
   replay, lease, tombstone, and retained-bootstrap obligations for every head
   in a concurrent frontier.

## 4. Bounded pre-admission quarantine

1. A candidate with `missing_parent` or `unknown_schema` is stored, if at all,
   in a **pre-admission quarantine** outside the verified log, projection,
   branch frontier, checkpoint closure, and analytics input. `opaque_payload`
   is not a reason to reinterpret or promote a header; it follows SPEC-4's
   header/payload rules.
2. A quarantine record MUST retain the exact candidate bytes, claimed/derived
   identifier, reason, first-seen time, last-validation time, required parent
   IDs or schema hash, and bounded provenance diagnostics. It MUST NOT contain
   an invented projected value or inferred parent edge.
3. An implementation MUST persist local bounds for entry count, total bytes,
   maximum record age, maximum parent-depth walk, and retry cadence. When any
   bound is reached, it MUST decline further quarantine admission with an
   explicit resource-exhausted result rather than evict an admitted artifact
   or silently materialize a candidate.
4. Expiry or capacity eviction removes only the local quarantine copy. It MUST
   NOT emit a tombstone, alter a branch frontier, produce a negative
   availability claim, or imply that the candidate was invalid.
5. A missing-parent candidate may be re-evaluated only after every parent in
   its full transitive closure is locally admitted. An unknown-schema candidate
   may be re-evaluated only after the exact `schema_hash` and deterministic
   interpreter are available. Receipt of a similarly named schema, an unsigned
   schema, or an arrival-order guess is insufficient.
6. Invalid, unauthorized, or payload-invalid candidates follow SPEC-4's reject
   disposition, not this retry quarantine. A relay or later child reference
   cannot promote them.
7. The actual values for the bounds in rule 3 are local operational policy,
   not canonical semantics. Each supported device/relay profile records them
   before enablement; changing a bound MUST NOT change the verified log or the
   deterministic result for a complete admissible history.

## 5. Retention, leases, and garbage collection

### 5.1 Retention planes and common rules

1. Retention is local possession policy. It does not alter canonical history,
   grant access, or prove that another peer has a copy.
2. A retention participant MUST distinguish: verified headers; encrypted
   payload segments and snapshot components; segment manifests and reachability
   proofs; authorization/identity evidence; signed checkpoints; quarantine;
   and local cache data. A storage report MUST identify which category it
   covers.
3. Under D-CL2, header retention is verse-wide among authorized members while
   payload retention is petal/resource scoped. Segment and snapshot packing
   MUST remain scope-affine; a peer without a payload capability MUST NOT be
   required to retain or decrypt another petal's payload merely to retain
   causal-header evidence.
4. No participant may advertise a checkpoint, branch bootstrap, or `seed`
   commitment for an artifact it has discarded or cannot serve under a valid
   capability check.
5. A multi-head tracking frontier may use a checkpoint or compacted branch
   summary only when the checkpoint binds the exact sorted-frontier hash and
   every retained bootstrap path proves the complete selected closure.

### 5.2 Storage-role obligations

| Role | Minimum obligation | Explicit non-obligation |
| --- | --- | --- |
| Mobile peer | Retain the selected branch registry state, active frontier evidence, authorization facts needed to revalidate local capabilities, and authorized recent payload/snapshot/tail data for its declared interest. It MUST retain enough header/manifest evidence to detect a missing causal closure rather than invent a current state. | It is not an archive and need not retain unrelated petal payload ciphertext or serve after local eviction. |
| Relay/seeder | Retain exactly the immutable artifacts covered by an active accepted GC lease, their address-to-bytes bindings, and enough lease/capability evidence to serve only an authorized `fetch` requester. It MUST durably acknowledge a lease before advertising that lease's availability. | It is not a log authority, materializer, decryptor, or source of a valid checkpoint merely because it has bytes. |
| Archive | Retain the complete authorized header plane, encrypted payload/snapshot/segment artifacts, manifests, authorization and identity evidence, and checkpoints for each accepted retention domain, so it can replay or serve an authorized historical selection. | It cannot promise physical deletion of copies it never controlled or plaintext already disclosed to another principal. |

1. A mobile peer may release old payload ciphertext after it has an eligible,
   authorized checkpoint/snapshot/tail path for its declared selection, subject
   to D-CL19 and section 5.4. It MAY retain headers longer than payloads.
2. A relay MAY refuse a lease before acknowledgement for capacity, policy, or
   authorization reasons. After acknowledgement, it MUST either meet the lease
   or report a durable lease-failure state; silent eviction is prohibited.
3. An archive that accepts a full-history domain MUST retain the supporting
   authorization and manifest evidence as well as payload bytes. Retaining only
   a projection database or only an unproven checkpoint is not archival
   retention.

### 5.3 GC leases

1. A **GC lease** is an authenticated, time-bounded retention commitment from
   a holder for an immutable artifact set. Its descriptor MUST bind a lease
   identifier, holder principal, authorized issuer, verse/scope, artifact-set
   commitment or manifest root, issue/expiry times, and any legal-hold or
   replacement condition.
2. Accepting or renewing a lease requires `seed` authority for every covered
   object class and scope. A holder MAY retain ciphertext without `decrypt`,
   but it MUST validate the requester's authorization before serving bytes.
3. An artifact is locally GC-eligible only when it is outside every active
   lease, legal hold, selected branch/replay requirement, and tombstone
   retention requirement. Eligibility permits local reclamation; it does not
   authorize deleting another peer's copy or asserting global erasure.
4. Expiry removes the holder's future retention guarantee. It does not by
   itself establish a safe handoff, erase ciphertext, or remove the artifact
   from verified history.
5. Lease serialization, renewal authority, handoff quorum, failure reporting,
   storage quotas, and duration defaults are owner-approved operational policy.
   SPEC-6 MUST define the artifact-set/reachability proof without granting
   relay authority over history.

### 5.4 Tombstones and sparse history

1. A tombstone operation and its causal evidence MUST remain available for as
   long as any retained branch bootstrap, checkpoint, manifest, or replay tail
   can include operations whose materialized effect it suppresses. This lifts
   D-CL12's tombstone non-resurrection rule into retention behavior.
2. A participant MAY compact a tombstone only after a replay-verifiable
   checkpoint proves its effect and all remaining retained bootstrap paths
   include that proof. It MUST NOT serve an older ancestor path as a complete
   current history without the suppressing tombstone evidence.
3. Removing a deleted object's payload ciphertext is independent of retaining
   its tombstone/header. Header retention may continue to expose the metadata
   residuals documented by SPEC-3; it MUST contain no payload plaintext.
4. The age at which tombstones, historical payloads, snapshots, and audit
   evidence may be released is owner-approved operational policy. It must
   account for offline members, legal holds, forensic needs, archive
   commitments, and the ability to replay a declared analytics source identity.

### 5.5 Crypto-shredding semantics

1. D-CL1 requires scope-key encryption from the first admitted payload or
   segment. There is no retrofit path that can make already seeded plaintext
   segments erasable.
2. The strongest supported erasure claim is **crypto-shredding**: destroy the
   relevant current and retired scope-key material in controlled key stores,
   stop future authorized delivery, and prevent reissue under the applicable
   epoch/rekey policy. The immutable ciphertext, headers, hashes, manifests,
   traffic records, and adversarial copies may remain.
3. Crypto-shredding does not revoke plaintext previously decrypted by a
   principal, remove a copied key, guarantee deletion from an uncontrolled
   peer, or conceal historical metadata. It MUST NOT be described as physical
   deletion or as a guarantee against a prior recipient.
4. Scope keys are 32-byte CSPRNG values, recipient-device wrapped under the
   D-CL17 X25519 construction, and rotate on each epoch bump. Crypto-shredding
   destroys controlled current/retired scope keys and prevents future wraps; it
   cannot revoke plaintext or uncontrolled key copies.

#### 5.5.4 Scope-key destruction workflow

1. A controlled key store MUST index current and retired scope keys, recipient
   wraps, and topic-key derivation context by exact scope and epoch. It MUST
   destroy those local key records before reporting crypto-shredding complete.
2. An issuer MUST first stop reissue for the affected scope/epoch, then destroy
   its scope key and ephemeral wrap secrets, then remove controlled wraps. It
   MUST record the destruction disposition without recording the destroyed key
   bytes. A device that has not destroyed an already copied key cannot make an
   erasure claim on behalf of the system.
3. On member removal, a scope epoch bump and an e+1 key rotation precede any
   future delivery. Retired e keys remain decryptable only to controlled stores
   that retain them for the approved historical policy; destroying them is a
   crypto-shredding action, not a physical deletion of their ciphertext.

## 6. Required conformance tests

A conforming implementation MUST provide deterministic fixtures and tests with
at least the following names and outcomes.

1. **`tracking_preserves_sorted_concurrent_frontier`** — concurrent admitted
   heads remain a sorted set under every arrival permutation; no receipt-order
   winner is selected.
2. **`paused_receipt_never_advances_materialized_projection`** — a paused
   branch can retain later evidence but exposes neither a newer projection nor
   a committed analytics position until deterministic resume replay succeeds.
3. **`detached_selection_is_immutable_and_reintegration_is_explicit`** — a
   detached selection does not follow tracking updates and reaches tracking
   only through a valid multi-parent kind-3 operation.
4. **`multi_head_tracking_checkpoint_and_gc_bind_sorted_frontier`** — a
   multi-head tracking frontier checkpoints and compacts only when the exact
   sorted-frontier hash, closure, replay proof, and bootstrap obligations hold.
5. **`checkpoint_claim_is_canonical_and_domain_separated`** — canonical bytes,
   DID/key binding, `fe-checkpoint-v1` signature preimage, and BLAKE3
   `checkpoint_id` reproduce byte-for-byte.
6. **`checkpoint_binds_frontier_manifest_materializer_and_authorization`** —
   changing a frontier, manifest, materializer identity/version, authorization
   root, or projection root invalidates checkpoint use.
7. **`checkpoint_requires_manager_plus_and_checkpoint_capability`** — an
   ordinary editor, expired chain, stale epoch, relay seed capability, or
   materializer-only capability cannot create a valid checkpoint.
8. **`checkpoint_replay_mismatch_is_not_authoritative`** — a valid signature
   with an incomplete manifest, changed materializer result, or mismatched
   authorization root is rejected and the projection rebuilds from verified
   history.
9. **`scope_affine_checkpoint_snapshots_preserve_sparse_access`** — a
   petal-authorized peer can validate/fetch only its component and cannot
   decrypt or claim whole-verse replay verification from another petal's
   snapshot.
10. **`missing_parent_and_unknown_schema_stay_pre_admission`** — both cases
    remain outside the verified log, frontier, checkpoints, analytics, and
    projection until their exact prerequisites arrive.
11. **`quarantine_bounds_fail_closed_without_history_mutation`** — count,
    bytes, age, and depth exhaustion return an explicit resource result and
    never evict admitted history or create a materialized no-op.
12. **`lease_requires_seed_authority_and_durable_acknowledgement`** — a relay
    cannot advertise covered data before durable acceptance, cannot serve to
    an unauthorized fetch requester, and cannot turn seeding into append or
    decrypt authority.
13. **`gc_preserves_tombstone_non_resurrection`** — every retained bootstrap
    path that can reach a deleted object also carries replay-verifiable
    suppression evidence; ancestor-only replay cannot resurrect it.
14. **`mobile_release_requires_verified_bootstrap_path`** — a mobile peer that
    releases payload or header data cannot claim bootstrap/seed availability
    unless its retained checkpoint, manifests, frontier, and authorized tail
    prove recovery under the active policy.
15. **`crypto_shredding_preserves_ciphertext_but_revokes_future_key_access`**
    — after a ratified key-destruction workflow, ciphertext/header evidence is
    not misrepresented as deleted while controlled key resolution and future
    delivery fail.

## 7. Design notes

- **A frontier is evidence, not a tie-break:** CRDT reduction can converge the
  projection of concurrent operations without pretending that one concurrent
  operation causally supersedes the others. D-CL19 exists because a checkpoint
  must commit to that distinction before it can make compaction safe.
- **Checkpoints accelerate rather than bless:** Manager+ signatures make a
  checkpoint discoverable and accountable; immutable operations, manifest
  reachability, authorization replay, and deterministic materialization make
  it trustworthy.
- **Headers support causality; payloads support privacy:** Verse-wide header
  availability enables a single causal DAG. Scope-affine encrypted payloads
  and snapshots keep storage and decryption selective. This deliberately does
  not hide all timing or collaboration-graph metadata from an authorized verse
  member.
- **Retention is an availability promise, not deletion magic:** A lease can
  make a holder accountable for bytes it agreed to preserve. It cannot prove
  global replication, remove an adversarial copy, or make a relay a history
  authority.

## 8. Remaining implementation gates

1. Retention durations, quotas, legal holds, archive acceptance, and any
   user-facing deletion promise remain owner-approved operational policy. They
   do not change the D-CL17 cryptographic contract or the D-CL19 frontier rule.
2. Workstream G remains blocked. No branch control, checkpoint, retention,
   crypto-shredding, relay, Iroh, peer-replication, or seeding implementation
   is authorized until the owner approves the complete SPEC set and a separate
   implementation/operations package.
3. **Retention policy:** Ratify retention durations/quotas, legal-hold and
   audit rules, lease issuer/renewal/handoff requirements, archive acceptance
   boundaries, mobile recent-window expectations, and the exact wording of
   any user-facing deletion or erasure promise. These values affect product,
   cost, privacy, and compliance; this specification intentionally chooses
   none of them.
