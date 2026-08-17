# Canonical log-first materialization contract v1

**Status:** Owner-approved 2026-08-09. Implementation (Workstream G) is unlocked; network rollout, relay seeding, and inbound P2P remain owner-gated.

This document defines the log-first-strict contract between admitted Canonical
Fractal Data Log operations and a local SurrealDB projection. It implements
D-CL14 and the materialization part of D-CL4. It depends on the operation
artifact in [operation-envelope.md](operation-envelope.md), but does not define
capability grammar, segment layout, branch selection, retention, or transport.
Those decisions belong respectively to SPEC-3, SPEC-6, and SPEC-5.

## 1. Conformance vocabulary and scope

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. An **admitted operation** has the meaning in SPEC-1 §1: its canonical
   envelope, author binding, signature, capability, schema, parent set, payload
   artifact, and encryption have verified. A stored but opaque header is not an
   admitted operation and MUST NOT affect a projection.
3. The **verified log** is the durable, append-only set of admitted operation
   bytes, indexed by their SPEC-1 `op_id`. A staging or quarantine store is not
   part of the verified log.
4. A **projection** is the state derived by one identified materializer from a
   selected causal history. SurrealDB is a projection and catalog; it is never
   the source of truth for canonical state.
5. A **materializer version** identifies the exact deterministic reduction
   rules, registered schema interpreters, and projection layout used to derive
   a projection. A version change MUST be explicit; executable build identity
   alone is insufficient.
6. This document specifies the admission-to-projection boundary. SPEC-5 selects
   branch frontiers and defines tracking, paused, detached, checkpoint retention,
   and quarantine bounds. This document makes none of those policy choices.

## 2. Required commit pipeline

For an operation intended to affect a projection, an implementation MUST use
the following order and MUST NOT substitute a direct SurrealDB write.

```text
validate candidate
  → admit operation
  → durably append verified immutable bytes
  → deterministically materialize eligible causal history
  → atomically commit Surreal projection state and apply marker
```

1. Candidate validation MUST complete all SPEC-1 admission checks and the
   applicable identity, capability, revocation, schema, target-existence, and
   scope-validity checks before the operation enters the verified log. A
   locally decidable failed precondition MUST be returned before an appendable
   envelope is built; it MUST NOT create a durable operation merely to fail
   during projection.
2. A successful append MUST make the exact complete-envelope bytes retrievable
   by their derived `op_id` across process restart. It MUST also durably record
   the byte-to-`op_id` binding and enough verified metadata to find parents.
3. The commit boundary is **log-first-strict**: a failed, cancelled, or
   indeterminate append MUST fail the mutation and MUST NOT modify SurrealDB,
   emit a committed event, or report success to the caller. This is D-CL14.
4. An indeterminate append outcome MUST be reconciled by looking up the exact
   `op_id`. A caller MAY retry the same immutable bytes; it MUST NOT mint a
   replacement operation merely because the first acknowledgement was lost.
5. Once append succeeds, a process crash or materializer failure MAY delay the
   projection but MUST NOT invalidate the accepted operation. The operation is
   pending materialization until replay completes.
6. A committed WS/API notification, checkpoint claim, or analytics result MUST
   refer only to a projection that includes a durable verified-log position.
   Preview traffic is outside this pipeline under D-CL13 and SPEC-7.

## 3. Durable append and exactly-once projection

1. `op_id` is the sole identity of an operation. The first append of valid
   complete-envelope bytes records that identity exactly once.
2. Re-appending the same `op_id` with byte-identical bytes MUST return
   `already_present` and MUST NOT create a second log entry, causal edge,
   materializer work item, or externally visible commit.
3. Bytes that do not hash to the claimed `op_id` MUST be rejected. A claimed
   identity that maps to different bytes is an integrity violation; neither
   candidate may be materialized.
4. A materializer MUST persist an apply marker keyed by at least
   `(materializer_version, projection_identity, op_id)`. Applying an operation
   already marked for that projection MUST be a no-op with the same resulting
   projection bytes.
5. For one materializer and projection, the SurrealDB mutation and its apply
   marker MUST commit atomically. If a crash occurs before that commit, replay
   MUST apply the operation again. If it occurs after that commit, replay MUST
   observe the marker and not apply it again.
6. An implementation that cannot make a particular SurrealDB mutation and
   apply marker atomic MUST rebuild that projection from the last valid
   checkpoint or empty state rather than claim exactly-once projection.
7. The append store MAY contain an admitted operation before all materializer
   instances process it. That is normal eventual projection lag, not a second
   commit protocol.

## 4. Deterministic causal materialization

1. A materializer MUST derive state from the causal closure of the selected
   head, never from arrival order, local row timestamps, database-generated
   IDs, wall-clock reads, or mutable pre-existing SurrealDB values.
