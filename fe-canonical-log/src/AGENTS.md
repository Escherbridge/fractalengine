# fe-canonical-log

Workstream G, the Canonical Fractal Data Log. This crate owns the immutable operation
artifact: its canonical bytes, its signature, its content address, and the admission rules
that decide whether a received header may be materialized.

Normative sources, in precedence order:

1. `docs/spec/canonical-log/operation-envelope-v1.test.mjs` — the conformance **oracle**.
   Where the prose is ambiguous, its encoder/decoder behavior wins.
2. `docs/spec/canonical-log/operation-envelope-v1.json` — the four golden vectors.
3. `docs/spec/canonical-log/operation-envelope.md` — SPEC-1, the envelope prose.

The JSON and the `.mjs` file are read-only fixtures. Code changes to match them; they never
change to match code.

## Forbidden dependencies

This crate MUST NOT depend on `fe-runtime`, `fe-identity`, `fe-policy`, `fe-database`, or
`surrealdb`. Those pull in bevy, tokio, iroh, and keyring transitively, which would make the
log's byte-exactness tests hostage to a multi-minute engine build and would drag a
platform-specific credential store into a pure protocol crate. The only workspace edge is
one-directional: `fe-canonical-log -> fe-sdk`. `fe-sdk` is serde-only by design, so the edge
stays cheap and never becomes a cycle. Anything the log needs from identity or policy arrives
as plain bytes or as a trait the caller implements, never as a dependency.

`fe-identity` already has a `did:key` codec. `src/did_key.rs` deliberately reimplements it
rather than importing it, for exactly the reason above; the two are kept byte-compatible by a
test in each crate against the same fixture DID.

## §cbor — why the codec is hand-rolled

`ciborium` is a correct CBOR library and the wrong tool here. Its serde output is not the
RFC 8949 §4.2.1 deterministic profile:

- it does not guarantee map keys are emitted in ascending canonical key-byte order;
- on decode it accepts non-minimal integer and length arguments, indefinite lengths, floats,
  tags, and non-NFC text — every one of which SPEC-1 §2 requires us to *reject*, not repair.

So using it would still require a full canonicalization-and-validation layer on top, and that
layer plus the library's own encoder would be two independent places where the emitted bytes
could drift. Since `op_id = BLAKE3(complete_envelope)` and the signature covers exact bytes,
byte drift is a consensus fork, not a cosmetic bug. One hand-written codec with the profile
enforced in a single place, tested directly against the golden vectors, is the smaller risk.

Design notes:

- `CborValue` is a **closed** value model: `Uint`, `NegInt`, `Bytes`, `Text`, `Array`, `Map`,
  `Null`. There is no `Float`, `Tag`, or `Undefined` variant, so a forbidden value is not
  merely rejected at the boundary — it cannot be constructed anywhere in the crate.
- Rejection is per-reason. `CborError` has a distinct variant for each rule (indefinite
  length, non-minimal argument, reserved additional information, tag, float/simple value,
  duplicate key, unsorted keys, non-UTF-8, non-NFC, trailing bytes, out-of-range negative
  integer, oversized length, depth limit). Later slices map these into their own admission
  error taxonomies and need to distinguish "peer sent junk" from "peer sent a non-canonical
  re-encoding of something valid", so a single catch-all variant would be useless to them.
- **Map ordering is plain bytewise** comparison of each key's own complete canonical
  encoding — *not* length-first. `Uint(1000)` (`19 03 e8`) therefore sorts before `Text("")`
  (`60`). A length-first sort is the classic canonical-CBOR mistake and would silently
  produce a different `op_id`; a test pins the distinguishing case.
- **NFC is checked on every text string in the tree**, including map keys and values nested
  arbitrarily deep, per SPEC-1 §2 rule 6 (errata E2) and matching the oracle's `assertNfc`.
  Non-NFC user text must be carried as a byte string, never as a text string.
  Non-NFC text is rejected, never normalized, on the decode path.
  `CborValue::text()` normalizes on the *construction* path, which is the only place where
  normalizing is safe.
- There are two encoders. `encode_canonical_checked` is the one every signing and hashing
  path uses; it rejects the two shapes that would otherwise encode to bytes this crate's own
  decoder refuses — a `Text` that is not already NFC, and a `Map` holding two keys with
  identical canonical encodings. Because `CborValue`'s variants are public, either shape is
  constructible by a later slice, and the infallible encoder would hand it canonical-*looking*
  bytes whose rejection surfaced at the receiving peer rather than at the author.
  `encode_canonical` stays infallible for internal round-trip use, where the input is always
  a value that already survived the bounded decode; it carries a debug assertion for the same
  two shapes so a mistake fails in test builds rather than on the wire. Both sort map keys
  rather than requiring them pre-sorted, and `NegInt` holding a non-negative value encodes as
  major type 0, so a mis-built integer still yields canonical bytes.
