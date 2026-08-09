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

`EncryptionParams::assert_production_suite` has exactly one caller, and it is
`decode_and_admit`. Keep it that way: a suite check reachable only by opt-in reads as enforced
without being enforced, which is how this repository has previously shipped dormant gates.

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
| `author_key` | SPEC-2 | later slice, unassigned at W1a |
| `capability` | SPEC-3 | later slice, unassigned at W1a |
| `crypto` | SPEC-1 §9, SPEC-3 §10 | later slice, unassigned at W1a |
| `materialize` | SPEC-4 | later slice, unassigned at W1a |
| `branch` | SPEC-5 | later slice, unassigned at W1a |
| `retention` | SPEC-5 | later slice, unassigned at W1a |
| `checkpoint` | SPEC-5 | later slice, unassigned at W1a |
| `segment` | SPEC-6 | later slice, unassigned at W1a |
| `wire` | SPEC-6 | later slice, unassigned at W1a |
| `compose` | cross-cutting facade | later slice, unassigned at W1a |

Unimplemented modules are one-line doc-comment placeholders. They exist so `lib.rs` compiles
today and so module boundaries are settled before parallel slices start writing code.

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
