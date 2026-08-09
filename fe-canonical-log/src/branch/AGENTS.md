# branch + checkpoint (SPEC-5 §2 and §3)

Normative source: `docs/spec/canonical-log/branches-checkpoints-retention.md`, with the
consumer-side rebuild contract in `docs/spec/canonical-log/log-first-materialization.md` §6.
This directory owns the branch state machine, the branch-control payload, and the verse branch
registry; `src/checkpoint.rs` owns the signed checkpoint claim that commits to a selection.
Quarantine, leases, and tombstone retention (§4, §5) belong to `src/retention/`.

## Provisional wire numbering

D-CL24 requires every wire number this crate invents to be recorded for later owner
ratification. **No cross-implementation interop is claimed for a provisional row.**

| Artifact | Field | Key | Status |
| --- | --- | --- | --- |
| branch-control payload | `action` | 0 | Normative — §2.2 rule 4 |
| branch-control payload | `target_branch_id` | 1 | Normative — §2.2 rule 4 |
| branch-control payload | `selected_frontier` | 2 | Normative — §2.2 rule 4 |
| branch-control payload | `source_branch_id` | 3 | Normative — §2.2 rule 4 |
| checkpoint claim v1 | `checkpoint_version` | 0 | **Provisional** |
| checkpoint claim v1 | `verse_id` | 1 | **Provisional** |
| checkpoint claim v1 | `branch_id` | 2 | **Provisional** |
| checkpoint claim v1 | `frontier_commitment` | 3 | **Provisional** |
| checkpoint claim v1 | `segment_manifest_id` | 4 | **Provisional** |
| checkpoint claim v1 | `materializer_id` | 5 | **Provisional** |
| checkpoint claim v1 | `materializer_version` | 6 | **Provisional** |
| checkpoint claim v1 | `projection_root_hash` | 7 | **Provisional** |
| checkpoint claim v1 | `authorization_view_root` | 8 | **Provisional** |
| checkpoint claim v1 | `snapshot_manifest_id` | 9 | **Provisional** |
| checkpoint claim v1 | `signer` | 10 | **Provisional** |
| checkpoint claim v1 | `capability` | 11 | **Provisional** |
| checkpoint claim v1 | `issued_hlc` | 12 | **Provisional** |
| checkpoint claim v1 | signature | 13 | **Provisional** |

§3.1 rule 2 gives the bindings as a prose table with no integer keys and rule 3 defers them to
"the implementation package", so the checkpoint keys are assigned in that table's row order,
flattening the rows that name two or three values. The signature sits one past the last
unsigned key, mirroring the operation envelope's key 10. The branch-control keys are quoted
verbatim from §2.2 rule 4 and are not ours to renumber.

The crate-root `src/AGENTS.md` register indexes this table under "Provisional wire numbering";
the rows here remain the source of truth for the numbers.

## The three modes are three different answers to "what may move"

- **Tracking** may move its materialized frontier. `BranchRegistry::advance_tracking_frontier`
  computes the new frontier as *old heads minus this operation's parents, plus this operation*.
  That is a set operation with no reference to receipt order, wall clock, or signer, which is
  what §2.1 forbids choosing by. It deliberately does **not** require a parent to be a current
  head: a late-arriving sibling of an already-absorbed head is a legitimate concurrent head and
  must widen the frontier. Causal completeness is the SPEC-4 admission layer's obligation, and a
  missing parent is a quarantine reason there, not a frontier repair here.
- **Paused** may move only `received_frontier`. The materialized frontier — and therefore every
  exposed projection and committed analytics position — stays at the last committed selection
  until `resume_tracking` succeeds against a replay-verified frontier. Two separate frontiers
  are what make "we have the bytes" and "we have committed to them" different states; one
  frontier plus a mode flag would let a receipt silently become a commit.