- `MAXIMUM_NESTING_DEPTH` bounds decode recursion. The decoder runs on unauthenticated peer
  bytes before any signature check, so unbounded nesting would be a free stack-overflow DoS.
  Encoding is not depth-bounded because its input is always a value this crate built or one
  that already survived the bounded decode.
- Declared lengths are checked against the bytes actually remaining *before* allocating, so a
  hostile 8-byte length argument cannot trigger a large allocation.

## §did_key — self-contained did:key codec

Multibase base58btc (`z` prefix), multicodec prefix `[0xed, 0x01]`, exactly 32 bytes of
Ed25519 public key. SPEC-1 §3.2 accepts canonical `did:key` Ed25519 DIDs only; other DID
methods require a `protocol_version` increase, not a wider parser here. The algorithm mirrors
`fe-identity/src/did_key.rs` and is pinned to `signing_key.did` /
`signing_key.public_key_hex` from the golden-vector JSON.

Deliberate differences from `fe-identity`: a typed `DidKeyError` instead of `anyhow`, and raw
`[u8; 32]` instead of `ed25519_dalek::VerifyingKey`, so the envelope's DID-to-key binding
check is a byte comparison and does not depend on point decompression succeeding first.

## §ingress — `decode_and_admit` is the only door for peer bytes

`signing::decode_and_admit(bytes) -> (CompleteEnvelope, Hash32)` is the crate's real ingress
primitive, and any slice handling bytes that arrived from a peer MUST use it. In order it:
decodes canonically; re-encodes and asserts byte equality with the received slice; checks the
§3.2 author binding; verifies the §5.1 Ed25519 signature; applies the §6 structural rules;
refuses every non-production payload suite; and returns `op_id` computed over the **received**
bytes.

The re-encode assertion is the part that is easy to skip and expensive to lose. `op_id` is a
content address: it must name the artifact the peer actually stored and relayed, not a
re-serialization of whatever this crate's parser happened to keep. Today `decode_canonical` is
strict enough that decode-then-encode is the identity on every accepted byte string, so the
assertion never fires; that is exactly why it must stay. The day a struct field stops
round-tripping — a widened integer, a normalized string, a dropped map entry — the assertion
is what turns a silent content-address fork into a rejected message.

`CompleteEnvelope::decode_canonical` plus `verify_envelope`, and `op_id_of`, skip the
byte-equality assertion, the structural rules, and the suite check. They are for
**locally-constructed** envelopes only, where this process built the value and the bytes do
not exist yet.

`EncryptionParams::assert_production_suite` is called from every path that admits or seals bytes
and from no path that is reachable only by opt-in: `decode_and_admit` (SPEC-1 ingress),
`segment::artifact::admit_sealed` (SPEC-6 ingress), and
`segment::relay_policy::assert_seals_under_current_key`, which every seal reaches through
`seal_artifact`. That is the invariant — mandatory on each such path — not a caller count. A suite
check reachable only by opt-in reads as enforced without being enforced, which is how this
repository has previously shipped dormant gates; adding a fourth mandatory path is fine, making
any of these three optional is not.

`SigningError::NonCanonicalAuthorDid` is the same kind of guard one level down. §3.2 accepts
canonical `did:key` only, so the binding check compares the derived key bytes *and* asserts
the DID string is the canonical text for that key. `did_key::did_to_public_key` currently
admits only the canonical base58btc spelling, so the round-trip guard is unreachable through
it; it exists so a future relaxation of that parser cannot silently admit an alias whose
`op_id` and author identity disagree.

## §scope — invalid tuples are unconstructible

`Scope`'s three fields are private. §3.1 forbids a non-null `resource_id` under a null
`petal_id`, and `Scope::contains` drives authorization decisions in SPEC-3: with public fields
an invalid tuple was constructible and `contains` answered nonsense for it, silently widening
or narrowing authority. Every constructor (`new`, `from_cbor`) validates, `verse_wide` cannot
produce an invalid tuple, and `contains` still guards both operands explicitly so the answer
is `false` rather than nonsense if a future in-module change reintroduces one.

## §module-ownership

`lib.rs` declares the complete module tree up front so no later slice has to edit it, and
`Cargo.toml` declares the complete dependency set for the same reason. Both files are
effectively frozen after W1a.

Several declared dependencies therefore have **no caller yet** — `data-encoding`, `zeroize`,
`rand`, `async-trait`, `chacha20poly1305`, `x25519-dalek`, `ulid`, `serde`, `fe-sdk`. That is
deliberate, not drift: they belong to modules the ownership table below assigns to later
slices, and freezing the manifest is what keeps those slices from serializing on edits to one
shared file. A dependency audit must not strip them. `x25519-dalek` carries the
`static_secrets` feature because `StaticSecret` is feature-gated there and the SPEC-3 §10
HPKE-style scope-key wrap needs a long-lived recipient secret, not an ephemeral one.
`serde_json` is a **dev**-dependency: it reads the golden-vector JSON in tests and no library
code parses JSON.

