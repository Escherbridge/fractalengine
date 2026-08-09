# Canonical log capability and revocation model v1

**Status:** Draft -- owner approval required before implementation.

This document defines the capability certificate chain, authorization verbs,
hard-revocation behavior, and topic-privacy requirements for the Canonical
Fractal Data Log. It implements D-CL1, D-CL2, D-CL3, D-CL10, and D-CL17 in
combination with the operation envelope in
[`operation-envelope.md`](operation-envelope.md).

Its D-CL17 key lifecycle is defined in section 10. It does not authorize its
implementation, key transport, relay service, or any other network wiring.

## 1. Conformance vocabulary

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **principal** is a canonical `did:key` Ed25519 DID and its 32-byte raw
   public key.
3. A **certificate** is a signed grant from one principal to another. A
   **chain** is the ordered root-to-leaf sequence of certificates that proves
   a grant for one action.
4. A **capability** is the effective, attenuated grant represented by a valid
   chain; a bare certificate is not a capability.
5. An **epoch scope** is the scope whose monotonically increasing epoch
   invalidates the whole chain. It is selected by the root grant and cannot be
   changed by a delegate.
6. An **authorization view** is the persistent, verified local projection of
   identity, Manager+ authority, membership, and epoch-bump operations. API,
   relay, WebSocket, and materializer processes MUST consult this view; an
   in-memory notification is only a cache invalidation optimization.
7. **Contains** means scope containment: the verse IDs match; a parent with a
   non-null petal ID requires the same petal ID in the child; and a parent
   with a non-null resource ID requires the same resource ID in the child. A
   child MAY narrow a null parent petal or resource ID, but MUST NOT widen a
   non-null one.

`Hash32`, `Identifier32`, the canonical scope map, deterministic CBOR profile,
and byte ordering are exactly those defined by SPEC-1.

## 2. Canonical certificate-chain artifact

### 2.1 Common encoding rules

1. Certificates and chains use RFC 8949 deterministic CBOR with every
   restriction in SPEC-1 section 2: definite lengths, shortest integers,
   no floats, no tags, and no unrecognized map keys.
2. A principal is the two-entry map `{0: did, 1: public_key}`. `did` is a
   canonical `did:key` Ed25519 DID and `public_key` is exactly 32 bytes. The
   DID-to-key binding MUST verify before its signature is considered.
3. A `CertificateId` and `ChainId` are unkeyed 32-byte BLAKE3 digests. They
   are binary values, never text encodings inside signed CBOR.
4. Arrays of hashes, IDs, or operation kinds MUST be strictly ascending by
   unsigned byte or integer order and contain no duplicate.

### 2.2 Certificate grammar

One complete certificate is one CBOR map with exactly fifteen keys in numeric
order. `certificate_id` is derived and is not serialized.

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `capability_version` | unsigned integer | MUST be `1`. |
| 1 | `issuer` | principal map | Signing principal. |
| 2 | `audience` | principal map | Principal allowed to exercise or attenuate this grant. |
| 3 | `parent_certificate_id` | `CertificateId` or `null` | `null` only for a root certificate. |
| 4 | `issuer_authority_id` | `Hash32` | The admitted identity/authority record authorizing the root issuer; delegates copy the root value exactly. |
| 5 | `grant_scope` | scope map | Maximum target scope for this certificate. |
| 6 | `epoch_scope` | scope map | Scope whose epoch applies to the chain; it MUST contain `grant_scope`. |
| 7 | `scope_epoch` | unsigned `u64` | Epoch current when the root certificate was issued; unchanged across the chain. |
| 8 | `verbs` | unsigned `u8` bitset | Non-empty subset of the verb bits in section 3.1. |
| 9 | `object_classes` | unsigned `u8` bitset | Non-empty subset of the class bits in section 3.2. |
| 10 | `not_before_ms` | unsigned `u64` | Inclusive Unix millisecond time. |
| 11 | `not_after_ms` | unsigned `u64` | Exclusive Unix millisecond time. |
| 12 | `delegation_depth` | unsigned `u8` | Number of further certificates the audience may issue. |
| 13 | `caveats` | caveat map | Fixed grammar in section 2.3. |
| 14 | `signature` | 64-byte byte string | Ed25519 signature in section 2.4. |

