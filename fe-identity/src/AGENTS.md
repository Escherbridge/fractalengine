# fe-identity/src — keystore + token rationale

## §secret-store (`secret_store/`)

`SecretStore` is a pluggable trait for string secrets keyed by
`(service, account)`; implementations are `Send + Sync + 'static` so they can
be shared across threads as `Arc<dyn SecretStore>`. Three backends:

- `InMemoryBackend` — always compiled; tests use it to avoid touching the OS
  keychain.
- `OsKeystoreBackend` (`secret_store/os_keystore.rs`) — wraps
  `keyring::Entry`; compiled only under the `keyring` feature so headless and
  CI builds don't require a platform keystore.
- `EnvBackend` — maps `(service, account)` to env var
  `FE_SECRET_{SERVICE}_{ACCOUNT}` (components uppercased, non-alphanumerics
  replaced with `_`). Reads prefer the real environment, then fall back to a
  runtime-written in-memory override map; `set` writes only to that map and
  never mutates the process environment (env mutation is process-global and
  race-prone).

`keychain.rs` layers node-keypair persistence on top of the trait: the
32-byte Ed25519 seed is hex-encoded and stored under service
`"fractalengine"`, keyed by node id.

### §secret-store-service

`keychain::SERVICE` (`"fractalengine"`) is `pub(crate)` so `x25519.rs` and
`rotation_provisioning.rs` reuse the same literal instead of duplicating it.
All three modules share one `(service, account)` namespace in the
`SecretStore`; callers keep the Ed25519 identity-key account (e.g.
`"node_keypair"`) and any X25519 device-key account (e.g. `"device_keypair"`)
distinct so entries don't collide.

## §x25519-device-key (`x25519.rs`)

`DeviceKeypair` wraps an `x25519_dalek::StaticSecret` (dependency approved
under D-CL21, 2026-08-16; declared with the `static_secrets` feature — the
bare crate doesn't expose `StaticSecret`). It is generated from OS randomness
**independently** of the node's Ed25519 identity key, never birationally
converted from the Ed25519 seed. Reusing one seed across both algorithms
would couple device-key rotation to identity-key rotation, which
`author-key-lifecycle.md`'s SPEC-2 rotation lifecycle and
`capabilities-and-revocation.md`'s SPEC-3 §10 per-epoch scope-key wrap treat
as separate concerns — an identity-key rotation must not force every device
that principal owns to re-enroll, and vice versa.

Device keys get their own `did:key` encoding using multicodec prefix `0xec01`
(`did_key_from_x25519_public_key` / `x25519_public_key_from_did_key`),
distinct from `did_key.rs`'s Ed25519 `0xed01` prefix, so a device DID can
never be mistaken for an identity DID by a decoder that checks the prefix.

Persistence (`store_device_keypair` / `load_device_keypair` /
`load_or_generate_device_keypair` / `delete_device_keypair`) mirrors
`keychain.rs`'s Ed25519 functions exactly, on the same `SecretStore` trait
and `keychain::SERVICE`, under a caller-chosen distinct account string.

This module holds only key material. It does not depend on `fe-canonical-log`
and does not implement the SPEC-3 §10.2 HPKE-style wrap itself — that
construction (ephemeral X25519 key generation, BLAKE3-keyed wrap-key
derivation, XChaCha20-Poly1305 sealing) lives in
`fe-canonical-log/src/crypto/key_wrap.rs` and consumes the raw 32-byte keys
this module produces.

## §key-provisioning-tracking (`rotation_provisioning.rs`)

`author-key-lifecycle.md` §2.3 and conformance case 11 require that a key
regenerated after local secret loss is never presented as a continuation of
the lost key's lineage. `keychain::load_or_generate_keypair` alone can't
support that guarantee — its caller cannot tell, from the returned
`NodeKeypair`, whether it was loaded or freshly minted.
`load_or_generate_keypair_tracked` wraps it and returns
`KeyProvisioningOutcome::Existing` or `::NewPrincipal` so the caller must
handle the two cases explicitly instead of accidentally treating a fresh
principal as a lineage continuation.

`provision_successor_for_planned_rotation` and `retire_predecessor_local_use`
implement the two ends of §5.4's planned-recovery sequence: mint and store
the successor under a distinct account *before* any rotation operation is
built (leaving the predecessor's stored secret readable), then — only after
the rotation is admitted and successor capabilities are obtained —
explicitly retire local use of the predecessor. Keeping these as two
separate calls means a caller cannot lose predecessor access by accident
while a rotation is still in flight. Neither function builds, signs, or
validates the §3 rotation operation itself; that belongs to whichever
Workstream G component assembles `author-key-rotation-v1` payloads, which is
outside fe-identity.

## §api-token (`api_token.rs`)

`ApiClaims` is a deliberately separate JWT type from `FractalClaims`
(`jwt.rs`) for the Realtime API Gateway:

- TTL up to 30 days (`MAX_API_TOKEN_TTL_SECS`) vs. the 300-second session
  token.
- `scope` authorizes at any hierarchy level (`"VERSE#v1"`,
  `"VERSE#v1-FRACTAL#f1"`, ...), not just petal.
- `token_type: "api"` discriminates from session tokens so middleware can
  route without trial-decoding both claim shapes.
- `jti` gives each token a unique id for server-side revocation tracking.

Tokens are Ed25519-signed with the node keypair; minting rejects empty scopes
and TTLs over the maximum.
