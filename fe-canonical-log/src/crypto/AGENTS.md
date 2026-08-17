# src/crypto — the one cipher-executing module

SPEC-1 §3.5/§5.2/§9, SPEC-3 §10.1–§10.3, SPEC-6 §9.1. Every other module in this crate treats
ciphertext as opaque bytes; this is the only place a key, a nonce, or an AEAD call exists.

Scope boundary: protocol only. No socket, listener, connection, iroh, libp2p, `fe-network` or
`fe-sync` reference; no device enrolment, no wrap delivery transport, no relay seeding. §10.2.4
delivery and §10.3.4 network enablement remain owner-gated and are deliberately unbuilt here.

## §dependencies — approved under D-CL21

`chacha20poly1305 = "0.10"` and `x25519-dalek = { version = "2", features = ["static_secrets"] }`
are the workstream's only genuinely new third-party dependencies. They were **approved by the
owner under D-CL21 on 2026-08-16** and are already declared in `fe-canonical-log/Cargo.toml`.
This slice adds, removes, and re-versions nothing. `blake3`, `ed25519-dalek`, `rand`, and
`zeroize` were already crate dependencies; `rand` had no other user before this module.

## §no-hpke-crate — why X25519 + BLAKE3 KDF + AEAD, not the `hpke` crate

SPEC-3 §10.2 says "HPKE-style", not "HPKE". It then specifies its own KDF
(`BLAKE3_keyed(shared_secret, domain || 0x00 || canonical_key_wrap_aad)`), its own
deterministic-CBOR AAD map, its own AEAD suite selection, and its own Ed25519 artifact
signature. RFC 9180 HPKE would supply a different KDF (HKDF), a different key schedule
(`psk_id`/`info`/`exporter_secret`), and its own encoding — none of which the spec's wire format
has room for. Pulling in `hpke` would mean either ignoring most of it or contradicting the
ratified construction, and it would add a fourth crypto dependency to carry a key schedule this
protocol does not use. The three primitives the spec names (X25519, BLAKE3-keyed, XChaCha20-
Poly1305) are all already present or approved, so the construction is written directly.

The security property this loses relative to HPKE is the vetted key schedule; the property it
keeps is that the AAD map — which binds protocol version, scope, epoch, `key_id`, recipient
principal, recipient device key, and ephemeral public key — is an input to the KDF *and* the
AEAD, so altering any binding both fails authentication and derives a different wrap key.

## §aead — one seal door, one open door, one unconditional assertion