| Module | Spec anchor | Owner |
| --- | --- | --- |
| `cbor` | SPEC-1 §2 | W1a — implemented |
| `did_key` | SPEC-1 §3.2 | W1a — implemented |
| `kind` | SPEC-1 §6 | W1b |
| `units_codec` | SPEC-1 §4 | W1b |
| `envelope` | SPEC-1 §3 | W1b |
| `signing` | SPEC-1 §5.1 | W1b |
| `payload_aad` | SPEC-1 §5.2 | W1b |
| `frontier` | SPEC-1 §6 | W1b |
| `author_key` | SPEC-2 | W2-author-key-lifecycle — implemented |
| `capability` | SPEC-3 | W2-capability-chain — implemented |
| `crypto` | SPEC-1 §9, SPEC-3 §10 | W3-crypto-aead-keywrap — implemented; W3R-crypto — authority hardening |
| `materialize` | SPEC-4 | W2-materialize — implemented |
| `branch` | SPEC-5 | W2-branch-checkpoint — implemented |
| `retention` | SPEC-5 | W2-retention — implemented |
| `checkpoint` | SPEC-5 | W2-branch-checkpoint — implemented |
| `segment` | SPEC-6 | W2-segment — implemented |
| `wire` | SPEC-7 | W2-wire — implemented |
| `compose` | cross-cutting facade | Wave 3 integration |

Unimplemented modules are one-line doc-comment placeholders. They exist so `lib.rs` compiles
today and so module boundaries are settled before parallel slices start writing code.

Each implemented module carries its own `AGENTS.md` holding that module's rationale. This
file stays the crate-wide register: read it first, then the module file.

`compose` stopped being a placeholder in the R-unify pass. It is not a "high-level facade" any
more; it is the one place a vocabulary or a binding that spans two modules is defined, so that
two leaf slices cannot each invent their own. See §unified-vocabularies.

## §unified-vocabularies — one name per concept, defined in `compose`

Wave 2 shipped three parallel `QuarantineReason` enums and three unbound checkpoint
vocabularies. Duplicated vocabularies are not a cosmetic problem here: a two-variant local
stand-in for a real verification result is a gate that anyone can satisfy by typing the word
`Verified`, and two enums for one concept mean a reviewer who checks one has not checked the
other.

- **`compose::QuarantineReason` is the crate's only quarantine vocabulary.**
  `author_key::admission`, `retention::quarantine`, and `segment::store` each `pub use` it and
  none defines its own. It retains `AuthorEquivocation` per D-CL25, and that variant now names
  **both** conflicting operations rather than "the other one": an asymmetric reason invited a
  reader to treat the named side as the loser, which is precisely the fork §3.4 forbids
  adjudicating. The reason never carries the candidate's own `op_id` — every store already keys
  it by that.
- **`QuarantineReason::class` is the crate's only budget mapping** (erratum G1). Every reason
  maps to exactly one `QuarantineReasonClass`, through a total match with **no wildcard arm**:
  a new reason variant does not compile until its author states which budget it spends, so no
  future reason can quietly join and dilute the residual class. `retention::PerReasonBudgets`
  is a struct with a field per class rather than a map, because a map can omit a class and an
  omitted class is either unbounded or silently zero — neither being a decision an operator
  should make by accident.
- **`checkpoint::compaction_decision` is the crate's only compaction gate.** It takes the real
  `CheckpointVerification` produced by `verify_checkpoint_claim` (signature, Manager+ authority,
  frontier and manifest bindings, independent replay), plus the tombstone records themselves as
  a required parameter, and runs `retention::tombstone::assert_no_resurrection` over each. The
  old `retention::tombstone::CheckpointVerdict{Verified,NotVerified}` stand-in is deleted, and
  `BootstrapCoverage` no longer carries an `every_suppression_effect_is_checkpoint_proved` bool
  — that evidence is inspectable, so it is inspected. `may_compact_tombstone` survives only as
  a single-record spelling that delegates to the same function.
- **`segment::checkpoint_proof::CheckpointView` binds to a real signed claim through
  `compose::SignedCheckpointProofView`.** The trait gained a required `frontier_commitment()`
  method with **no default**: `run_proof` recomputes `SortedFrontier::commitment` over the heads
  it is about to walk and refuses `FrontierCommitmentMismatch` when the signature covered a
  different selection. A defaulted method returning the recomputation would have made the check
  vacuous, which is why it is required.
