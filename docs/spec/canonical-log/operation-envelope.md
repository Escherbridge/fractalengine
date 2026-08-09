# Canonical log operation envelope v1

**Status:** Owner-approved 2026-08-09. Implementation (Workstream G) is unlocked; network rollout, relay seeding, and inbound P2P remain owner-gated.

This document defines the immutable operation artifact for the Canonical
Fractal Data Log. It implements D-CL1 through D-CL9 at the envelope boundary.
Identity lifecycle belongs to SPEC-2; capability and key distribution belong to
SPEC-3; materialization belongs to SPEC-4; branch/retention behavior belongs to
SPEC-5; and segment/relay behavior belongs to SPEC-6.

## 1. Conformance vocabulary

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **header** is the canonical CBOR operation envelope defined in §3. It
   contains no payload plaintext or payload ciphertext.
3. A **payload artifact** is the separately stored ciphertext identified by a
   header's `ciphertext_hash`.
4. An **admitted operation** is a header whose canonical encoding, author
   binding, signature, capability, schema, parent set, payload artifact, and
   encryption all verify. Parsing a header alone does not admit it.
5. A `Hash32`, `Identifier32`, and key identifier are byte strings of exactly
   32 bytes. All BLAKE3 values use the unkeyed 32-byte BLAKE3 digest.
6. Byte comparison means unsigned lexicographic comparison of complete byte
   strings.

## 2. Canonical CBOR profile

1. V1 uses the core deterministic encoding profile in RFC 8949 §4.2.1.
2. Every string, byte string, array, and map MUST use a definite length.
3. Integers MUST use the shortest permitted CBOR representation. Floating-point
   values, indefinite lengths, tags, `undefined`, and simple values other than
   `null` are forbidden in signed bytes.
4. A major type 1 (negative integer) argument above `i64::MAX` decodes to a
   value below `i64::MIN` and is outside the v1 profile. Implementations
   MUST reject such an argument rather than decode it.
5. Envelope maps use only the unsigned integer keys listed in this document.
   They MUST contain every listed key exactly once and no other key.
6. Every text string in the profile MUST be valid UTF-8 normalized to Unicode
   NFC before encoding, not only application-defined text keys inside a
   schema payload. Non-NFC user text MUST be carried as a CBOR byte string;
   it MUST NOT be carried as a text string. Maps with application-defined
   text keys are allowed only inside a schema payload, and their keys MUST
   be ordered by their complete encoded CBOR key bytes. Implementations
   MUST reject, rather than normalize, non-NFC bytes received from a peer.
7. Byte strings and text strings MUST be minimally encoded. A text string is
   UTF-8; identifiers are never text strings unless their table says so.

## 3. Envelope grammar

The stored envelope is one CBOR map with exactly eleven keys, in numeric order.
`op_id` is derived and is **not** a serialized field; including it would make
the content address self-referential.

| Key | Name | CBOR representation | Rule |
| ---: | --- | --- | --- |
| 0 | `protocol_version` | unsigned integer | MUST be `1`. |
| 1 | `operation_kind` | unsigned integer | Non-zero `u16`; §6 defines the v1 structural values. |
| 2 | `scope` | map | The map in §3.1. |
| 3 | `author` | map | The map in §3.2. |
| 4 | `capability` | map | The reference in §3.3. |
| 5 | `schema_hash` | `Hash32` | Hash of the schema that validates the decrypted intent payload. |
| 6 | `branch_id` | `Identifier32` | A branch identifier within the verse DAG. |
| 7 | `parents` | array of `Hash32` | Canonically sorted, unique DAG parent operation IDs. |
| 8 | `hlc` | map | The HLC map in §3.4. |
| 9 | `payload` | map | The payload reference in §3.5. |
| 10 | `signature` | byte string, 64 bytes | Ed25519 signature defined in §5. |

### 3.1 Scope map

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `verse_id` | `Identifier32` | Required. |
| 1 | `petal_id` | `Identifier32` or `null` | `null` denotes verse-wide scope. |
| 2 | `resource_id` | `Identifier32` or `null` | MUST be `null` when `petal_id` is `null`. |

The three values form the scope tuple `(verse, petal?, resource?)`. A resource
scope is necessarily within its petal; a petal scope is necessarily within its
verse. Fractal is a materialized hierarchy concern and is not an authorization
scope in this envelope.

### 3.2 Author map

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `did` | UTF-8 text string | V1 accepts canonical `did:key` Ed25519 DIDs only. |
| 1 | `public_key` | byte string, 32 bytes | The raw Ed25519 public key derived from `did`. |

The DID and public key MUST bind exactly. Receivers derive the Ed25519 key from
the `did:key` multicodec value and reject a mismatch. A later identity version
may support other DID methods only by increasing `protocol_version`.