The unsigned certificate is the exact canonical map containing keys 0 through
13. Its signature preimage is:

```text
ASCII("fe-capability-cert-v1") || 00 || unsigned_certificate
```

`signature` is deterministic Ed25519 signing of that preimage by
`issuer.public_key`. `certificate_id = BLAKE3(complete_certificate)`, where
`complete_certificate` includes key 14 and the signature.

### 2.3 Caveat map

The caveat map contains exactly the following keys. `null` means the
certificate adds no restriction for that field; it never removes a parent
restriction.

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `max_payload_bytes` | `u64` or `null` | Maximum plaintext intent payload bytes. |
| 1 | `max_segment_bytes` | `u64` or `null` | Maximum encrypted segment bytes the audience may seal. |
| 2 | `allowed_schema_hashes` | sorted `Hash32` array or `null` | Allowlist for operation payload schemas. |
| 3 | `resource_allowlist` | sorted `Identifier32` array or `null` | Further restriction within `grant_scope`; each ID MUST be in that scope. |
| 4 | `allowed_operation_kinds` | sorted non-zero `u16` array or `null` | Further restriction on operation kinds the audience may append. |
| 5 | `max_preview_hz` | `u16` or `null` | Maximum previews per second accepted from this audience. |

Unknown caveat keys MUST be rejected. A future caveat grammar requires a new
`capability_version`; silently ignoring a restriction is prohibited.

### 2.4 Chain grammar and verification

A capability chain is the three-entry CBOR map below. It has no outer
signature: every certificate is individually signed and the content address
commits to the complete ordered proof.

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `capability_version` | unsigned integer | MUST be `1`. |
| 1 | `certificates` | non-empty array of byte strings | Exact complete-certificate encodings, root first and leaf last. |
| 2 | `leaf_certificate_id` | `CertificateId` | MUST equal the BLAKE3 ID of the final certificate. |

`chain_id = BLAKE3(complete_chain)`, where `complete_chain` is the exact
canonical outer map. It MUST equal `capability.chain_id` in the referenced
operation envelope.

For a chain to authorize an action, a verifier MUST perform all of these steps:

1. Decode and re-encode the outer chain and every certificate byte-for-byte.
   Reject a non-canonical, malformed, duplicate, or unknown field.
2. Verify each principal's DID-to-key binding and its certificate signature.
3. Require the first certificate to have `parent_certificate_id = null`. Its
   issuer MUST be a current Manager+ identity for `epoch_scope`, as proven by
   `issuer_authority_id` in the verified authorization view.
4. For each later certificate, require its `parent_certificate_id` to equal
   the previous `certificate_id`, its issuer to equal the previous audience,
   its `issuer_authority_id`, `epoch_scope`, and `scope_epoch` to equal the
   root values, and its audience to be a valid principal.
5. Require every child to attenuate, never expand: child scope is contained by
   parent scope; verb and object-class bitsets are subsets; `not_before_ms` is
   no earlier; `not_after_ms` is no later; delegation depth is no greater than
   `parent.delegation_depth - 1`; a non-null parent caveat MUST remain
   non-null in the child; and every non-null child caveat is no less restrictive
   than the corresponding parent caveat.
6. Require the leaf audience to equal the operation author for `append`, or
   the authenticated requester for another verb. A service MUST NOT exercise a
   user's chain merely because it can fetch the chain artifact.
7. Require `not_before_ms <= now < not_after_ms`, a maximum certificate
   lifetime of 24 hours, and `operation.capability.scope_epoch ==
   leaf.scope_epoch`. A clock outside that interval denies the request; an
   implementation MAY choose a stricter maximum lifetime.