- **`materialize::CheckpointBinding` is derived from the claim, not restated beside it.**
  `compose::checkpoint_binding_for_selection` is the one function that turns a SPEC-5
  `CheckpointClaimV1` into a SPEC-4 §6.3 binding, and it is **fallible**: it builds the binding
  from the claim's own fields, so the two vocabularies cannot disagree by construction, and then
  refuses to hand it back unless `CheckpointBinding::validate` has recomputed the frontier
  commitment over the caller's own frontier and matched branch, manifest and materializer against
  the caller's current values. There is deliberately no separate "and also check these agree"
  helper — that would be exactly the gate a future author forgets. SPEC-5 names a materializer by
  32-byte identifier and SPEC-4 by hand-authored name/number, so `BindingSelection` carries both
  and only the caller holds the mapping.

## §single-seal-entry-point — all four lanes seal through one door

`segment::artifact::seal_artifact` is the ONLY way to obtain a `SealedArtifact`;
`SealedArtifact::seal` is `pub(crate)`. In order it refuses a lane/body-class mismatch, then the
production suite plus the lane's current scope epoch and current key through
`relay_policy::assert_seals_under_current_key`, then a nonce already seen under that key through
`NonceLedger::record_fresh`.

Before R-unify only the payload shard was checked, and only partially: `seal_payload_shard` took
a bare `current_key_id` and could not see the lane's epoch, the header segment, HashSeq nodes
and the manifest went through the ungated `SealedArtifact::seal`, and
`assert_seals_under_current_key` had no caller at all. Nonce reuse under XChaCha20-Poly1305 is a
keystream-recovery break and sealing under a retired epoch key defeats the whole D-CL17
revocation story, so neither check may be reachable only by opt-in.

`seal_payload_shard` now takes the `RelayAuthorizationView` and derives its lane and epoch from
the shard's own topic, so it cannot be asked about a lane the shard does not belong to. It no
longer records the nonce: the nonce belongs to the sealed artifact, so packing a candidate twice
must not burn a nonce the AEAD never used.

## Provisional wire numbering

D-CL24 register. Every CBOR integer key this crate invented — because its spec describes a map
in prose without a normative key table — is recorded below. **No cross-implementation interop
is claimed for any provisional key.** The owner ratifies or replaces them before any second
implementation reads these bytes.

**This section is the single ratification surface.** Every row is here, in full. The earlier
arrangement — an index here, the row-by-row tables in five module files — meant the owner had to
visit six documents to ratify one numbering scheme, and a slice could add a key without ever
touching the register. Module `AGENTS.md` files must carry a pointer to this section, not a
second copy of a table; two copies of a key table drift exactly the way two canonical encoders
drift.

`retention` and `compose` assign no wire numbers at all; nothing in either is CBOR-encoded,
signed, or hashed as a wire artifact.

### Normative, not ours to renumber

| Artifact | Spec table |
| --- | --- |
| Operation envelope, keys 0..10 | SPEC-1 §3 (see §W1b below and `envelope.rs`) |
| Capability certificate | SPEC-3 §2.2 keys 0..14 |
| Capability caveats | SPEC-3 §2.3 keys 0..5 |
| Capability chain | SPEC-3 §2.4 keys 0..2 |
| Topic label | SPEC-3 §6.1 keys 0..4 |
| Rotation payload, rotation statement | SPEC-2 §3, §3.1 |
| Disavow payload | SPEC-2 §6.2 |
| Branch-control payload, keys 0..3 | SPEC-5 §2.2 rule 4 (`0` action, `1` target_branch_id, `2` selected_frontier, `3` source_branch_id) |
| Wire common frame | SPEC-7 §2.1: `0` wire_version, `1` message_type, `2` request_id, `3` body |
| `commit_submit` | SPEC-7 §4.1: `0` session_generation, `1` authorization_binding_id, `2` claimed_op_id, `3` complete_envelope, `4` payload_ciphertext |

### Provisional — SPEC-2 (`author_key`)

`disavow_rescind_payload` (§6.1 rule 1 is prose):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `disavow_op_id` | byte string, 32 bytes |
| 1 | `reason_code` | unsigned `u16` |

`continuity_grant_statement` (§9.2 is prose):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `protocol_version` | unsigned integer, MUST be 1 |
| 1 | `verse_scope` | map, SPEC-1 §3.1 grammar, MUST be verse-wide |
| 2 | `lost_principal_did` | UTF-8 text |
| 3 | `lost_principal_public_key` | byte string, 32 bytes |
| 4 | `new_principal_did` | UTF-8 text |
| 5 | `new_principal_public_key` | byte string, 32 bytes |
| 6 | `reason_code` | unsigned `u16` |

`continuity_grant_payload` (§9.2 is prose):

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `statement` | the map above |
| 1 | `new_principal_signature` | byte string, 64 bytes |

### Provisional — SPEC-3 (`capability`)

| Structure | Spec text | Provisional encoding | Why a choice was needed |
| --- | --- | --- | --- |
| `canonical_scope_key_context` | §6, "the deterministic-CBOR pair of the exact scope map and its epoch" | two-element CBOR array `[scope_map, topic_epoch]`, epoch unsigned | "pair" names no container; an array is smaller and order-fixed against a two-key map whose keys would themselves need numbering |