`XChaCha20Poly1305Suite::seal` / `::open` are the only two places in the crate that reach a
cipher, and `aead::seal` / `aead::open` are the only front doors to them. All four call
`EncryptionParams::assert_production_suite` on their first line, before any key material is
touched — not behind an `Option`, not at a call site, not behind a `#[cfg]`. A `cfg` gate would
delete the check from exactly the build it exists to protect (see `src/AGENTS.md` §"Fixture suite
65535 is a runtime assertion").

The check is duplicated on the trait impl deliberately. `AeadSuite` is a public trait, so a
caller can reach the cipher without going through `aead::seal`; if the assertion lived only in
the front door, that path would skip it. That is why `AeadSuite`'s methods take the whole
`EncryptionParams` rather than a bare nonce — an implementation cannot check a suite it was never
shown. `the_trait_surface_refuses_the_fixture_only_suite_too` pins the trait path specifically.
`segment::artifact::EncryptionDescriptor::assert_production_suite` is the sibling check on the
stored-artifact decode path; all of them must stay.

`associated_data` is an opaque `&[u8]` on purpose. An operation payload supplies
`payload_aad::payload_aad_preimage`; a stored artifact supplies
`artifact_aad::StoredArtifactAad::preimage`. Keeping the construction outside means neither
function can be specialized into a variant that quietly skips one of them.

**`FreshNonce` is a one-shot grant, not a value.** It is neither `Clone` nor `Copy`, and `seal`
consumes it by value, so one draw seals at most one plaintext. XChaCha20-Poly1305 nonce reuse
under one key is a keystream-recovery break, so this is enforced by the type system rather than
detected afterwards by a ledger. `segment::artifact::NonceLedger` remains the *second* line of
defence for the sealing path that goes through `seal_artifact`; it is bounded and evicts, so it
is a guard, never a proof. `FreshNonce::from_params` exists for the golden-vector oracle and the
negative tests; it cannot smuggle a non-production suite past `seal`, which still asserts.

`SealingKey` is the single 32-byte secret type — scope keys, derived wrap keys, and derived topic
keys are all one concept ("a key an AEAD suite seals under") rather than three near-identical
newtypes. It zeroizes on drop, redacts itself in `Debug`, and compares in constant time. No key
type in this module derives `Debug`, and no error variant carries key bytes.

## §artifact-aad — the pre-seal construction, and why it duplicates a shape

`segment::artifact::SealedArtifact::associated_data` answers the §9.1 question for an artifact
that already holds its ciphertext. Sealing needs the answer *first*: the ciphertext cannot exist
until the AAD authenticating it does, and `SealedArtifact::validate` rejects an empty ciphertext,
so the sealed type cannot be used as the pre-seal carrier. `StoredArtifactAad` closes that
ordering gap, and `for_plaintext` adds the Poly1305 tag length so the declared
`ciphertext_length` is the one the seal will actually produce.

That is a second construction of the same four-key metadata map, and the divergence risk is real.
`the_preseal_construction_matches_the_segment_slices_sealed_artifact_aad` pins the two to
byte-identical output across all four lanes; if the segment slice renumbers the outer map, that
test fails rather than two modules silently disagreeing about what was authenticated.

Domains are the four provisional prefixes reserved in `segment/AGENTS.md` and registered in
`src/AGENTS.md` §"Per-lane AEAD AAD domains", reused from `LaneClass::aad_domain` rather than
re-declared here:

| Lane | Domain | Lane | Domain |
| --- | --- | --- | --- |
| header segment | `fe-segment-header-v1` | HashSeq node | `fe-segment-hashseq-v1` |
| payload shard | `fe-segment-payload-shard-v1` | segment manifest | `fe-segment-manifest-v1` |

All four are NUL-terminated, so no domain is a prefix of another — a test pins the full
cross-product, and a second test proves a header-lane AAD cannot open a payload-shard ciphertext.

## §key-id — the identifier is derived, never stored alongside the key

`key_id = BLAKE3("fe-scope-key-id-v1" || 0x00 || canonical_scope_key_context || scope_key)`
(§10.1.3). `canonical_scope_key_context` is `capability::topic::scope_key_context`, the same
value the §6 blinded topic key derives from, so a scope or epoch change rotates the identifier
and the topic label together rather than as two steps an issuer could get out of sync.

The BLAKE3 preimage carries raw key bytes and is zeroized before `derive_key_id` returns.

## §key-wrap — provisional numbering and what "complete wrap" means

§10.2.3 fixes the seven-key associated-data map. Everything else about the artifact's encoding is
this slice's own assignment, provisional, and belongs in the `src/AGENTS.md` provisional
numbering surface the owner ratifies:

| Map | Keys |
| --- | --- |
| `KeyWrapAad` (§10.2.3, **normative**) | `0` protocol_version, `1` scope, `2` scope_epoch, `3` key_id, `4` recipient principal, `5` recipient X25519 public key, `6` ephemeral X25519 public key |
| `ScopeKeyWrapBody` (provisional) | `0` associated_data map, `1` wrap_nonce bytes(24), `2` sealed_key bytes(48), `3` issuer principal, `4` issuer capability reference |
| `ScopeKeyWrap` (provisional) | keys `0`..`4` of the body, plus `5` signature bytes(64) |

§10.2.3 says the signature covers `canonical_complete_wrap`. Read literally that would be
circular, so — following the SPEC-1 §5.1 unsigned/complete convention — the signed preimage is
the five-key body **without** the signature slot, and the artifact adds key 5. `ScopeKeyWrapBody`
is therefore what the spec calls the complete wrap.

`SCOPE_KEY_WRAP_DOMAIN` is deliberately the same string for two constructions: §10.2.2 uses it
for the BLAKE3-keyed wrap-key KDF and §10.2.3 uses it for the Ed25519 signature. That is what the
spec says. The two are not confusable — different primitives, different keys, and structurally
distinct bodies (a seven-key map versus a five-key map whose key 0 is that seven-key map) — but
the reuse is noted here so a future reader does not "fix" one of them.

`EphemeralSecret` is not used for the per-delivery ephemeral key even though the name fits:
x25519-dalek's `EphemeralSecret` cannot be constructed from bytes, so it cannot be pinned in a
test vector. `StaticSecret` is used instead; it zeroizes on drop under the crate's default
`zeroize` feature, and `wrap_scope_key` generates a new one on every call, which is what actually
makes the key per-delivery.

**Possession is never authority.** `open_scope_key` checks, in order: the issuer DID binding and
signature, the protocol version, the recipient principal binding, that this device's public key
is the one the AAD names, that the X25519 exchange was contributory, the AEAD tag, the recovered
plaintext length, and finally that the recovered key derives the `key_id` the AAD advertises.
That last check is the only defence against a malicious issuer delivering key material under
someone else's advertised identifier; a test builds exactly that artifact, because the check is
otherwise unreachable (any recipient-side tamper of the AAD breaks decryption first).

`issue_scope_key_wrap` is the entry point a delivery issuer uses: it runs
`segment::relay_policy::authorize_scope_key_wrap` against the persistent SPEC-3 view **before**
deriving anything, so a removed member or a superseded epoch is refused with no key material
touched. `wrap_scope_key` is the unauthorized primitive underneath it, kept public only for
callers that have already satisfied §10.2.1 through a different authorization surface and for
conformance vectors. If a future caller reaches for `wrap_scope_key` directly, that is the thing
to question — this codebase's recurring defect is the gate that is built but never wired.

## §scope-key-lifecycle — custody, rotation, and the limits of shredding

`ScopeKeyStore` (this module) is custody of key *material* keyed by `(scope, epoch)`;
`retention::crypto_shred::ScopeKeyStore` is the SPEC-5 destruction contract. They are two traits
with one name in two modules on purpose: they answer different questions and Wave 3's
`fe-database` store implements both, exactly as `InMemoryScopeKeyStore` does here. Reads hand out
a borrowed `SealingKey`, never a copy, so no implementation can produce key bytes that outlive
the store's zeroizing drop.

`ScopeKeyLifecycle::on_epoch_bump` generates the `e + 1` key **before** stopping `e` issuance:
stopping first would leave the scope with no issuable key at all if generation failed.
`get_or_generate` refuses an epoch whose issuance has stopped, so a stale caller cannot resurrect
a revoked epoch by asking again — that is also what makes a crypto-shred permanent for reissue,
since `crypto_shred` calls `stop_reissue` first.

`topic_key` derives the §6 blinded topic key from that epoch's scope key through
`capability::topic::derive_topic_key`, not a second construction. An epoch bump therefore rotates
the topic label as a consequence of rotating the scope key rather than as a step an issuer could
forget; `scope_epoch_bump_rotates_scope_and_topic_keys` pins both halves.

`LaneScopeKeyResolver` answers the segment slice's §4.2.4 `has_decrypt_authority` from real
custody. A `LaneKey::Header` carries no epoch — the lane key is just `{verse_id}` — so the
verse-wide header epoch is a constructor parameter; a payload lane carries its own epoch inside
its topic scope. Holding the header key grants no payload authority, which is SPEC-6 §2.2.3, and
a test pins it.

**Crypto-shredding is best-effort over locally controlled material only.** It destroys this
store's copy of the scope key, stops every future reissue path for that scope/epoch, and records
a disposition. It cannot touch immutable ciphertext, an already-delivered wrap on another device,
a relay's opaque retained artifact, or any copy a removed member made before removal. Nothing in
this module may be described as physical deletion (§10.3.3, SPEC-6 §10 "no erase promise from
storage"). `DestructionDisposition` structurally cannot carry key bytes, and neither can any
error variant here.