2. A child operation is eligible only after each parent in its closure has been
   admitted and processed for the same materializer version, subject to
   semantic exclusion rules such as an admitted disavow from SPEC-2.
3. Among causally concurrent eligible operations, the materializer MUST use
   the deterministic HLC order from SPEC-1: `(wall_ms, counter,
   author.public_key)`. The author/HLC equivocation rule in SPEC-1 prevents an
   unresolved equal ordering key.
4. Registered schemas MUST define deterministic intent reductions and conflict
   behavior for their operation kinds. A schema MUST NOT read an old projection
   value into a new canonical intent, depend on physical row order, or use
   floating-point values as canonical state.
5. A materializer MUST apply the same admitted operation set, causal closure,
   and materializer version to the same projection identity with identical
   resulting canonical projection state on every conforming implementation.
6. A materializer MUST retain the evidence and diagnostic status of an
   operation excluded from a projection. It MUST NOT mutate, delete, or rewrite
   the operation bytes or DAG edges to make the projection convenient.
7. A change to a lifecycle or authorization fact that semantically excludes
   prior projected operations MUST trigger deterministic replay of the affected
   causal history. It MUST NOT be implemented as an ad hoc compensating
   SurrealDB row edit.
8. **A reduction MUST NOT read an external artifact to compute projected
   state.** The admitted operation's own verified bytes and extracted facts are
   the only admissible inputs. A reduction MAY write a deterministic
   *reference* to an external artifact — a snapshot component, segment shard,
   payload artifact, or bulk columnar artifact — where the referenced identity
   is a content address derived from those same signed bytes. It MUST NOT
   dereference that artifact and fold its contents, length, statistics,
   presence, or absence into the projected value.

   This is the bright line that keeps rule 5 and section 6 rule 1 satisfiable.
   Which external artifacts a peer holds is local availability, and it differs
   between peers by design under D-CL2 sparse payload replication. A reduction
   that reads one produces a projection that is a function of local storage
   rather than of the admitted operation set, so two conforming peers with the
   same closure would materialize different state and neither could rebuild the
   other's from verified history.
9. When a reduction cannot construct the verified reference required by rule 8
   — for example the segment manifest binding the artifact ID and its stored
   length is not locally held — it MUST return the
   `referenced_artifact_unavailable` state in section 5 rather than substitute a
   default, an empty reference, a partial value, or a semantic exclusion. That
   state MUST be expressible in the reduction's own result type: an
   implementation whose reduction signature can only apply or exclude cannot
   represent it, and will encode an availability gap as a settled decision.

## 5. Admission, quarantine, and materializer errors

The following errors are protocol states, not reasons to silently skip a
candidate. Error names are stable conformance categories; implementations MAY
attach implementation-specific diagnostics without changing their meaning.

| Category | Condition | Required result |
| --- | --- | --- |
| `invalid_envelope` | Non-canonical bytes, invalid `op_id`, DID/key mismatch, malformed field, bad signature, invalid author/HLC uniqueness, or invalid structural rule. | Reject; do not append or materialize. Retain bounded diagnostic evidence only if local policy permits. |
| `unauthorized_operation` | Capability, epoch, revocation, identity-lifecycle, or operation-kind authority check fails. | Reject; do not append or materialize. |
| `invalid_payload` | Payload hash/length, AEAD authentication, decryption, or schema validation fails after an otherwise parseable candidate. | Reject; do not append or materialize. |
| `precondition_failed` | A cheap, locally decidable target-existence, target-scope, or lifecycle precondition fails before admission. | Return the original command/API error; do not construct or append an operation. This is the required local implementation rule in `fe-database/src/AGENTS.md` §log-first-commit. |
| `missing_parent` | A syntactically valid candidate references a parent not yet admitted locally. | Place outside the verified log in pending quarantine; do not materialize. Re-evaluate only after its full parent closure arrives. Bounds and expiry are SPEC-5 decisions. |
| `unknown_schema` | The schema hash or required deterministic interpreter is unavailable. | Quarantine outside the verified log; do not materialize or reinterpret. Re-evaluate only after the exact registered schema is available and validates. Bounds and expiry are SPEC-5 decisions. |
| `unknown_kind` | The candidate's `operation_kind` has no registered structural rule, schema, and reduction in this build (SPEC-1 §6 rule 7). | Quarantine outside the verified log; do not materialize or reinterpret as a neighbouring kind. Re-evaluate only after this build gains that exact kind. Distinct from `unknown_schema`: a schema arrives through the registry, a kind through a build upgrade. Its budget is partitioned from every other reason per SPEC-5 §4 rule 4. |
| `opaque_payload` | The header is available but the local materializer lacks an authorized, verified payload artifact. | Retain only the non-admitted header/index evidence; do not materialize. Fetch and key-distribution policy is SPEC-3/SPEC-6. |
| `referenced_artifact_unavailable` | A deterministic reduction cannot construct the verified reference section 4 rule 8 permits it to write, because the artifact's binding evidence is not locally held. | Mark the projection position pending replay; do not advance the apply marker, substitute a default or empty reference, or record a semantic exclusion. Retryable, like `missing_parent`. Fetch and key-distribution policy is SPEC-3/SPEC-6. |
| `materialization_failed` | A verified append succeeded but deterministic reduction or SurrealDB commit did not finish. | Mark pending replay; do not expose the affected projection position as committed. Retry or rebuild solely from verified history. |
| `checkpoint_mismatch` | A checkpoint identity, signature, projection root, or replay result disagrees with verified history. | Reject the checkpoint as an accelerator; retain the log and rebuild from an earlier verified checkpoint or empty state. |