### Provisional — SPEC-3 §10.2 scope-key wrap (`crypto/key_wrap.rs`)

Lifted here from `crypto/AGENTS.md` by W3R-crypto; that file now carries only a pointer.
**Ours, unratified, no interop claimed.**

| Key | Name | Representation |
| ---: | --- | --- |
| 0 | `associated_data` | map |
| 1 | `wrap_nonce` | byte string, 24 bytes |
| 2 | `sealed_key` | byte string, 48 bytes |
| 3 | `issuer` | principal |
| 4 | `issuer_capability` | capability reference |

`ScopeKeyWrap` is keys 0..4 of the body above, plus key 5 `signature`, byte string 64 bytes.

**The signed preimage is our reading, and it is unratified.** §10.2.3's phrase
`canonical_complete_wrap` read literally is circular — the complete wrap cannot contain the
signature computed over the complete wrap. Per SPEC-1 §5.1's unsigned/complete convention we
read the signed preimage as the five-key body *without* the signature slot, with the artifact
adding key 5; on that reading `ScopeKeyWrapBody` is what the spec calls the complete wrap.
A second implementation following the literal text would compute a different preimage.

### Provisional — SPEC-5 checkpoint claim v1 (`checkpoint.rs`)

§3.1 rule 2 gives the bindings as a prose table with no integer keys and rule 3 defers them to
"the implementation package", so the keys follow that table's row order, flattening rows that
name two or three values. The signature sits one past the last unsigned key, mirroring the
operation envelope's key 10.

| Key | Field | Key | Field |
| ---: | --- | ---: | --- |
| 0 | `checkpoint_version` | 7 | `projection_root_hash` |
| 1 | `verse_id` | 8 | `authorization_view_root` |
| 2 | `branch_id` | 9 | `snapshot_manifest_id` |
| 3 | `frontier_commitment` | 10 | `signer` |
| 4 | `segment_manifest_id` | 11 | `capability` |
| 5 | `materializer_id` | 12 | `issued_hlc` |
| 6 | `materializer_version` | 13 | signature |

### Provisional — SPEC-6 (`segment`)

SPEC-6 describes every one of these maps in prose and gives no normative integer-key table.

Sealed outer map (§2.1.2) — `artifact::SealedArtifact`. It carries nothing else: no verse ID,
petal ID, resource ID, Hexon URI, capability chain, or unblinded topic name.

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | `format_version` (always 1) | uint |
| 1 | `lane_class` | uint |
| 2 | `ciphertext_length` | uint |
| 3 | `encryption` descriptor | map (SPEC-1 §3.5 shape, reused verbatim) |
| 4 | `ciphertext` | bytes |

Lane class values — `artifact::LaneClass`. Each inner body restates its own lane class at key 0,
which is how the §2.1.5 lane/body mismatch is detected without a second discriminant.

| Value | Lane | Value | Lane |
| ---: | --- | ---: | --- |
| 1 | header segment | 3 | HashSeq node |
| 2 | payload shard | 4 | segment manifest |

Header segment body (§3.1) — `header_lane::HeaderSegmentBody`. `op_id` is not carried: it is
BLAKE3 of the record's own bytes and is always recomputed, so there is nothing for a sender to
lie about. Records are ordered strictly ascending by derived `op_id` — also provisional, since
§3.1.1 says "ordered set" without fixing the order.

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | lane class (always 1) | uint |
| 1 | `verse_id` | bytes(32) |
| 2 | records | array of complete SPEC-1 envelope byte strings |

Payload topic scope (§3.2.2) — `payload_shard::PayloadTopicScope`:

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | `verse_id` | bytes(32) |
| 1 | `petal_id` | bytes(32) |
| 2 | `scope_epoch` | uint |
| 3 | `key_id` | bytes(32) |

Payload shard record (§3.2.1) — `payload_shard::PayloadShardRecord`:

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | `op_id` | bytes(32) |
| 1 | `ciphertext_hash` | bytes(32) |
| 2 | `ciphertext_length` | uint |
| 3 | `ciphertext` | bytes |

Payload shard body (§3.2) — `payload_shard::PayloadShardBody`. Strict ascending record order is
provisional: §3.2.3 fixes uniqueness but not order, and a fixed order makes the body canonical
and duplicate detection a single comparison.

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | lane class (always 2) | uint |
| 1 | topic scope | map |
| 2 | records | array, strictly ascending by `op_id` |

HashSeq node body (§4.1.1) — `hashseq::HashSeqNode`. Entry map: key 0 `artifact_id` bytes(32),
key 1 `stored_length` uint.

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | lane class (always 3) | uint |
| 1 | indexed lane key | array |
| 2 | `predecessor_id` | bytes(32) or null |
| 3 | entries | array of maps, sealing order preserved |

