# Canonical log author key and identity lifecycle v1

**Status:** Owner-approved 2026-08-09. Implementation (Workstream G) is unlocked; network rollout, relay seeding, and inbound P2P remain owner-gated.

This document defines the author-key lifecycle for the Canonical Fractal Data
Log. It implements D-CL11 using the operation envelope in
[operation-envelope.md](operation-envelope.md). It does not define capability
certificate grammar, capability issuance, payload-key distribution, or branch
retention.

## 1. Conformance vocabulary and terms

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **key instance** is the pair of a canonical Ed25519 `did:key` value and
   its 32-byte public key. The two values MUST bind as required by SPEC-1
   §3.2.
3. A **lineage** is the directed, scope-local sequence of admitted key-rotation
   operations from a predecessor key instance to successor key instances. It
   proves key continuity, not a real-world-person claim.
4. A key is **active at a causal point** when it is the last non-superseded key
   in its lineage as resolved from that point's reachable parents. A key is
   **retired** only on causal paths that reach its admitted rotation.
5. A **suspect window** is the inclusive range of HLC pairs for one key
   instance, at one affected scope or its descendants, identified by an
   admitted Manager+ disavow operation.
6. An operation is **semantically disavowed** when it matches a suspect window.
   Its immutable bytes and cryptographic evidence remain retained, but it MUST
   NOT contribute to a materialized projection.
7. Key lineage never grants authority. Every authoring operation, including a
   lifecycle operation, MUST independently validate its capability at the
   envelope's `scope_epoch` under SPEC-3.

## 2. Identity binding and `fe-identity`

1. V1 accepts only `did:key` Ed25519 identifiers. A `did:key` is derived from
   its public key and is therefore immutable; rotation creates a successor DID,
   not a mutable DID document.
2. `fe_identity::NodeKeypair` is the local holder of one Ed25519 signing key,
   and `NodeIdentity` derives its `did:key` from that key. Future lifecycle
   code MUST use those primitives for key generation, DID derivation, signing,
   and strict Ed25519 verification. It MUST NOT accept an account name,
   keychain slot, Iroh node ID, or API-token subject as proof of log identity.
3. `load_or_generate_keypair` producing a new key after secret loss creates an
   unrelated key instance. It MUST NOT be presented as a continuation of the
   lost key's lineage without an admitted recovery operation defined below.
4. The current use of the same seed for `NodeKeypair` and Iroh transport keys
   does not make an Iroh connection an author signature. Operation admission
   MUST verify the envelope signature and lineage independently of transport
   authentication.
5. A lineage is scoped by the envelope scope of its rotation operation. A
   rotation at verse scope applies to that verse and all descendant scopes; a
   narrower rotation applies only at that scope and its descendants. It has no
   effect in another verse.

## 3. Self-certifying key rotation

1. A rotation is a normal encrypted intent operation (`operation_kind = 1`)
   using the registered `author-key-rotation-v1` schema. It MUST have exactly
   one parent, and its header author MUST be the predecessor key instance.
2. The predecessor MUST be active at the operation's parent-induced causal
   state. A rotation signed by a retired, unknown, disavowed, or otherwise
   unauthorized predecessor MUST be quarantined and MUST NOT materialize.
3. The decrypted rotation payload is one deterministic-CBOR map with exactly
   the following fields:

   | Key | Name | Representation | Rule |
   | ---: | --- | --- | --- |
   | 0 | `rotation_statement` | CBOR map | Exact map in §3.1. |
   | 1 | `successor_signature` | byte string, 64 bytes | Ed25519 signature in §3.2. |

4. The successor DID and public key MUST bind exactly. The successor key MUST
   differ from the predecessor key and MUST NOT already occur earlier in the
   predecessor's lineage.
5. The envelope signature proves authorization by the predecessor; the
   successor signature proves possession by the successor. A record with only
   one proof is not a rotation.