1. An error in the middle of a fetched segment or history MUST NOT cause a
   materializer to skip that candidate and continue as though the history were
   complete. The affected head remains unresolved for that projection until the
   error has the required disposition above.
2. Missing parents, unknown schemas, unknown operation kinds, and unavailable
   referenced artifacts are retryable availability states, not proof that a
   candidate is valid. They MUST NOT be provisionally applied.
3. Invalid and unauthorized candidates MUST never be promoted to valid merely
   because a later operation refers to them or because a relay stored them.
4. Materializer failure after append is recoverable only by deterministic retry
   or replay. It MUST NOT be repaired by hand-editing SurrealDB and advancing
   the apply marker.

## 6. Rebuild and checkpoint contract

1. A conforming projection MUST be rebuildable from an empty projection or a
   verified checkpoint plus the verified operation closure from that checkpoint
   to its selected frontier. No unpublished SurrealDB row, cache, or local event
   stream may be required for correctness.
2. A checkpoint is an acceleration claim under D-CL4, not an authority over
   the log. A receiver MUST be able to reject it and obtain the same resulting
   projection by replay.
3. Every checkpoint MUST bind, in canonical signed checkpoint bytes, at least:

   | Field | Binding requirement |
   | --- | --- |
   | `branch_id` | Identifies the branch whose causal history was projected. |
    | `frontier_commitment` | BLAKE3 of the exact lexicographically sorted selected-frontier `op_id` list. |
   | `segment_manifest_id` | Identifies the immutable segment manifest whose closure was used. |
   | `materializer_id` and `materializer_version` | Identify the deterministic projection rules and schema interpreter set. |
   | `projection_root_hash` | Commits to the canonical projected state or a deterministic export of it. |

4. A checkpoint consumer MUST verify that its selected frontier and segment
   manifest match the checkpoint bindings before using it. It MUST NOT treat a
   matching Manager+ signature alone as proof of correctness.
5. A materializer version change MUST create a distinct projection identity and
   checkpoint identity, even if the resulting rows appear equal in one test
   corpus. Old checkpoints remain evidence for their declared version; they
   MUST NOT be relabelled as a newer version.
6. The checkpoint signature format, Manager+ signing threshold, sorted-frontier
   selection, segment reachability proof, storage duration, and garbage
   collection rules are defined or constrained by SPEC-5 and SPEC-6.
7. Rule 1 is only satisfiable because of section 4 rule 8. A rebuild replays
   verified operations, and nothing else is guaranteed to be present at rebuild
   time — so a projection whose values were computed by reading external
   artifacts is not rebuildable, whatever a rebuild appears to produce on the
   machine that first materialized it. A projection MAY carry deterministic
   references to external artifacts, because a reference replays to the same
   value whether or not the artifact is held.

## 7. Reproducible analytics outcome

1. The canonical input identity for an analytics computation is the tuple of
   `branch_id`, `frontier_commitment`, `segment_manifest_id`, and
   `materializer_id/materializer_version`, or the identity of a checkpoint
   binding those values.
2. Given the same canonical input identity and authorized payload set, every
   conforming materializer MUST produce the same canonical source projection.
   This is the reproducibility guarantee for historical analytics.
3. An analytics result derived from a canonical projection SHOULD record that
   input identity. A result that does not identify its source projection MUST
   NOT be described as reproducible from the canonical log.
4. This contract does not prescribe an analytics query language, output format,
   access-control policy, or branch retention window. It guarantees a stable,
   replay-verifiable source relation for those future interfaces.

## 8. Required conformance tests

A conforming implementation MUST provide deterministic fixtures and tests with
at least the following names and outcomes.

1. **`append_before_projection`** — an admitted operation is recoverable by
   `op_id` before any projection mutation; no direct projection write occurs.