Lane key — `hashseq::LaneKey`, encoded as a two-element array so it can serve as a canonical CBOR
map key in the manifest: header lane `[0, verse_id]`, payload lane
`[1, payload_topic_scope_map]`.

Segment manifest body (§3.3) — `manifest::SegmentManifestBody`. Boundary map: key 0
`oldest_required_node` bytes(32), key 1 `stored_length` uint.

| Key | Field | CBOR type |
| ---: | --- | --- |
| 0 | lane class (always 4) | uint |
| 1 | `protocol_version` | uint |
| 2 | `verse_id` | bytes(32) |
| 3 | `branch_id` | bytes(32) |
| 4 | header roots | map: artifact ID bytes(32) → stored length uint |
| 5 | payload roots | map: topic map → root-set map |
| 6 | availability boundary | map: lane key array → boundary map |
| 7 | clear statistics (§3.3.5) | map, see below |
| 8 | sealed statistics refs (§3.3.6) | map: lane key array → reference map |

Clear statistics map: key 0 `min_hlc` HLC map, key 1 `max_hlc` HLC map, key 2 `petals`
array of bytes(32) **strictly ascending**, key 3 `operation_count` uint. Sealed statistics
reference map: key 0 `artifact_id` bytes(32), key 1 `stored_length` uint.

Two manifest shapes are provisional beyond the numbering. §3.3.1 requires "a sorted set of
header HashSeq roots" and §3.3.2 requires exact stored byte lengths; a *set* cannot carry
lengths, so roots are a map from artifact ID to stored length and a repeated ID with a
conflicting length is refused rather than deduplicated. §4.1.4 requires a declared availability
boundary without fixing its shape; the shape chosen is one oldest-required node per lane, and a
manifest is invalid unless the boundary covers exactly the lanes that carry roots.

Keys 7 and 8 are erratum G4. The split is on **derivation**, not usefulness: HLC, scope, and
count come from signed header fields, so they may sit in a manifest body that §2.2.2 seals under
the verse-wide header scope and every verse member can read; anything derived from payload
plaintext — column minima/maxima, histograms, bloom filters — must live in a separate artifact
sealed under the lane's own scope key, reachable from here only as an artifact ID and a length.
A minimum and maximum on a position column would otherwise publish a project's real-world
coordinates to every verse member with no payload capability. `SegmentStatistics` is a required
constructor parameter rather than an `Option` because a manifest with no range is one no peer
can skip, so an omitted block would degrade selective fetch to fetch-everything silently. The
petal array is decoded with an explicit strictly-ascending check rather than left to the
manifest's re-encode pin, so the diagnostic names the real defect.

Per-lane AEAD AAD domains (§9.1), reserved for Wave 3. Each lane's authenticated encryption binds
`ASCII(domain) || 0x00 || canonical_outer_metadata`, where the metadata is the sealed outer map
with the ciphertext (key 4) omitted. The domains are NUL-terminated so no domain can be a prefix
of another, matching the SPEC-1 §5.1 convention.

| Lane | Domain | Lane | Domain |
| --- | --- | --- | --- |
| header segment | `fe-segment-header-v1` | HashSeq node | `fe-segment-hashseq-v1` |
| payload shard | `fe-segment-payload-shard-v1` | segment manifest | `fe-segment-manifest-v1` |

SPEC-6 §7 discovery contributes **no** labels of its own any more. The four §7 traffic kinds map
onto the three normative SPEC-3 §6.1 lanes; see `segment/AGENTS.md` §discovery-lanes.

### Provisional — SPEC-7 bodies (`wire`)

Every body below is that slice's own assignment, in declaration order, and is not an interop
claim. §2.2/§4.2/§4.3/§5.1/§5.2/§7.1 carry no key tables.