6. Rotation changes only the signer lineage. It does not transfer, mint,
   extend, or revoke a capability. Until SPEC-3 defines successor bindings, an
   operation signed by the successor MUST carry an independently valid
   capability for that successor at its scope and epoch.

### 3.1 Rotation statement

The `rotation_statement` map has exactly nine numeric keys.

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `protocol_version` | unsigned integer | MUST be `1`. |
| 1 | `scope` | map | Byte-for-byte equal to the envelope scope. |
| 2 | `branch_id` | `Identifier32` | Byte-for-byte equal to the envelope branch ID. |
| 3 | `parent_op_id` | `Hash32` | Equal to the sole envelope parent. |
| 4 | `scope_epoch` | unsigned `u64` | Equal to the envelope capability epoch. |
| 5 | `predecessor_did` | UTF-8 text | Equal to the envelope author DID. |
| 6 | `predecessor_public_key` | byte string, 32 bytes | Equal to the envelope author public key. |
| 7 | `successor_did` | UTF-8 text | Canonical Ed25519 `did:key`. |
| 8 | `successor_public_key` | byte string, 32 bytes | Key derived from `successor_did`. |

The statement binds the successor proof to the exact scope, branch, causal
parent, predecessor, and authorization epoch. It deliberately excludes the
outer operation ID, whose inclusion would create a signature/hash cycle.

### 3.2 Successor proof

`successor_signature` is deterministic Ed25519 signing by
`successor_public_key` of:

```text
ASCII("fe-author-key-rotation-v1") || 00 || canonical_rotation_statement
```

`canonical_rotation_statement` is the deterministic-CBOR encoding of §3.1.
Receivers MUST verify this proof after decrypting and schema-validating the
payload, in addition to verifying the predecessor's envelope signature.

## 4. Rotation validation and active-key resolution

1. A receiver MUST first complete the envelope admission checks in SPEC-1.
   It MUST then obtain the payload through a valid fetch/decrypt authorization
   and validate the registered rotation schema.
2. The receiver MUST verify every equality in §3.1, both DID/key bindings,
   the successor proof, the single-parent rule, and predecessor capability at
   the parent-induced `scope_epoch`.
3. A rotation whose parent, schema, payload key, capability chain, or epoch
   state is unavailable MUST enter the bounded quarantine defined by SPEC-5.
   It MUST NOT make its successor active provisionally.
4. On a causal path reaching one admitted rotation, its predecessor is retired
   and its successor becomes active for that scope. Operations whose parents
   causally include that rotation MUST NOT use the retired predecessor.
5. An operation by the predecessor that is causally concurrent with a rotation
   is not retroactively invalid merely because it arrived later. If hard
   exclusion is needed, a Manager+ actor MUST use the scope-epoch and disavow
   procedures in §§5–6.
6. Two causally concurrent rotations from the same predecessor at the same
   scope are a **rotation fork**. Receivers MUST retain both records, mark both
   successor transitions unresolved, and MUST NOT treat either successor as
   active until an authorized Manager+ resolution operation is defined and
   ratified. A causally later second rotation by the already retired
   predecessor is invalid.
7. A receiver MUST reject a lineage cycle, a duplicate successor transition,
   or an operation whose `(author.public_key, wall_ms, counter)` conflicts with
   a different `op_id`, as required by SPEC-1 §3.4.

## 5. Epoch interaction, planned recovery, and compromise containment

1. Rotation does not increment a scope epoch. A rotation is valid only when
   its predecessor capability validates at the parent-induced epoch recorded
   in the envelope.
2. A scope-epoch bump invalidates future use of capabilities from the earlier
   epoch; it does not erase or automatically disavow operations that were
   valid at their causal epoch. Validation is causal, never based on a
   receiver's wall-clock time or its newest observed epoch.
