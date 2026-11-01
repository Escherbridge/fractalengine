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

## §identity-tiers (`keychain::load_or_generate_keypair_durable`, `file_fallback.rs`, F6)

A third tier between the OS keystore and an ephemeral identity — see
`fractalengine/src/AGENTS.md` §identity-tiers for the in-app bug this fixes
(a transient keystore error used to mint an unpersisted DID, orphaning the
user's own verses). `load_or_generate_keypair_durable` tries the keystore
first (`Ok(None)` keeps the plain generate-then-persist path unchanged —
only an `Err`, read OR write, falls through); on fallthrough,
`file_fallback::load_or_generate_migrating(dir, legacy_dir, account)`
reads/creates `<dir>/<account>.key`, migrating in a legacy key found at
`<legacy_dir>/<account>.key` if `dir` doesn't have one yet (read-only at
the legacy path — write the new copy, delete the legacy one); only if
THAT also fails does it mint `KeypairSource::Ephemeral`. A successful
file-tier load attempts a best-effort heal-back write into the keystore
(never fatal if it fails). `dir` is resolved by the caller
(`fractalengine/src/main.rs`) to a per-user OS data directory — never a
path derived from a CWD-relative file like the DB path, which could
resolve somewhere unexpected depending on launch directory.

**Security: permissions are set AT creation, never after (security-review
fix).** `file_fallback::create_key_file_securely` opens with
`OpenOptions::create_new(true)` plus `.mode(0o600)` on unix in the SAME
syscall that creates the file — there is no window where the seed sits at
default (world-readable) permissions between a `write` and a later `chmod`.
`create_new` also makes creation atomic: a concurrent first-run race (two
launches, same account, same empty fallback dir) has the LOSER get
`AlreadyExists` and propagate it as `Err`, never silently reopen or
truncate the winner's file.

**Windows does NOT rely on the inherited ACL (second security-review
pass).** A freshly created file's inherited ACL can be broader than
owner-only (e.g. `BUILTIN\Users`), so immediately after `create_new`
creates the file, `windows_acl::restrict_to_owner` sets an explicit,
PROTECTED (non-inheriting) DACL granting only the Owner full access —
`ConvertStringSecurityDescriptorToSecurityDescriptorW` + `SetFileSecurityW`
with SDDL `"D:P(A;;FA;;;OW)"`, bound via a minimal direct `extern "system"`
declaration (NOT the `windows`/`windows-sys` crate — neither is a direct
dependency here, and the surface needed is three stable-ABI functions, so
adding either crate just for this was judged not worth it; see the
DPAPI-vs-DACL tradeoff below). If setting the DACL errors for ANY reason,
`create_key_file_securely` deletes the file it just wrote and returns
`Err` — the tier fails CLOSED, never leaving a plaintext seed at a
possibly-inherited ACL.

**Why a DACL and not DPAPI.** `CryptProtectData`/`CryptUnprotectData` (via
the `windows` crate) was the first-choice design for at-rest encryption,
but (a) `windows` is only a deep transitive dependency in this workspace
(via `keyring`/Tauri, with no DPAPI bindings exposed — adding it directly
just for two functions means picking a version/feature set and reviewing
unsafe FFI with blob-lifetime and `LocalFree` bookkeeping), and (b) DPAPI's
failure mode is DECRYPT-time, not just create-time — a profile/SID-binding
edge case (roaming profiles, machine-vs-user scope mismatches) could make
a previously-written key silently fail to decrypt on a later launch, which
is exactly the "identity silently stops loading" bug class this whole F6
fix exists to eliminate. An explicit, fail-closed DACL has no such
decrypt-time fragility: a key that was ever created successfully stays
readable by its owner forever, with no cryptographic machinery to desync.