- **Detached** may move nothing. `BranchRecord`'s fields are private and every registry entry
  point that could rewrite a selection refuses a detached record, so §2.1 rule 3 immutability is
  a property of the type rather than a rule callers are asked to remember. Detached work
  re-enters a tracking branch only by advancing the tracking frontier with an admitted kind-3
  merge whose parents name both heads; admissibility and CRDT reduction of that merge are
  SPEC-4's, and no mode toggle, copy, or acknowledgement substitutes for it.

`retarget` is a tracking-only transition. A paused branch reaches tracking through
`resume_tracking`, which is the one path that demands a replay-verified selection; letting
retarget also un-pause would create a second, weaker resume path.

## Injected seams, because this crate has no database and no capability engine

- `ManagerAppendOpAuthority` is the §2.2 rule 3 Manager+ `append/op` gate. SPEC-3's capability
  chain lives in `src/capability/`, and binding the two is the integration wave's job; taking a
  trait here keeps the branch state machine testable without a capability chain or a database.
- `VerseDagView` supplies the three DAG facts the registry cannot derive: whether an operation
  is admitted in this verse, the admitted `branch_genesis` of a branch, and whether a candidate
  selection is replay-verified. Wave 3 implements it over `fe-database`.
- `head_admission` returns `HeadAdmission::QuarantinedEquivocation { conflicting_op_id }`, and
  every path that could put an operation into a frontier — tracking advance, paused evidence,
  and every member of a control payload's `selected_frontier` — refuses it. §3.4's rule is
  quarantine both and materialize neither, so a branch frontier must never be the place a
  receiver quietly picks the equivocation winner.

## A checkpoint is an accelerator, and the type system says so

`verify_checkpoint_claim` takes the received bytes, not a decoded claim, so §3.2 rule 1's
decode-and-re-encode check cannot be skipped by a caller that already has a struct. It then
composes three independent things, and no two of them can substitute for each other:

1. `ManagerPlusAuthorizationView` — Manager+ status and an unexpired `append/checkpoint`
   capability for the exact verse scope. A relay holding `seed` and a materializer holding
   `materialize` both fail here.
2. The consumer's **own** selection — the frontier commitment is compared against
   `SortedFrontier::commitment` over the frontier the consumer derived from immutable operation
   IDs, and the segment manifest against the one the consumer derived. §3.2 rule 2 forbids
   taking either from a relay's asserted inventory.
3. `ReplayVerifier` — an independently derived projection root.

Only all three agreeing yields `Verified`. A caller that cannot decrypt every scope the
selection represents, cannot reconstruct the authorization view, or cannot replay gets
`UntrustedAccelerator`, which §3.2 rule 4 requires it not present as replay-verified. Any
disagreement is `Rejected`; §3.2 rule 5 forbids a signature from repairing one.

`snapshot_manifest_id` is an `Option<Hash32>` and nothing else, so §3.1 rule 1's ban on
plaintext snapshot bytes inside a claim is unrepresentable rather than merely prohibited.

`compaction_decision` is the D-CL19 §2.2 rule 2 gate: a multi-head frontier compacts only
against a `Verified` checkpoint that binds the exact sorted frontier, and the caller states its
own retained-bootstrap and suppression-evidence obligations because only the caller knows what
it still stores. Those obligations are parameters with no default — durations, quotas, and
retention windows are owner-approved operational policy under D-CL24 and §8, and this crate
picks none of them.

## Cross-module bindings deferred to the integration wave

- SPEC-6's checkpoint-proof module (`src/segment/checkpoint_proof.rs`) deliberately consumes an
  abstract checkpoint view rather than `SignedCheckpointClaim`. Binding the two — so that a
  reachability proof is checked against a real claim — happens in the serial integration wave,
  not in either leaf module.
- `materializer_id` and `materializer_version` are plain fields on `CheckpointClaimV1` rather
  than SPEC-4's materializer identity type, which `src/materialize/identity.rs` owns. The
  integration wave replaces the field pair with that type.
- The checkpoint and branch-control decoders repeat a handful of one-line CBOR field accessors
  that `src/envelope.rs` keeps private. Widening those to `pub(crate)` is an integration-wave
  change to a frozen wave-1 file.