| Body | Keys |
| --- | --- |
| `authorize` | `0` capability_chain_bytes, `1` authorization_binding_id, `2` requested_verb, `3` requested_object_class, `4` requested_scope |
| `authorized` | `0` authorization_binding_id, `1` session_generation, `2` leaf_principal, `3` chain_id, `4` epoch_scope, `5` scope_epoch, `6` expires_at_ms |
| `authorization_revalidation_required` | `0` authorization_binding_id, `1` scope, `2` invalidated_session_generation, `3` reason |
| `commit_ack` | `0` session_generation, `1` authorization_binding_id, `2` claimed_op_id, `3` state (`0` rejected, `1` accepted_pending_materialization, `2` committed, `3` already_committed), `4` category (rejected only), `5` branch_id, `6` scope, `7` projection_identity, `8` cursor (committed/already_committed only). Keys `5`..`8` are structurally ABSENT, not null, for every other state |
| `commit_delta` | `0` subscription_id, `1` session_generation, `2` op_id, `3` branch_id, `4` scope, `5` projection_identity, `6` cursor, `7` change_summary |
| `subscribe` | `0` session_generation, `1` authorization_binding_id, `2` subscription_id, `3` branch_id, `4` scope, `5` projection_identity |
| `resume` | `0` session_generation, `1` subscription_id, `2` prior_cursor |
| `replay_complete` | `0` subscription_id, `1` session_generation, `2` cursor |
| `snapshot_required` | `0` subscription_id, `1` session_generation, `2` reason (`0` broadcast_lagged, `1` cursor_unavailable, `2` cursor_invalid, `3` projection_changed, `4` replay_limit, `5` authorization_changed) |
| `scene_snapshot` | `0` subscription_id, `1` session_generation, `2` branch_id, `3` scope, `4` projection_identity, `5` snapshot_cursor, `6` view_bytes |
| `snapshot_ack` | `0` session_generation, `1` subscription_id, `2` snapshot_cursor |
| `preview_send` | `0` session_generation, `1` authorization_binding_id, `2` scope, `3` preview_sequence, `4` preview_kind, `5` expires_at_ms, `6` preview_data |
| `preview_delta` | `0` sender_principal, `1` scope, `2` preview_sequence, `3` preview_kind, `4` expires_at_ms, `5` preview_data |
| `preview_dropped` | `0` scope, `1` preview_sequence (key ABSENT, not null, when unknown), `2` reason (`0` rate_limited, `1` overloaded) |
| `protocol_error` | `0` category, and nothing else by design (§6.3 rule 3 — any further diagnostic could disclose whether a private operation, artifact, branch, or cursor exists). Discriminants `0`..`19` live in `wire/error.rs`'s `to_u64`/`from_u64` |

Reserved preview keys (`wire/preview.rs::RESERVED_PREVIEW_KEYS`), permanent and never assignable
to a real field in any preview body: `90` op_id, `91` signed_envelope_bytes, `92` parent_ids,
`93` branch_id, `94` hlc, `95` payload_ciphertext, `96` payload_hash, `97` checkpoint_identity,
`98` durable_cursor.

Two product numbers SPEC-7 requires but never states — the default preview rate cap and the
resume replay limit — are caller-supplied constructor parameters with no `Default` picking a
value, per D-CL24/M7. The same rule governs every quarantine bound, GC lease duration and
retention window in `retention`.

## Fixture suite 65535 is a runtime assertion, not a feature

`suite_id = 65535` appears in the payload-bearing golden vector and is reserved solely for
that fixture. It must be rejected on production paths. That rejection is a plain runtime
check, deliberately **not** a cargo feature: a `#[cfg]` gate would make the rejection
disappear from the compiled artifact whenever the feature was enabled, which is precisely the
build the check exists to prevent. There is no `[features]` table in this crate.

## Envelope invariants later slices must not relitigate

- The nonce in the `encryption` map is **unconditionally 24 bytes** (SPEC-1 §3.5,
  XChaCha20-Poly1305, 192-bit). The golden vector was regenerated to 24 bytes; there is no
  suite-conditional nonce length.
- `op_id` is derived, never serialized. It is `BLAKE3` over the *complete* envelope, key 10
  and its 64-byte signature included.
- The signature preimage is `ASCII("fe-oplog-v1") || 0x00 || unsigned_envelope`, where
  `unsigned_envelope` is the ten-key map.
- The payload AAD preimage is `ASCII("fe-oplog-payload-aad-v1") || 0x00 ||
  payload_aad_envelope`, where key 9 holds a **one-entry** map containing only key 2, the
  `encryption` map. Ciphertext hash and length are omitted there to break the circular
  dependency, and the signature over the full envelope is what binds them.
- `encryption.suite_id` is decoded as strictly greater than zero. Zero is not a suite the
  registry may ever assign; the oracle rejects it, and admitting it here would let a
  zero-initialized or truncated map read as a well-formed suite selection.

## §W1b — primitives later slices inherit

- **`EquivocationKey` (§3.4)** is `(author.public_key, wall_ms, counter)`. That triple MUST
  identify at most one `op_id`. Two distinct `op_id`s sharing one `EquivocationKey` is author
  equivocation, and the rule is **quarantine both, materialize neither**: a receiver retains
  the evidence and MUST NOT pick a winner, because picking one is precisely the fork the
  author was attempting. Only an authorized resolution operation releases the quarantine.
  This crate ships the identity type; SPEC-4's materializer enforces the rule, and it is an
  invariant, not a policy knob.
- **`SortedFrontier` is the single D-CL19 frontier commitment.** It is non-empty, strictly
  sorted, deduplicated, and commits as BLAKE3 over the concatenated 32-byte IDs in
  byte-lexicographic order. SPEC-4 through SPEC-7 all commit to a frontier through
  `SortedFrontier::commitment` and no later slice may define a second frontier ordering or a
  second commitment function. Two orderings would mean two commitments for one DAG state,
  which is a silent partition between peers that each believe they agree.