8. Require the requested verb, object class, target scope, resource ID,
   payload/segment size, schema hash, operation kind, and preview rate to fit
   the effective leaf grant and every caveat.
9. Resolve `epoch_scope` in the persistent authorization view and require its
   current epoch to equal `scope_epoch`, subject to the causal revocation rule
   in section 5 for historical replay.

The chain itself is an authorization artifact, not verse-wide data. A peer
MUST obtain it only through an authenticated capability presentation or an
authorized artifact request. Its `chain_id` is nevertheless visible in an
operation header, so repeated use is a linkability signal described in
section 8.

## 3. Capability verbs and object classes

### 3.1 Verb bits

| Bit | Verb | Meaning |
| ---: | --- | --- |
| `0x01` | `append` | Create an admissible immutable artifact or operation intent. |
| `0x02` | `fetch` | Request or receive the encrypted/header artifact. It does not grant plaintext access. |
| `0x04` | `decrypt` | Resolve the relevant scope key and authenticate/decrypt an artifact. |
| `0x08` | `materialize` | Apply a verified artifact to a local projection or derived local catalog. |
| `0x10` | `preview` | Send or receive an ephemeral, lossy preview message. |
| `0x20` | `seed` | Retain and serve an immutable artifact to an authorized requester. |

`append`, `materialize`, and `seed` are different authorities. In particular,
a relay with `seed` MUST NOT mint an operation, and an editor with `append`
MUST NOT treat an unverified remote artifact as materialized state.

### 3.2 Object-class bits

| Bit | Class | Meaning |
| ---: | --- | --- |
| `0x01` | `op` | One operation header and its referenced payload artifact. |
| `0x02` | `segment` | An immutable, scope-affine segment or segment manifest. |
| `0x04` | `checkpoint` | A signed replay-verifiable checkpoint claim and its snapshot artifact. |
| `0x08` | `shard` | A Hexon data shard or sub-artifact. |
| `0x10` | `tile` | A terrain tile or tile-derived data artifact. |
| `0x20` | `asset` | A scene asset or asset-derived immutable artifact. |

`preview` traffic is deliberately not an object class. It is a separate
message type with no `op_id`, no durable storage route, and no materializer
entry point (D-CL13).

### 3.3 Effective permission table

Every non-dash cell below requires the named verb and a chain whose effective
scope contains the exact object scope. `Manager+` is additionally verified
from the authorization view; a role embedded only in a bearer token is not
sufficient.

| Object class | Append | Fetch | Decrypt | Materialize | Preview | Seed |
| --- | --- | --- | --- | --- | --- | --- |
| `op` | `append/op`; leaf is author; schema and kind caveats apply | `fetch/op`; headers are verse-wide, payloads use their petal/resource scope | `decrypt/op` plus the payload scope key | `materialize/op`, successful full admission, and decrypt if payload-bearing | --; an operation MUST never travel as a preview | `seed/op`; may serve only after requester `fetch` validation |
| `segment` | Derived sealing only: every enclosed op MUST already be admitted; no capability can bless an invalid op | `fetch/segment`; segment scope MUST be single-scope/petal-affine | `decrypt/segment` plus segment scope key | Unpack only after integrity and admission checks; then requires `materialize/op` for enclosed ops | -- | `seed/segment`; seeders do not need decrypt |
| `checkpoint` | `append/checkpoint` plus current Manager+ and a replay-verifiable claim | `fetch/checkpoint` | `decrypt/checkpoint` plus checkpoint scope key | `materialize/checkpoint`, signature/replay verification, and compatible materializer version | -- | `seed/checkpoint`; never asserts authority merely by serving |
| `shard` | `append/shard` plus validated parent Hexon manifest | `fetch/shard` | `decrypt/shard` plus shard scope key | `materialize/shard` after format/hash validation into a local catalog | -- | `seed/shard`; ciphertext-only seeding is allowed |
| `tile` | `append/tile` plus the authorized source/derivation policy | `fetch/tile` | `decrypt/tile` plus tile scope key | `materialize/tile` after source/hash validation | --; rendered data is not a WS preview | `seed/tile` |
| `asset` | `append/asset` plus the authorized attachment/import policy | `fetch/asset` | `decrypt/asset` plus asset scope key | `materialize/asset` after content/hash validation | --; thumbnails need their own asset artifact/capability | `seed/asset` |