3. If an epoch bump is causally visible to an operation, that operation MUST
   use the bumped epoch and a capability valid for it. An operation concurrent
   with an unavailable bump may remain historically valid; this is the
   partition boundary addressed by an explicit suspect-window disavow.
4. **Planned recovery** is a normal §3 rotation completed while the
   predecessor remains available: create the successor in the local secret
   store, append and receive admission for the dual-signed rotation, obtain
   successor capabilities, then retire local use of the predecessor. Destroying
   a local old secret does not erase its historic signatures.
5. On suspected compromise, an authorized Manager+ actor MUST first issue
   scope-epoch bumps for every affected scope, then provision a successor with
   current-epoch capabilities. It SHOULD also issue the narrowest truthful
   suspect-window disavow and trigger a deterministic rematerialization before
   trusting derived state or a checkpoint.
6. A lost predecessor key has no self-certifying continuity path in V1. A new
   `fe-identity` key is a new principal and MAY receive a new independent
   capability, but it MUST NOT claim the lost key's lineage or retroactive
   authority. An Owner-countersigned continuity grant may link the principals
   only for attribution/display under section 9.

## 6. Manager+ suspect-window disavow

1. A disavow is a normal encrypted intent operation using the registered
   `manager-suspect-key-disavow-v1` schema. Its author MUST validate a
   Manager+ capability at the disavow operation's parent-induced epoch for the
   envelope scope containing the affected scope.
2. The decrypted payload is one deterministic-CBOR map with exactly six
   fields:

   | Key | Name | Representation | Rule |
   | ---: | --- | --- | --- |
   | 0 | `subject_did` | UTF-8 text | Canonical DID of the suspect key. |
   | 1 | `subject_public_key` | byte string, 32 bytes | MUST bind to `subject_did`. |
   | 2 | `affected_scope` | map | Same grammar as SPEC-1 §3.1; equal to or below the header scope. |
   | 3 | `first_hlc` | map | Inclusive `{ wall_ms, counter }` lower bound. |
   | 4 | `last_hlc` | map | Inclusive `{ wall_ms, counter }` upper bound; MUST be no earlier than `first_hlc`. |
   | 5 | `reason_code` | unsigned `u16` | Registered code; no free-form sensitive text. |

3. An operation matches the disavow exactly when its author key equals
   `subject_public_key`, its scope is `affected_scope` or one of its
   descendants, and its HLC pair lies inclusively within the stated range.
   A matching operation becomes semantically disavowed regardless of its valid
   original signature.
4. Disavows are monotonic: overlapping admitted disavows union their matching
   sets. They MUST NOT delete or rewrite the matched bytes, DAG edges,
   signatures, or content addresses.
5. A materializer replaying a head that reaches a disavow MUST exclude its
   matching operations from the projection. It MUST then revalidate dependent
   authorization facts; a descendant whose capability or semantic precondition
   depends on a disavowed fact MUST NOT materialize. Other descendants remain
   retained evidence and may materialize if independently valid.
6. A checkpoint after a disavow MUST commit to the reached disavow operation
   and to the resulting materializer version. A peer replays rather than trusts
   a Manager+ claim blindly, consistent with D-CL4.
7. A key rotation, capability grant, or scope-epoch bump within the disavowed
   window has no special immunity. Its effects are recomputed from the retained
   non-disavowed history. This is the historic-operation blast radius.
8. A disavow alone does not prevent a compromised key from using an otherwise
   live capability. Scope-epoch bumps stop future authorization; disavows
   classify historic operations. Both are required for compromise containment.

### 6.1 Disavow rescind

1. A rescind is a normal encrypted intent operation using the registered
   `manager-suspect-key-disavow-rescind-v1` schema. Its payload names exactly
   one admitted disavow `op_id` and an enumerated reason code; it cannot alter
   the original disavow bytes or expand its scope/window.
2. The rescind author MUST hold authority strictly higher than the authority of
   the original disavow issuer in the parent-induced authorization view. An
   Owner-issued disavow is final and MUST reject every rescind attempt.