- **`sign_domain` / `verify_domain` are the generic artifact-signing seam.** Every signed
  artifact in the log family — envelopes today, capability chains, checkpoints, and segment
  headers later — is `Ed25519(domain || body)` with a distinct NUL-terminated ASCII domain.
  Keeping the pair generic means a future artifact gets its own domain separator instead of a
  copy of the signing code, and the NUL terminator is what stops one domain from being a
  prefix of another.
- **`to_cbor` is fallible on the aggregate types** (`Scope`, `PayloadRef`, `UnsignedEnvelope`,
  `CompleteEnvelope`) and infallible on the leaf ones. Those four carry cross-field invariants
  — resource-without-petal, the exact no-payload form, the protocol version, strictly
  ascending parents — that a plain struct cannot enforce at construction. Encoding is the last
  moment before bytes become signable, so it is the right place to refuse, and returning
  `Result` there keeps the invariant from being restated at every call site.
- **`verify_strict` over `Verifier::verify`.** `ed25519-dalek`'s strict verifier rejects
  small-order and non-canonically-encoded public keys and `s` values, so a signature that
  verifies for one peer cannot fail to verify for another. Signature malleability under a
  content-addressed protocol means two `op_id`s for one authored operation, which is
  indistinguishable from equivocation to everyone downstream.
- **`decode_and_admit` is the mandatory ingress** for peer bytes; see §ingress above.

## Owner-ratified errata, 2026-08-09

Three points where this crate diverged from `operation-envelope.md` prose have been resolved
in favor of the implementation and the oracle. The spec text now says the following, and code
must not drift back:

- **E1** — §6 rule 5: a detached-merge operation (kind 3) **MUST** use the no-payload form; it
  was "MAY", and the oracle already enforced MUST. A merge reconciles lineage and carries no
  new intent; intent belongs in a separate operation whose parent is the merge. This
  forecloses payload-carrying merges until a `protocol_version` bump. `kind::
  validate_structural_rules` enforces it alongside the two-parent minimum.
- **E2** — §2 rule 6: the NFC requirement applies to **every** text string in the profile, not
  only to application-defined text keys inside a schema payload. Both the oracle and this
  decoder already enforced the broader rule. Non-NFC user text MUST be carried as a CBOR byte
  string, never as a text string.
- **E3** — §2: CBOR major-type-1 arguments above `i64::MAX` (values below `i64::MIN`) are
  outside the v1 profile and MUST be rejected. The decoder already rejected them; the
  restriction is now normative rather than an implementation artifact.

## Owner-ratified errata, 2026-08-16 (D-CL28 gates)

Four gates on Wave 3, all landed before it. Unlike E1-E3 these are spec *additions*, not
implementation-vs-prose reconciliations, and each is here because it is cheap to add now and a
migration afterwards.

- **G1** — unknown operation kinds are their own quarantine reason with their own budget.
  `compose::QuarantineReason::UnknownKind` plus `QuarantineReasonClass` and
  `retention::PerReasonBudgets`; `retention::admit_candidate` checks the class budget before the
  pool, and `evict_expired_or_over_capacity` sheds an over-budget class from that class only.
  The defect this closes is availability, not correctness: under D-CL2 headers replicate
  verse-wide with no version gate, so an un-upgraded peer receives every operation of a kind it
  cannot interpret, and one reason-blind pool let that traffic evict the same peer's legitimate
  `MissingParent` backlog. Normative text: SPEC-5 §4 rules 1, 2, 4, 6, 7; SPEC-4 §5
  `unknown_kind`; SPEC-1 §6 rule 7 cross-reference (E4).
- **G2** — a reduction writes references, never reads artifacts.
  `ProjectionMutation::ReferencedArtifactUnavailable` makes the third outcome expressible; the
  enum previously offered only `Apply` and `Excluded`, so a materializer that could not resolve
  a reference had to encode an availability gap as a settled decision, and two peers would then
  disagree about a result one never computed. Normative text: SPEC-4 §4 rules 8-9, §5
  `referenced_artifact_unavailable`, §6 rule 7. **The rule that matters for Wave 3:** `meta` and
  `envelope_bytes` are the only admissible inputs to `CausalMaterializer::reduce`.
- **G4** — manifest statistics are tiered by derivation. See §Provisional wire numbering, keys 7
  and 8. Normative text: SPEC-6 §3.3 rules 1, 5, 6, 7.
- **G5** — spec-text only, no code in this crate: a mobile peer may affirmatively release
  headers behind a replay-verified checkpoint, at the cost of its bootstrap-advertising rights
  over that range. Normative text: SPEC-5 §5.2 rules 4-5. The retention module models
  quarantine and leases, not device storage profiles, so nothing here implements it; Wave 3's
  `fe-database` retention work is where it becomes callable.