The table prevents a dangerous shortcut: authorization to fetch ciphertext is
not authorization to decrypt it, and authorization to seed a blob is not
authorization to infer or expose its scope to an unqualified requester.

## 4. Delegation and attenuation

1. A root certificate is issued only by a Manager+ identity authorized at
   `epoch_scope`. Its `issuer_authority_id` anchors that authority in the
   verified log. Identity-key rotation and Manager+ history are defined by
   SPEC-2; this document never treats a self-asserted DID as a root issuer.
2. A delegate may issue a child only while its own certificate has
   `delegation_depth > 0`. A terminal certificate has zero depth and cannot
   issue a child.
3. A child certificate MUST remain in the same epoch family: its
   `epoch_scope`, `scope_epoch`, and `issuer_authority_id` equal its parent.
   A delegate cannot obtain a fresh epoch, rotate its authority anchor, or
   escape a hard revocation by constructing a new chain.
4. A child `grant_scope` MUST be contained by its parent. A broad verse grant
   can be narrowed to a petal/resource; a petal/resource grant cannot be
   widened back to verse scope.
5. The effective expiration is the earliest `not_after_ms` in the chain.
   Every V1 certificate MUST expire within 24 hours of issuance. Routine
   removal therefore has a bounded offline window even before hard revocation
   converges.
6. A scope-specific hard-revocation blast radius is selected at root issuance.
   To revoke a petal independently, the Manager+ issuer MUST root that grant
   at the petal as `epoch_scope`. A chain rooted at verse scope is deliberately
   revoked verse-wide; a delegate cannot retroactively make it narrower.
7. A certificate that permits `append/op` is not sufficient for a structural
   authorization operation. `allowed_operation_kinds` MUST explicitly include
   the registered kind, and section 5 adds the Manager+ requirement for an
   epoch bump.
8. A reconnecting member MUST present a currently valid chain. The service
   re-evaluates its authorization view and scope epoch before issuing a fresh
   root/leaf chain or renewal; it MUST NOT extend or resume an expired chain.
   Every replacement certificate remains subject to the 24-hour maximum.

## 5. Persistent epoch revocation

### 5.1 Scope-epoch bump operation

SPEC-1 reserves `operation_kind = 4` for `scope_epoch_bump`. It is a signed,
payload-free structural operation with these rules:

1. Its header `scope` is exactly the `epoch_scope` being bumped; it MUST NOT
   target a descendant or ancestor indirectly.
   It MUST have exactly one parent, as required by SPEC-1.
2. It uses the no-payload form: `encryption = null`, zero ciphertext length,
   and `ciphertext_hash = BLAKE3(empty)`.
3. Its capability epoch is the currently materialized epoch `e`; admission
   raises the target scope's epoch to exactly `e + 1`. Arbitrary skips and
   decreases are invalid.
4. Its author MUST be a current Manager+ identity for that exact epoch scope,
   its leaf chain MUST allow `append/op` and operation kind `4`, and all normal
   chain, signature, parent, and branch rules still apply.
5. The no-payload header is replicated verse-wide. Thus a member can observe
   epoch changes without receiving unrelated petal payload plaintext.
6. Duplicate valid bumps from the same `e` have the same resulting epoch
   `e + 1`; they are retained as auditable evidence only when neither bump is
   in the other's parent closure. A candidate with epoch `e` that follows an
   already admitted `e -> e + 1` bump is stale and rejected. A bump from
   `e + 1` is valid only after the prior epoch is materialized at that scope.