3. A valid rescind removes only the named disavow's projection effect from the
   rescind operation's causal future. Historical checkpoints and projections
   retain the original evidence and are replayed under their selected closure;
   no mutable correction or retroactive authority is inferred.

## 7. Required conformance cases

A future implementation MUST provide deterministic fixtures and tests for at
least the following cases.

1. Valid predecessor envelope signature plus valid successor proof activates
   exactly the announced successor after its sole parent.
2. A malformed DID/key binding, changed branch, changed parent, changed scope,
   or changed epoch invalidates the successor proof.
3. A successor proof made by another key, a repeated key, and a lineage cycle
   are rejected.
4. A rotation by an unauthorized, retired, unknown, or disavowed predecessor
   does not activate its successor.
5. A missing parent, schema, payload key, capability chain, or epoch state
   remains quarantined and never provisionally activates a successor.
6. An old-key operation causally after rotation is rejected; a causally
   concurrent old-key operation follows normal validation until a disavow
   applies.
7. Two concurrent rotations from one predecessor produce a rotation fork and
   activate neither successor.
8. A stale-epoch rotation following a visible scope-epoch bump is rejected;
   a historically valid rotation before that bump remains replay-verifiable.
9. A Manager+ disavow rejects an editor-authorized disavow, a wrong-scope
   disavow, and an invalid HLC range; a valid bounded window marks only the
   intended key/scope/HLC operations semantically disavowed.
10. Replaying the same head on two implementations yields the same retained
    evidence set, disavowed set, active-key state, and projection after a
    disavow.
11. Loss of a local `fe-identity` secret followed by
    `load_or_generate_keypair` cannot impersonate the lost DID or continue its
    lineage.
12. An Iroh transport key match without a valid envelope signature and
    successor proof does not authorize an operation or a rotation.
13. A regenerated key is treated as a new principal even when an
    Owner-countersigned continuity grant links it to a lost predecessor for
    attribution/display.
14. A rescind by equal or lower authority, or any rescind of an Owner-issued
    disavow, is rejected; a strictly higher-authority rescind has only its
    causal-future projection effect.

## 8. Design notes

- **Key-derived DIDs:** `did:key` is intentionally self-certifying, but it
  cannot be edited to point at a replacement key. The signed lineage supplies
  continuity without pretending that a new key has the same DID.
- **Dual possession proof:** predecessor-only rotation would let a stolen old
  key nominate an unwilling successor; successor-only rotation would let any
  new key claim an existing history. Both proofs are required.
- **Causal cutoff, not clock cutoff:** an HLC is an ordering aid, not a trusted
  global clock. Rotation takes effect only where its parent is causally known;
  hard containment therefore combines epochs with an auditable disavow window.
- **Evidence is not projection:** preserving disavowed bytes supports audit,
  peer convergence, and reproducible detection. It does not make their prior
  materialized effects acceptable.

## 9. D-CL18 exceptional recovery and correction

1. A regenerated identity is always a new principal. It has no self-certified
   successor relationship, no inherited capability, and no authority over old
   operations merely because it was created after a key loss.
2. An Owner-countersigned `owner-continuity-grant-v1` encrypted intent may
   link one lost predecessor principal to one new principal for attribution and
   display. The payload binds both principals, the affected verse scope, and a
   reason code. It must be signed by the Owner in the ordinary envelope and by
   the new principal under the domain `fe-owner-continuity-grant-v1` before it
   materializes as an attribution link.
3. The grant creates no signer lineage, capability delegation, key recovery,
   retroactive validation, disavow override, or right to reuse historical
   authorship. The new principal still needs an independently issued current
   capability for every action.
4. Section 6.1 is the only disavow correction path. Strictly higher authority
   than the original issuer is required, and Owner-issued disavows are final.
   A continuity grant cannot rescind a disavow or resolve a rotation fork.