Every error from this path is real (never `.ok()`'d away): a failed
secure create returns `Err` from `load_or_generate`/
`load_or_generate_migrating`, which `load_or_generate_keypair_durable`
treats exactly like a file-tier read failure — fall through to the loud
ephemeral tier, never a silently open-permissioned key file.

**Owner-only files, generalized (2026-11-01).** The pattern above is now
`file_fallback::create_owner_only_file(path, contents)` (public;
`create_key_file_securely` is a one-line call into it) so other secrets
reuse it instead of re-deriving it — first consumer: the GUI's agent-QA token
file (fe-api/AGENTS.md §qa-dev-token). One tightening rode along: on Windows
the file is now created EMPTY, restricted to the owner-only DACL, and only
THEN written, so the content never exists under the inherited ACL even for
the instant between write and `SetFileSecurityW`. A failed content write also
deletes the file (unix and Windows). `replace_owner_only_file(path, contents)`
adds an atomic replace for files that are rewritten: a hidden sibling temp
(`.<name>.<pid>.<nanos>.tmp`) goes through `create_owner_only_file`, then
`std::fs::rename` moves it over `path` (same directory, so the protected DACL
or `0600` mode travels with it); the temp is removed on any failure.

### QA harness security correction (2026-11-01)

The current Windows `create_owner_only_file` supplies the protected owner-only
DACL directly to `CreateFileW(CREATE_NEW)` and writes through that same handle,
with no sharing. This supersedes the historical create-empty/set-by-path/reopen
sequence above. ABI declarations follow the vendored `windows-sys` definitions.
No secret bytes are written through a reopened pathname. Existing secret-file
journeys cover replacement, refused overwrite and junction-parent rejection.

Secret writes reject symlink/reparse ancestry. On Windows each ancestor is
opened with `OPEN_REPARSE_POINT | BACKUP_SEMANTICS`, checked through its handle,
and held without write/delete sharing until creation/replacement completes. Thus
ancestors cannot be renamed into junctions between validation and writing.
Unix rejects existing symlink ancestry but does not claim handle-relative
protection against a malicious concurrent directory writer. Owner-only file
creation remains `create_new + 0600` there.

`qa_storage_root()` resolves the explicit absolute `FE_QA_STORAGE_ROOT` and
rejects redirected ancestors. GUI startup uses it for run-local identity,
settings and storage; absent means normal desktop behavior. Invalid roots
fail startup, and settings resolution independently avoids user persistence.

The QA `RunLocalBackend` implements the same `SecretStore` trait for every
service/account value, including share signers, namespace secrets and canonical
scope keys. A fixed `identity/secrets.json` path stores a map whose keys encode
service/account bytes as delimiter-separated hex, so arbitrary account strings
never become filesystem paths. Updates use the owner-only atomic replacement
helper and a per-backend lock; all consumers in one GUI process share that
backend. Reopening the same run retains its secrets and deletion state without
contacting the OS keystore. This is a single-writer run store, not a multiprocess
database. The existing trait-object journey covers reopen, overwrite, delete,
and traversal-shaped account names without global environment mutation.

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

`examples/mint_api_token.rs` (M1 user-testing validation fixture, 2026-10-07)
is the headless equivalent of the UI's `MintApiToken` flow for validators
that control the process environment: it mints a real token from a hex seed
via the same `mint_api_token` call — the relay verifies Bearer JWTs against
the keypair loaded from `FE_SECRET_FRACTALENGINE_NODE_KEYPAIR`, so a token
minted from the same seed passes the production `auth_middleware` unchanged.

### client claim (TOKEN-CLIENT-CLAIM, 2026-10-25)

`ApiClaims.client: Option<String>` names the API *client* a token was minted
for — an ed25519 `did:key`. It is `#[serde(default, skip_serializing_if =
"Option::is_none")]`, so a legacy token (no client) decodes unchanged and
re-encodes byte-identically.

- **Ownership key only, never an authorization subject.** `sub` stays the
  issuing node DID; roles, the AC-46 cap and every `AuthContext`/`CallerAuth`
  key on `sub`. fe-api reads `client` in exactly one place (`owner_key`, the
  upload owner / quota / unplaced-reach key — fe-api `AGENTS.md` §assets).
  Naming a Manager's DID as `client` therefore grants nothing (threat TC-1).
- **Grammar** (`validate_client_did`): non-empty, ≤ `MAX_CLIENT_DID_LEN`
  (128) bytes, decodes via `public_key_from_did_key` (ed25519 multicodec only —
  an x25519 device DID is refused) AND re-encodes to the identical string, so
  one key cannot form two owner buckets through alternate spellings. Checked at
  mint (`mint_client_api_token`) and again at verify (`verify_api_token`), so a
  correctly-signed token carrying a bad client is still refused.
- **Mint doors.** `mint_api_token` / `mint_api_token_at` keep minting legacy
  tokens (the many test/CLI call sites stay untouched). The production door —
  `DbCommand::MintApiToken` (fe-database `handlers/api_token.rs`) and the
  harness DB loop — applies one rule, `resolve_client_did`: a requested client
  is validated, a blank one gets `new_client_did()` (a fresh key whose secret
  half is dropped), then `mint_client_api_token` signs it. The
  CLI example stays legacy unless `--client <did:key>` or `--new-client` is
  passed (A3) and echoes the client to stderr.