The authorization view persists `current_epoch[epoch_scope]` and the admitted
bump operation IDs. It is rebuilt from verified log data/checkpoints, not from
process-local caches or JWT revocation lists.

### 5.2 Admission and replay rule

1. A newly received operation with an epoch below the current epoch for its
   chain's `epoch_scope` MUST be retained as rejected evidence and MUST NOT be
   appended to an admitted segment or materialized.
2. When a peer later learns a valid epoch bump, it MUST re-evaluate affected
   uncheckpointed state. An operation with a lower epoch remains historically
   valid only when it is in the bump operation's transitive parent closure.
   A lower-epoch operation that is concurrent with, or follows, the bump is
   **revoked-concurrent**: retain it for audit/equivocation evidence but never
   materialize it into the converged projection.
3. Checkpoints MUST bind the epoch-bump closure they used. A checkpoint that
   materializes a revoked-concurrent operation is invalid and cannot bootstrap
   an authorization view.
4. A disconnected peer cannot receive a bump immediately. The 24-hour maximum
   TTL bounds routine exposure; on convergence the causal rule above removes
   stale-authorized state. This is a deliberate availability/security tradeoff,
   not a claim of instantaneous partition-safe revocation.

### 5.3 API, relay, and WebSocket observability

1. API authorization MUST evaluate the persistent authorization view before
   each mutating request, content fetch, decrypt request, and subscription
   change. It MUST key caches by chain ID, epoch scope, epoch, expiry, and
   authority-view version; any epoch bump invalidates matching entries.
2. A relay MUST check the requester's `fetch` capability before disclosing an
   artifact and `seed` capability before accepting a seed commitment. It MAY
   retain ciphertext without `decrypt`, but MUST NOT use cache possession as
   authority to disclose it. A relay reloads the persistent view after restart
   and treats missed notification messages as cache misses, never as approval.
3. A WebSocket session pins the verified leaf principal, chain ID, epoch scope,
   epoch, expiration, and subscribed scopes. On an affected epoch bump or
   expiration, it MUST immediately stop commits, previews, payload delivery,
   and snapshots for that scope. It MAY request a replacement chain; otherwise
   it closes the affected subscription/session with an explicit
   `authorization-revalidation-required` result.
4. A WebSocket MUST re-check expiry on a timer even without traffic. A valid
   initial handshake does not authorize an unbounded session. New subscriptions
   and resumed durable cursors perform a fresh capability check.
5. Authorization-view events may be broadcast to speed revalidation, but every
   consumer MUST read durable epoch state at startup and before a cache-based
   allow decision. This prevents restart or broadcast lag from resurrecting a
   revoked capability.

## 6. Blinded swarm topics

Public names such as a verse namespace, Hexon URI, tileset ID, petal ID, or raw
BLAKE3 hash MUST NOT be used as an advertisement or swarm topic for a private
scope. They reveal project existence to observers and invite dictionary
enumeration.

### 6.1 Topic label and derivation

1. A topic label is the deterministic-CBOR map:

   | Key | Name | Representation |
   | ---: | --- | --- |
   | 0 | `topic_version` | `1` |
   | 1 | `lane` | `0 = header`, `1 = payload`, `2 = availability` |
   | 2 | `object_class` | one class bit from section 3.2 |
   | 3 | `scope` | canonical scope map |
   | 4 | `topic_epoch` | unsigned `u64` |

2. `topic_epoch` is the current epoch of the membership/topic-key scope. A
   hard epoch bump therefore rotates private discovery labels as well as
   capability validation.