2. **`strict_append_failure_leaves_projection_unchanged`** — append failure,
   cancellation, and indeterminate append leave SurrealDB, committed-event
   output, and analytics position unchanged; exact-byte retry deduplicates.
3. **`append_and_apply_idempotence`** — duplicate immutable append and replay
   produce one verified-log entry, one materialized effect, and one apply
   marker.
4. **`crash_between_append_and_apply_replays`** — a crash after durable append
   and before atomic apply is recovered solely by replay, with no hand repair.
5. **`crash_after_atomic_apply_does_not_double_apply`** — replay after a crash
   following projection/apply-marker commit is a no-op.
6. **`causal_and_concurrent_order_is_arrival_independent`** — permutations of
   the same causal DAG, including concurrent HLC values, yield identical
   canonical projection roots.
7. **`invalid_midstream_candidate_blocks_complete_claim`** — invalid,
   unauthorized, missing-parent, unknown-schema, and opaque-payload cases are
   respectively rejected or quarantined and never silently skipped.
8. **`materializer_failure_stays_pending_until_replay`** — an appended
   operation with a failed reduction is not announced as projected; deterministic
   retry or rebuild converges without a replacement operation.
9. **`checkpoint_binds_frontier_manifest_and_materializer`** — changing any of
    selected frontier, segment manifest, materializer identity/version, or
    projection root invalidates the checkpoint for reuse.
10. **`empty_rebuild_matches_checkpoint_replay`** — an empty-state replay and
    verified-checkpoint-plus-tail replay produce the same projection identity
    and canonical projection root.
11. **`analytics_source_identity_is_reproducible`** — two independent
    materializations with the same declared source identity expose identical
    canonical analytics input relations.
12. **`reduction_writes_references_and_never_reads_external_artifacts`** — two
    peers replaying the same admitted closure, one holding every referenced
    external artifact and one holding none, produce identical canonical
    projection roots for every operation either of them reduced.
13. **`referenced_artifact_unavailable_is_expressible_and_distinct_from_exclusion`**
    — a reduction that cannot construct its verified reference returns the
    `referenced_artifact_unavailable` state, that state is not equal to any
    semantic exclusion, the apply marker does not advance, and re-reducing the
    same operation returns the same state.

## 9. Design notes

- **Append is authority; projection is derivative:** A local database is useful
  for queries, indexes, and UI latency, but it cannot authenticate history or
  repair a missing signed operation. Log-first-strict behavior therefore makes
  append failure visible rather than manufacturing local state.
- **Pending is safer than divergent:** An unavailable parent, schema, payload,
  or materializer is an availability problem. Treating it as a successful
  no-op would make different peers produce different views of the same head.
- **Version is part of data provenance:** A checkpoint without materializer
  identity can allow two applications to claim that incompatible reductions are
  both the verified state. Binding the version makes the analytics source
  auditable and replay-verifiable.
- **No branch or storage policy here:** This contract intentionally does not
   choose what a mobile peer retains, when a head advances, how detached work is
   merged, or when quarantine expires. Those choices remain with SPEC-5.
- **A reference replays; a read does not:** Section 4 rule 8 is what lets the
  canonical log commit bulk artifacts — snapshots, and under D-CL26 columnar
  observation hexons — by reference without making the projection a function of
  what this machine happens to have on disk. The distinction is not stylistic:
  it is the difference between a projection two peers can both rebuild and one
  only its author can.

### Errata

Owner-ratified under D-CL28, 2026-08-16.

- **G2 (2026-08-16):** §4 gained rules 8 and 9, §5 gained the
  `referenced_artifact_unavailable` category, §5 note 2 was widened, and §6
  gained rule 7. §4 previously stated that a materializer must not read a
  mutable pre-existing SurrealDB value (rule 1) and must produce identical state
  everywhere (rule 5), but never stated the general rule those two imply: an
  external artifact is as much a local-availability input as a local row. Rule 9
  additionally requires the unavailable state to be *expressible in the
  reduction's own result type*, because a reduction that can only apply or
  exclude will encode an availability gap as a settled decision, and two peers
  will then disagree about a result one of them never actually computed.
- **G1 companion (2026-08-16):** §5 gained the `unknown_kind` category, so
  SPEC-5 §4's partitioned quarantine reasons have canonical names here. See
  `branches-checkpoints-retention.md` §7 erratum G1.

## 10. D-CL20 approved legacy-operation deferral

HARD-1's local `commit_operation` seam is not a canonical materializer for
GLTF import, node duplicate/rename, or Verse/Fractal/Petal creation. D-CL20
approves deferring those multi-row operation contracts to SPEC-4/SPEC-8 rather
than inventing a replay meaning during hardening. They remain excluded from any
canonical cutover, shadow parity claim, or Workstream G implementation until
each has an atomic intent schema, exact candidate set, deterministic reduction,
and failure/replay contract.