### 3.3 Capability reference map

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `chain_id` | `Hash32` | BLAKE3 ID of the canonical binary capability/delegation chain. |
| 1 | `scope_epoch` | unsigned `u64` | Epoch whose authorization the author relied on. |

The capability chain is an independently addressable artifact. Its canonical
binary certificate grammar, attenuation, expiry, and revocation validation are
specified by SPEC-3. The envelope only commits to its exact artifact and epoch.

### 3.4 Hybrid logical clock map

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `wall_ms` | unsigned `u64` | Unix milliseconds. |
| 1 | `counter` | unsigned `u32` | Logical counter for this wall time. |

The wire HLC is never the packed local-storage integer currently used by
`fe-database`. Ordering for concurrent operations is `(wall_ms, counter,
author.public_key)` using byte comparison on the final term. The pair
`(author.public_key, wall_ms, counter)` MUST identify at most one `op_id`.
A second, different operation with that pair is author equivocation: receivers
MUST retain evidence, quarantine both candidates, and MUST NOT materialize
either without an authorized resolution operation.

### 3.5 Payload reference map

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `ciphertext_hash` | `Hash32` | BLAKE3 of the stored ciphertext artifact. |
| 1 | `ciphertext_length` | unsigned `u64` | Exact ciphertext byte length. |
| 2 | `encryption` | map or `null` | Encryption map below, or `null` only for a no-payload operation. |

When `encryption` is a map, it contains exactly:

| Key | Name | Representation | Rule |
| ---: | --- | --- | --- |
| 0 | `suite_id` | unsigned `u16` | MUST be `1` (XChaCha20-Poly1305); never a per-operation negotiation. |
| 1 | `key_id` | `Identifier32` | Identifier for the scope payload key. |
| 2 | `nonce` | byte string, 24 bytes | Fresh CSPRNG-generated 192-bit nonce for this encryption under `key_id`. |

All non-empty operation payloads and all payload segments MUST be encrypted
under the scope key. The header stays payload-free and may replicate
verse-wide; a peer fetches and decrypts a payload only when its capability and
interest permit it. `ciphertext_hash` intentionally commits to ciphertext, not
plaintext, so headers do not become a plaintext-content oracle.

`encryption = null` is permitted only when the operation kind has no semantic
payload. It MUST use `ciphertext_length = 0` and
`ciphertext_hash = BLAKE3(empty)`. It does not authorize an unencrypted payload.

V1 uses a 32-byte scope key with XChaCha20-Poly1305 (`suite_id = 1`) for every
non-empty payload. A nonce MUST be generated from a CSPRNG immediately before
sealing and MUST NOT repeat for the same scope key; failure to obtain a nonce
or detect a duplicate fails sealing. Scope-key delivery, rotation, and
destruction are specified by SPEC-3 section 10 and SPEC-6 section 9. A
different payload suite requires a new `protocol_version`, not an
operation-by-operation choice. `suite_id = 65535` is reserved solely for the
encoding fixture in this repository and MUST be rejected on production paths.

## 4. Exact scalar units and payload form

1. The canonical scalar is an `i64` measured in nano-base-units.
2. Position is `Nanometers(i64)`; rotation is `Nanodegrees(i64)`; scale is
   `ScalePartsPerBillion(i64)`.
3. Canonical transform vectors contain three CBOR signed integers in X, Y, Z
   order. They MUST NOT contain floats.
4. SDK and WIT interfaces carry these values as `s64`. Renderers may expose
   explicitly approximate `f64` views, but the integer is authoritative.
5. An `f64` view cannot preserve every integer beyond approximately
   ±9.2e15 nanometres (about ±9,200 km). This is a render-tier precision limit,
   never permission to alter canonical data.
6. A decrypted payload is canonical CBOR and is validated by `schema_hash`.
   Its schema defines operation-specific intent fields. Payloads MUST state the
   desired intent only; they MUST NOT capture an old materialized value.

## 5. Signature, operation ID, and encryption binding

### 5.1 Signature preimage

1. `unsigned_envelope` is the canonical encoding of the §3 outer map with keys
   0 through 9 only. Its map length is therefore ten.
2. `signature_preimage` is the exact byte concatenation:

   ```text
   66 65 2d 6f 70 6c 6f 67 2d 76 31 00 || unsigned_envelope
   ```

   The prefix is ASCII `fe-oplog-v1` followed by one NUL byte.
3. `signature` is deterministic Ed25519 signing of `signature_preimage` with
   the author private key. Receivers verify against `author.public_key` only
   after validating the DID-to-key binding.
4. `op_id = BLAKE3(complete_envelope)`, where `complete_envelope` is the exact
   canonical outer map including key 10 and its 64-byte signature. The op ID
   addresses exactly the artifact that a peer stores and relays.

### 5.2 Payload AEAD associated data

For an encrypted payload, AEAD associated data is:

```text
ASCII("fe-oplog-payload-aad-v1") || 00 || payload_aad_envelope
```

`payload_aad_envelope` is the canonical outer map with keys 0 through 8 plus a
key 9 value containing only the `encryption` map. It omits ciphertext hash and
length to avoid a circular dependency. The signature later binds the full
payload reference, including those two omitted values. A receiver MUST use
XChaCha20-Poly1305 with the resolved 32-byte scope key, the 24-byte nonce, and
this exact AAD; it MUST verify AEAD authentication before materialization.

## 6. DAG and structural operation rules

1. `parents` MUST be sorted strictly ascending by complete 32-byte hash and
   contain no duplicate.
2. Every listed parent MUST have the same `scope.verse_id` and be admitted or
   held in the bounded missing-parent quarantine defined by SPEC-5.
3. A `branch_genesis` operation has `operation_kind = 2` and zero parents.
4. A normal intent operation has `operation_kind = 1`, at least one parent, and
   an encrypted non-empty payload reference.
5. A detached-to-tracking merge has `operation_kind = 3`, at least two parents,
   and MUST use the no-payload form in §3.5. Its `branch_id` is the tracking
   target. The actual merge admissibility policy is specified by SPEC-5.
6. A `scope_epoch_bump` operation has `operation_kind = 4`, exactly one parent,
   an exact revoked scope in its header, and the no-payload form in §3.5. The
   authorizes-at-epoch value is `e`; replay advances that scope to epoch
   `e + 1`. Its authority, replay, and observer rules are specified by SPEC-3.
7. Other non-zero operation kinds require a registered schema and payload rule.
   Unknown kinds or unknown `schema_hash` values MUST be quarantined, never
   materialized or silently reinterpreted.
8. Parent reachability, not HLC order, establishes causality. HLC provides a
   deterministic concurrent ordering used by the CRDT materializer.

## 7. Golden-vector conformance

The machine-readable vectors are in
[`operation-envelope-v1.json`](operation-envelope-v1.json). The included
dependency-free conformance check decodes every CBOR field, reconstructs
canonical bytes from semantic values, and then validates the committed
cryptographic values.
A future codec's conformance test MUST, for every vector:

1. decode and re-encode `unsigned_envelope_cbor_hex` byte-for-byte;
2. construct the signature preimage from §5.1 and verify the Ed25519 signature;
3. decode and re-encode `complete_envelope_cbor_hex` byte-for-byte;
4. BLAKE3 the complete envelope and match `op_id_hex`;
5. BLAKE3 the fixture ciphertext and match `ciphertext_hash_hex`; and
6. decode and re-encode the schema payload fixture byte-for-byte where present.

The vectors cover a Unicode text key, unsigned integer edges, i64
quantization edges, a no-payload merge, `branch_genesis`, `scope_epoch_bump`,
and the exact `payload_aad_envelope` construction. The non-empty payload vector
uses the fixture-only suite 65535; it tests encoding and binding, not a
production AEAD choice.

## 8. Design notes

- **Headers before payloads:** D-CL2 requires a single verse DAG while keeping
  petal payloads sparse. This split gives every peer causal headers without
  giving every peer every encrypted payload.
- **Derived `op_id`:** D-CL8 hashes signed bytes, so the content address is
  stable and does not create a self-reference inside CBOR.
- **No float migration loophole:** Transform floats in current handlers are
  display inputs. They must be explicitly quantized before a future envelope
  encoder is invoked; they are never signed directly.
- **Intent-only payloads:** Undo is a derived log index. Capturing an old row
  value would make a signed intent depend on one materialization order.

### Errata

- **E1 (2026-08-09):** §6 rule 5 changed the detached-merge no-payload form
  from MAY to MUST. A merge reconciles lineage and does not carry new
  intent; intent belongs in a separate operation whose parent is the merge.
- **E2 (2026-08-09):** §2 rule 6 extended the NFC requirement to every text
  string in the profile, not only application-defined text keys inside a
  schema payload, and required non-NFC user text to be carried as a byte
  string rather than a text string. The oracle and the Rust decoder already
  enforced the broader rule.
- **E3 (2026-08-09):** §2 rule 4 added a normative rejection of major-type-1
  arguments above `i64::MAX`. The Rust decoder already rejected them; this
  makes the restriction normative rather than an implementation artifact.

## 9. Ratified encryption contract

D-CL17 fixes V1 payload encryption as XChaCha20-Poly1305 with a fresh random
192-bit nonce and a 32-byte scope key. The scope key is delivered only in a
recipient-device X25519 HPKE-style wrap after current authorization validation,
and it rotates on every epoch bump. `suite_id = 65535` remains test-only. This
document authorizes local implementation only. Peer delivery, relay service, and
network enablement remain blocked by a separate owner gate.