3. The V1 topic MAC is the keyed BLAKE3 construction:

   ```text
   topic_digest = BLAKE3_keyed(
       topic_key,
       ASCII("fe-topic-v1") || 00 || canonical_topic_label
   )
   topic_name = "fe-topic-v1/" || lowercase-base32(topic_digest)
   ```

   This is the required MAC-style derivation of a scope label under the
   membership key: it realizes the plan's HMAC-under-a-membership-key
   requirement using BLAKE3's keyed-hash primitive, rather than introducing a
   separate HMAC-SHA suite. It is not an unkeyed BLAKE3 content address.
   `topic_key` is exactly 32 bytes and is distinct per authorized
   membership/key scope.
4. Header lanes use a verse-wide scope label so authenticated verse members can
   obtain causal headers. Payload lanes use the petal/resource scope and are
   petal-affine. A segment MUST NOT mix payloads from different payload-topic
   scopes. This is D-CL2 sparse replication, not optional sharding behavior.
5. A node MUST validate a capability before subscribing, announcing, or
   answering on a private topic. It MUST rotate/unsubscribe affected topics on
   epoch bump. The initial capability/key bootstrap is not a public topic and
   is outside this discovery mechanism.
6. Public data follows the same pipeline. Its published manifest may disclose
   the relevant topic key, but it does not switch to a plaintext or unblinded
   protocol path.

`topic_key` is derived, never independently distributed:

```text
topic_key = BLAKE3_keyed(
    scope_key,
    ASCII("fe-topic-key-v1") || 00 || canonical_scope_key_context
)
```

`canonical_scope_key_context` is the deterministic-CBOR pair of the exact
scope map and its epoch. This gives every authorized recipient the same
32-byte topic key while separating it from payload sealing. The source,
wrapping, rotation, and destruction of `scope_key` are normative in section
10.

## 7. Privacy-leakage analysis

Encryption limits plaintext disclosure; it does not erase metadata. A conforming
implementation MUST document the following residuals to users and operators.

| Surface | Current/existing leakage | Required mitigation | Residual risk |
| --- | --- | --- | --- |
| Namespace, Hexon URI, tileset ID, petal ID | Deterministic identifiers reveal project existence and can be enumerated | Use blinded private topic labels; do not expose raw IDs in public discovery | Authorized members can still correlate their own scopes; a compromised topic key reveals its labels |
| BLAKE3 object/hash URL | A known plaintext can be hashed to test presence; raw hash URLs reveal access patterns | Authorize fetch by scope; use encrypted artifacts; avoid public hash lookup for private data | Ciphertext hash equality exposes repeated ciphertext and cache correlation |
| Header DAG | Verse ID, branch, parents, HLC, author DID, chain ID, and operation timing reveal collaboration graph and cadence | Header lanes only to authorized verse members; do not publish chains in discovery; minimize externally visible status | Verse-wide causal headers intentionally expose cross-petal metadata to members under D-CL2 |
| Payload/segment traffic | Ciphertext length, segment cadence, and petal-affine topic use reveal approximate object size and activity | Scope encryption, per-scope blinded lanes, bounded segment packing, and authorization before response | A relay or network observer can perform traffic analysis; padding/cover traffic are not specified by V1 |
| Capability chain | `chain_id` links repeated acts by one delegation; chain contents reveal identity/scope to a recipient | Present chains only to authenticated verifier paths; retain only under access control; rotate short TTL chains | Operation headers still carry stable chain IDs during a chain's lifetime |
| Relay/IP transport | Relay sees peer addresses, topic requests, timing, and requested ciphertext sizes | Relay is transport/seeder only; minimize logs and scope requests; blinded topics | Blinding is not anonymity, PIR, mixnet, or traffic-analysis protection |
| Public scopes | Published keys intentionally make content/discovery public | Same encrypted artifact pipeline and manifest disclosure | Public status is irrevocable once keys/artifacts are copied |

No privacy claim may promise physical deletion from previously replicated peers.
For encrypted private payloads, the strongest supported erasure claim is
crypto-shredding after the relevant scope keys are destroyed and future access
is denied. Existing ciphertext, headers, content hashes, timing records, and
copies held by an adversary may remain.

## 8. Required conformance tests

Future implementations MUST provide at least these named tests and committed
byte-exact vectors where a canonical artifact is involved:

1. `capability_chain_v1_golden_vector_round_trip` -- canonical root/delegate
   chain, certificate IDs, chain ID, and all Ed25519 signatures reproduce
   byte-for-byte.
2. `capability_chain_rejects_noncanonical_cbor` -- rejects indefinite lengths,
   unordered maps, duplicate keys, unknown fields, floats, malformed NFC text,
   and a DID/public-key mismatch.
3. `capability_chain_rejects_link_or_signature_substitution` -- rejects a
   changed parent ID, issuer/audience link, authority anchor, leaf ID, or
   domain-separated signature.
4. `capability_delegation_only_attenuates` -- independently attempts to widen
   scope, verb, class, lifetime, delegation depth, caveat limit, schema list,
   resource list, and operation-kind list; every attempt fails.
5. `capability_ttl_and_leaf_binding_are_enforced` -- checks 24-hour maximum,
   expiry boundary, not-before boundary, and author/requester leaf binding.
6. `scope_epoch_bump_kind_four_is_payload_free` -- verifies the exact no-payload
   envelope form, Manager+ and kind-caveat requirements, and deterministic
   `e -> e + 1` update.
7. `scope_epoch_bump_rejects_stale_and_revoked_concurrent_ops` -- proves an
   operation in a bump's parent closure remains historical evidence while a
   concurrent/later operation with old epoch is retained but never materialized.
8. `authorization_view_survives_restart_and_notification_loss` -- restarts API
   and relay views after an epoch bump and proves neither allows the old chain.
9. `websocket_revalidates_on_epoch_bump_and_expiry` -- verifies commits,
   previews, payload delivery, snapshots, resume, and new subscriptions stop
   until a valid replacement chain is supplied.
10. `relay_seeding_never_implies_decrypt_or_disclosure` -- a ciphertext-only
    relay can seed only to a valid `fetch` requester and cannot decrypt or mint
    an artifact.
11. `blinded_topic_derivation_is_deterministic_and_lane_separated` -- checks
    byte-exact topic vectors, scope/lane/class/epoch separation, and absence of
    raw identifiers from private topic names.
12. `private_topic_requires_capability_and_rotates_on_epoch_bump` -- rejects
    unauthorized subscribe/announce/respond and verifies old topic labels stop
    after a bump.
13. `scope_key_wrap_requires_current_recipient_authorization` -- a stale,
    expired, revoked, wrong-scope, or non-device-bound recipient cannot obtain
    a scope-key wrap; an authorized reconnect receives a newly issued chain and
    a fresh recipient-specific wrap.
14. `scope_epoch_bump_rotates_scope_and_topic_keys` -- a bump prevents every
    old key/wrap/topic label from serving new access and requires an epoch+1
    key before new payload, segment, snapshot, shard, tile, or asset sealing.

## 9. Design notes

- **One DAG, sparse plaintext:** verse-wide header visibility gives all
  authorized verse members complete causal structure. Petal-affine encrypted
  payload segments preserve selective replication and bounded mobile storage.
- **Capability versus possession:** BLAKE3 addressing proves bytes, not
  permission. Fetch, decrypt, materialize, and seed must remain separately
  checked even when a local cache already has the content.
- **Hard revocation over partition convenience:** concurrent stale writes are
  retained for audit but excluded from the converged materialization. This
  prevents a partitioned, revoked device from winning merely by reconnecting.
- **Blinding is not anonymity:** keyed topics stop public enumeration; they do
  not conceal traffic patterns from a relay or a malicious authorized member.

## 10. D-CL17 scope-key lifecycle and recipient delivery

### 10.1 Scope keys and sealing

1. Every scope/epoch has one 32-byte scope key generated from a CSPRNG. It
   seals every non-empty payload, segment, shard, tile, asset, and
   scope-affine checkpoint snapshot in that scope/epoch with
   XChaCha20-Poly1305. A header remains payload-free.
2. Every XChaCha20-Poly1305 encryption uses a newly generated 24-byte
   (192-bit) CSPRNG nonce. A producer MUST fail closed if its randomness source
   fails or it knows a nonce was reused for the same scope key.
3. `key_id` is `BLAKE3(ASCII("fe-scope-key-id-v1") || 00 ||
   canonical_scope_key_context || scope_key)`. It identifies a key without
   placing key material in an operation header. The same context derives the
   blinded `topic_key` in section 6.
4. Scope-key bytes, ephemeral private wrapping keys, and retired keys reside
   only in controlled device/issuer key stores. They MUST NOT be written to a
   header, manifest, checkpoint claim, preview, analytics result, or discovery
   topic. `suite_id = 65535` remains fixture-only and is rejected in production.

### 10.2 X25519 HPKE-style recipient-device wrap

1. A recipient device enrolls one 32-byte X25519 public key bound to its
   principal. A delivery issuer MUST first verify the device binding, the
   recipient's currently valid `decrypt` capability for the exact scope/epoch,
   and the issuer's current Manager+ authority for that epoch scope.
2. For each delivery, the issuer generates a fresh ephemeral X25519 key pair,
   computes `shared_secret = X25519(ephemeral_private, recipient_device_key)`,
   and derives the 32-byte wrap key as:

   ```text
   BLAKE3_keyed(
       shared_secret,
       ASCII("fe-scope-key-wrap-v1") || 00 || canonical_key_wrap_aad
   )
   ```

   It then uses XChaCha20-Poly1305 with a fresh random 24-byte wrap nonce to
   seal exactly the 32-byte scope key under that wrap key and AAD.
3. A key-wrap artifact contains the deterministic-CBOR `canonical_key_wrap_aad`
   map `{0: 1, 1: scope, 2: scope_epoch, 3: key_id, 4: recipient principal,
   5: recipient X25519 public key, 6: ephemeral X25519 public key}`, followed
   by the wrap nonce, 48-byte sealed key, issuer principal, issuer capability
   reference, and an Ed25519 signature over
   `ASCII("fe-scope-key-wrap-v1") || 00 || canonical_complete_wrap`.
   Recipients validate every binding and the issuer's authority before
   unwrapping. The artifact has its own D-CL3 signature domain.
4. A wrap is recipient-device specific. It may be delivered only over an
   authenticated, authorized delivery channel; it is never a header-lane,
   payload-lane, availability-topic, or preview artifact. A relay may retain
   an opaque wrap only when separately authorized to fetch/seed it and never
   gains decrypt authority from possession.

### 10.3 Authorization, rotation, reconnection, and destruction

1. On initial authorization or reconnect, the Manager+ issuer validates the
   recipient's current chain and device binding, issues or renews a chain with
   at most 24 hours lifetime, then creates a fresh wrap for every scope/epoch
   the recipient may decrypt. No expired, old-epoch, or revoked chain receives
   a wrap.
2. An admitted `scope_epoch_bump` from `e` to `e + 1` requires a new CSPRNG
   scope key for that exact scope. The issuer stops all e-key delivery, rotates
   its blinded topic label, and wraps only the e+1 key to currently authorized
   devices. Removing a member therefore requires the bump before any new wrap;
   it does not retroactively erase an old key the member already copied.
3. Crypto-shredding destroys the current and retired controlled scope-key
   material, local ephemeral wrapping secrets, and all future reissue paths for
   the destroyed scope/epoch. It also removes controlled recipient wraps where
   feasible. Immutable ciphertext, headers, manifests, and uncontrolled copies
   may remain and MUST NOT be described as physically deleted.
4. This ratified contract completes the protocol choice only. Workstream G,
   key delivery, relay seeding, and all network enablement remain prohibited
   until the owner approves the complete SPEC set and a separate implementation
   package.
