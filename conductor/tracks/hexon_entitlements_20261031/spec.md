---
type: spec
title: Registry protocol v1 + PackageStore trait + EntitlementVerifier (open licensing seam)
track: hexon_entitlements_20261031
tags: [feature, open-core, licensing, registry, fe-format, fe-hexon, fe-hexon-registry, fe-ui, hexon_entitlements_20261031]
created: 2026-10-31
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: user grill 2026-10-31 (open-core alignment)
depends_on: []
---

# Specification: Registry protocol v1 + PackageStore trait + EntitlementVerifier

**Track ID:** `hexon_entitlements_20261031` · **Type:** feature · **Priority:** P1 (open-core enabling track)
**Crates:** `fe-format` (entitlement primitives, format-level), `fe-hexon-registry` (protocol + storage abstraction), `fe-hexon` (client-side verification + decryption), `fe-ui` (Hexon Manager states), `docs/` (format spec)
**Citations computed at:** `f55ca21`. Symbol names are stable; line numbers are not — re-grep before editing.

## 1. User vision (2026-10-31, open-core alignment)

Per `conductor/open-core.md`'s rule — *"anything a self-hoster needs to run, verify, or extend
FractalEngine is open and lives here"* — this track delivers everything a self-hoster needs to
run their own registry and to distribute or verify licensed content without anyone's hosted
service. Any external service is a **consumer** of the two seams this track owns (S-REGISTRY,
S-ENTITLEMENT), never the other way around.

## 2. Problem & evidence

### 2.1 The license fields exist but do nothing (ground truth @ `f55ca21`)

`fe-format/src/license.rs:6-27` defines `LicenseType::{Free, Attribution, Paid}` and `License`
with `spdx_id`, `attribution`, `payment_provider`, `payment_verification_url`, `free_entries`,
and `encrypted_key`. `License` is embedded in a sealed archive's optional `license.json`
(`fe-format/src/archive.rs:33`, `:125`, `:490-492`) and round-trips through `HexonArchive`.
Nothing reads `encrypted_key` to decrypt anything, nothing checks `free_entries` to gate a load,
and nothing ever constructs a `Paid` license with a real key: every `License` literal across the
whole workspace sets `encrypted_key: None` —

```
fe-format/src/license.rs:39:            encrypted_key: None,   (License::default)
fe-hexon/src/authz.rs:232:            encrypted_key: None,
fe-hexon/src/manifest.rs:74:            encrypted_key: None,
fe-hexon/src/package.rs:343,426,535,644,702: encrypted_key: None,
fe-hexon/src/publisher.rs:341:              encrypted_key: None,
fe-hexon/src/registry.rs:525:               encrypted_key: None,
```

(ten sites, re-grep `encrypted_key: None` to confirm count has not changed). A `Paid` hexon
today is indistinguishable at runtime from a `Free` one — the type exists, the behavior does not.

**Note a naming collision worth not re-creating.** `fe-hexon/src/manifest.rs:288` defines its
*own* `License`/`LicenseType` (same shape, no `spdx_id`), used only by the legacy
`HexonPackage`/`HexonRegistry` path (`fe-hexon/src/registry.rs`, surfaced at
`fe-api/src/hexon.rs`'s `/api/v1/crates/*` routes — a different, in-process "local desktop
install" model, NOT the hosted HTTP registry). This spec's `EntitlementVerifier` targets
**`fe_format::license::License`** exclusively, because that is the type embedded in the sealed,
signed archives that `fe-hexon-registry` actually serves and that `docs/hexon-format-spec.md`
documents. `fe-hexon::manifest::License` is out of scope; do not wire entitlements through it.

### 2.2 The hosted registry has no protocol version and is open by default

`fe-hexon-registry/src/main.rs:16-18`:

```rust
let cfg = RegistryConfig::from_env();
if cfg.token.is_none() {
    tracing::warn!("FE_REGISTRY_TOKEN not set — registry is open access");
}
```

Unset `FE_REGISTRY_TOKEN` and the registry runs — a single shared bearer token is the *entire*
auth model (`routes.rs::require_token`, gates `/api/v1/*`; `/health` is always public). There is
no accounts, no RBAC, no protocol version anyone can negotiate against, and nothing stops an
operator from exposing it unauthenticated by omission rather than by choice. The format spec's
own `## API Endpoints` table (`docs/hexon-format-spec.md:560+`) claims a `Viewer+`/`Owner+` RBAC
column that **does not exist** in `fe-hexon-registry` (that RBAC belongs to the different
in-app `/api/v1/crates/*` routes in `fe-api`) — the doc and the code have drifted apart, which
this track also fixes (FR-8, §4.8).

### 2.3 Storage is hard-coded to one filesystem directory

`RegistryState` (`routes.rs:52-59`) is `{ dir: PathBuf, token, readonly, index: RwLock<Vec<HexonIndexEntry>>,
publish_lock: Mutex<()> }`, and every handler reads/writes `state.dir` directly
(`index::scan_dir`, `index::read_zip_bytes`, `routes::write_new`). There is no seam a
non-filesystem store (object storage, a database) could implement against without
rewriting every handler. `fe_format::HexonArchiveIndex::open` additionally requires a real
filesystem path for random-access zip reads (`fe-format/src/archive_index.rs:135` `open`, `:144`
`open_with_policy`; called from `fe-hexon-registry/src/routes.rs:225` and `:282`), which any
abstraction has to account for.

### 2.4 Industry anchor

| Practice | npm / crates.io / Docker registries | This track |
|---|---|---|
| Versioned, documented wire protocol | `/v2/` Docker Registry API, npm registry API spec | `registry protocol v1`, versioned explicitly (FR-1) |
| Pluggable storage backend | npm registries (local tarball store, S3, GCS all implement the same registry API) | `PackageStore` trait (FR-2) |
| Secure-by-default auth | Docker Registry defaults to requiring auth for push; GitHub Packages requires a token always | Default-deny (FR-3) |
| License-gated content, verified client-side | DRM-free licence keys (game key stores); age/SSH recipient-encrypted secrets | `EntitlementVerifier` + recipient-wrapped content key (FR-4/FR-5) |

## 3. Scope

**NFR-1 (hard constraint, from the grill):** no payment code, payment-provider integration,
pricing, revenue figures, or marketplace business logic in any file this track touches. This
track is verification and self-hosted issuance only (`conductor/open-core.md` §The rule).

**In scope (this repo, public, verification/self-issuance only):**
- Registry protocol v1: a version marker + a documented contract for `/api/v1/hexons/*`.
- `PackageStore` trait + a `FsPackageStore` reference implementation that is a **behavior-
  preserving refactor** of today's filesystem logic (same routes, same status codes).
- Default-deny auth with an explicit opt-out for local dev (OD-1).
- `EntitlementVerifier` trait, an ed25519 entitlement token format, pinned issuer keys, and a
  reference `LocalIssuer` any self-hoster can run to mint their own tokens (OD-8) — "any operator
  can run an issuer" per `conductor/open-core.md` §S-ENTITLEMENT.
- The `encrypted_key` wrap scheme (recipient-wrap, FR-5) and the on-disk encrypted-entry format
  for a licensed archive's non-free blobs.
- Client-side wiring in `fe-hexon` (decrypt in memory at load time; ciphertext stays at rest,
  §4.6) and `fe-ui` (Hexon Manager states).
- `docs/hexon-format-spec.md` version bump to 1.1.0.

**Out of scope (outside this repository, per `conductor/open-core.md` §The rule):**
- Operating an issuance service for other people: account systems, key custody on someone
  else's behalf, or any business process that decides *when* a token is minted or withheld.
  This track ships the token format, verification, and a self-hosted `LocalIssuer` only.
- A non-filesystem `PackageStore` implementation (e.g. object storage). This track defines the
  trait and the filesystem reference impl only.
- Listings, ranking, or any catalog curation beyond the existing `search` substring/tag/type
  filter.

**Out of scope (deferred within this repo):**
- Server-side content gating (the registry refusing to serve a licensed archive's bytes to an
  unauthenticated caller). NFR-3 keeps the registry a dumb byte host; access control is
  cryptographic, not server-enforced — anyone can download a licensed `.hexon`, nobody can use
  its non-free entries without a verified token. This mirrors DRM-free licence-key models and
  keeps the registry simple enough to stay in `PackageStore`'s trait boundary.
- A per-token revocation list. **One revocation model across S-ENTITLEMENT and S-PLUGIN:**
  licence-level revocation is short token expiry (`exp`) plus an issuer refusing to reissue; the
  `S-PLUGIN` signed revocation list (`plugin_host_revival_20261031`) is a *package-level* kill
  switch keyed by `(package_id, content_hash)` for withdrawn or malicious packages and never
  revokes a licence. `jti` is reserved so a token-level list could be added later without a
  format break, but none is planned.

## 4. Design

### 4.1 Seam names (verbatim — both repos use these)

- **S-REGISTRY**: registry protocol v1, versioned; a `PackageStore` storage trait; default-deny
  authentication.
- **S-ENTITLEMENT**: an ed25519 entitlement token (package id + content hash, licensee DID,
  expiry, issuer key id) verified against pinned issuer keys; unwraps `encrypted_key`; decrypts
  non-`free_entries`. Gives effect to `LicenseType::Paid` / `encrypted_key` / `free_entries`
  already defined in `fe-format/src/license.rs`. Any operator can run an issuer.

### 4.2 FR-1 Registry protocol v1

Add a protocol version to every response so a caller (including a future non-filesystem
implementation) can detect a mismatch instead of guessing from undocumented behavior. **OD-2**
decides the transport: a response header (`X-Hexon-Registry-Protocol: 1` on every `/api/v1/*`
response) is additive and needs no route changes; a route prefix (`/api/v1/v1/hexons/...`) is
uglier and would break every existing client (`fe-hexon/src/remote.rs`'s hard-coded
`/api/v1/hexons/...` URLs). **Recommended: header only**, plus a `protocol_version` field added
to `/health`'s existing JSON body (`routes.rs::health`, already `{"status": "ok", "indexed": N}`
→ `{"status": "ok", "indexed": N, "protocol_version": 1}`). `docs/hexon-format-spec.md` names
this "registry protocol v1" as a first-class, versioned document (§4.6).

### 4.3 FR-2 `PackageStore` trait

```rust
#[async_trait]
pub trait PackageStore: Send + Sync {
    async fn list(&self) -> anyhow::Result<Vec<HexonIndexEntry>>;
    async fn put_new(&self, id: &str, version: &str, bytes: Bytes) -> Result<(), StoreError>; // AlreadyExists -> 409
    async fn open_path(&self, id: &str, version: &str) -> Result<PathBuf, StoreError>; // see OD-3
}
```

`FsPackageStore` wraps today's `dir`/`index`/`publish_lock` fields unchanged — `list` returns the
cached index, `put_new` is `write_new`'s `create_new` semantics, `open_path` is the resolved
entry's on-disk path. **OD-3 (the real design question):** `fe_format::HexonArchiveIndex::open`
needs random-access file I/O (`&Path`), so `open_path` returning a local path is correct for a
filesystem store but forces a non-path store (e.g. object storage) to **download
to a local temp file before every `/meta`, `/chunk`, `/member` read** — there is no way around
this without changing `fe_format::HexonArchiveIndex` itself to accept `Read + Seek`. **Recommend:
accept the temp-file cost in non-filesystem implementations now** (reversal cost: low — it is
purely an implementation detail behind the trait) rather than touching `fe_format`'s reader in
this track.

### 4.4 FR-3 Default-deny auth (OD-1)

`main.rs` must refuse to start serving with no token configured unless an explicit escape hatch
is set. Recommended name: `FE_REGISTRY_ALLOW_OPEN=1`. Replace lines 16-18:

```rust
if cfg.token.is_none() {
    if std::env::var("FE_REGISTRY_ALLOW_OPEN").ok().as_deref() != Some("1") {
        anyhow::bail!("FE_REGISTRY_TOKEN not set — refusing to start open. \
                        Set FE_REGISTRY_ALLOW_OPEN=1 for local dev/test only.");
    }
    tracing::warn!("FE_REGISTRY_ALLOW_OPEN=1 — registry is running with no auth");
}
```

`RegistryConfig::from_env` (`config.rs`) gains the same check so tests can assert it directly
without spawning a process. This is additive-safe: anyone who already sets `FE_REGISTRY_TOKEN`
is unaffected; only the "forgot to set it" path changes from a log line to a refusal.

### 4.5 FR-4/FR-5 `EntitlementVerifier` + token + wrap scheme

**Token format (OD-5).** Mirror the existing `ApiClaims` precedent
(`fe-identity/src/api_token.rs:16-35`) rather than inventing a binary format: a JWT signed EdDSA
(the `jsonwebtoken` crate is already a workspace dependency via `fe-identity`), claims —

```rust
pub struct EntitlementClaims {
    pub sub: String,              // licensee DID (did:key, ed25519)
    pub package_id: String,       // hexon_id
    pub content_hash: String,     // pins the exact sealed archive's integrity root
    pub iat: u64,
    pub exp: u64,
    pub jti: String,              // revocation id, unused by v1, reserved
    pub kid: String,              // which pinned issuer key signed this
    pub wrapped_content_key: String, // base64, see below
}
```

**Placement.** `EntitlementClaims`, `IssuerKeyring`, the `EntitlementVerifier` trait with its
reference impl, `LocalIssuer`, and the wrap/seal primitives all live in `fe_format::entitlement`,
compiled only under a new opt-in `entitlement` cargo feature. `fe-hexon-registry` depends on
`fe-format` with `default-features = false` (`fe-hexon-registry/Cargo.toml:24`), so an
unconditional module would hand the registry jsonwebtoken/x25519/AEAD dependencies and break
NFR-3. `fe-hexon` (and later `fe-plugin`, for licensed plugins) enable the feature; neither has to
depend on the other to verify a token.

`IssuerKeyring` (config: a list of trusted `(kid, ed25519 public key)` pairs, **OD-6**) verifies
the signature before trusting any claim — exactly the "pin, don't trust-on-first-use" pattern
`fe_format::IntegrityPolicy::RequireSealed { pin }` already uses for publisher keys.

**Wrap scheme (OD-4, recommended: recipient-wrap).** `encrypted_key` on the archive's
`license.json` is the content key wrapped **to the issuer's own X25519 public key** (so only a
holder of the issuer's private key can ever redistribute it — this is what makes "any operator
can run an issuer" safe: the issuer, not the registry, custodies the unwrap capability). At
*mint* time the issuer unwraps it with their own secret and re-wraps it to the **licensee's**
X25519 public key, placing that per-licensee ciphertext in the token's `wrapped_content_key`
claim. The mint request therefore carries the licensee's X25519 device public key (the
`did:key` of its `fe_identity::x25519::DeviceKeypair`), while `sub` stays the licensee's ed25519
identity DID. `EntitlementVerifier::verify` then:

1. Decode the JWT, verify EdDSA signature against the pinned key named by `kid`.
2. Check `exp`, `package_id`, and `content_hash` match the archive being opened.
3. X25519-unwrap `wrapped_content_key` using the **licensee's own** device secret.
4. Return the 32-byte content key, which decrypts every entry not in `free_entries`.

Step 3 reuses `fe_identity::x25519::DeviceKeypair` (`fe-identity/src/x25519.rs:17-48`) verbatim —
it is already "an independent X25519 key, distinct from the node's Ed25519 identity, with its
own `did:key`" for exactly this kind of recipient-wrap use (its own doc comment cites "the
SPEC-3 §10 recipient-device scope-key wrap"). The AEAD itself (XChaCha20-Poly1305 sealing under
a 32-byte key) has a direct precedent to model the API after: `fe-canonical-log/src/crypto/
aead.rs::SealingKey` (zeroized on drop, constant-time compare, one disclosure point). **Do not
add a dependency on `fe-canonical-log`** (it is an unrelated audit-log concern) — mirror its API
shape with the `chacha20poly1305`/`x25519-dalek` crates directly in `fe-format::entitlement`,
which already depends on `ed25519-dalek`.

**Revocation (one model, see §3).** A licence ends when its token's `exp` passes and the issuer
declines to reissue; `EntitlementVerifier` never phones home. Because content is decrypted in
memory at load time (§4.6) and never persisted as plaintext, expiry actually bounds an honest
client's access. A determined licensee can always dump plaintext — the DRM-free trade-off this
spec accepts. Pulling a whole package from circulation is not an entitlement concern: for plugins
it is the S-PLUGIN revocation list; hexons (data, not code) have no kill switch in v1.

**Reversal cost of OD-4:** medium. A static symmetric key delivered inside the token (no
recipient-wrap) is simpler to implement but means the token itself, not just its signature, is
the secret — leaking one token leaks content access forever (no per-device revocation surface).
Recipient-wrap keeps the token's secret half bound to a device key that never leaves the device.

### 4.6 FR-6 Client-side enforcement (fe-hexon)

New `fe_hexon::entitlement_gate` module, consumed by `remote.rs`'s download/install path and by
the in-process `HexonPackage::open` path alike. Encrypted entries stay encrypted at rest; the gate
decrypts in memory each time an entry is loaded:

- No token present or token fails verification → only entries listed in `license.free_entries`
  load; everything else is `EntitlementRequired` (not an error — a distinct, inspectable state).
- Token verifies → content key recovered (§4.5) → every non-free entry's blob is opened through
  `open_paid_entry(content_key, ciphertext)` before use.
- A `Free`/`Attribution` license, or no `license.json` at all (pre-this-track archives), behaves
  exactly as today — `EntitlementVerifier` is never invoked (NFR-2).

### 4.7 FR-7 Hexon Manager UI states (fe-ui)

`fe-ui/src/dialogs/hexon_manager.rs`'s Installed/Available tabs gain a status per row:
`installed-preview` (free entries only, no token), `entitlement-required` (Paid, no valid
token), `entitled` (valid token, full content). `payment_verification_url`, when present, renders
as a plain outward link with no payment widget, no price, no "buy" button — **OD-7**: showing
the link at all is a UX choice (it is the archive's own declared pointer, not something this
track invents), but the UI must never fetch or render anything from that URL inline (SSRF/XSS
surface); it is `ui.hyperlink`, nothing more.

### 4.8 FR-8 Format spec 1.1.0

`docs/hexon-format-spec.md` moves 1.0.0 → 1.1.0 (OD-9): a "Registry protocol v1" section (the
`X-Hexon-Registry-Protocol` header, `/health`'s `protocol_version`, default-deny auth), the
entitlement token claims and encrypted-entry wire format, the revocation model in one paragraph,
a backward-compat note (`license: None` and `Free`/`Attribution` unaffected), and a fix for the
`## API Endpoints` table's RBAC column, which describes the in-app `/api/v1/crates/*` routes
rather than `fe-hexon-registry` (§2.2).

## 5. Acceptance criteria (observable, via real flows)

- AC-1: a `.hexon` with `license_type: Free` (or no `license.json`) installs and all entries load
  exactly as before this track — zero behavior change (NFR-2 regression guard).
- AC-2: a `Paid` archive with `free_entries: ["preview_1"]` and no entitlement token loads
  `preview_1` and refuses (not crashes) to load any other entry; Hexon Manager shows
  `installed-preview`.
- AC-3: minting a token with `LocalIssuer` for a licensee's own device key, then verifying it
  with `EntitlementVerifier`, recovers the exact content key the archive was sealed with, and
  every non-free entry decrypts to its original plaintext bytes (round-trip, not just "no error").
- AC-4: a tampered token (flipped signature byte, wrong `content_hash`, expired `exp`) is
  rejected — `EntitlementRequired`, never a silent fallback to plaintext.
- AC-5: starting `fe-hexon-registry` with no `FE_REGISTRY_TOKEN` and no
  `FE_REGISTRY_ALLOW_OPEN=1` refuses to start (exit non-zero, logged reason); setting either
  lets it start exactly as today.
- AC-6: `FsPackageStore` behind the new trait answers every existing route identically to the
  pre-refactor handlers (same JSON shape, same status codes) for the existing integration tests.
- AC-7: `/health` and every `/api/v1/*` response advertise `protocol_version: 1` / the version
  header.

## 6. Open decisions (user, ratify before build)

| ID | Decision | Options | Gates |
|---|---|---|---|
| **OD-1** | Default-deny escape hatch | **(rec)** `FE_REGISTRY_ALLOW_OPEN=1` hard-required to start with no token · keep warn-and-continue (status quo, rejected by the grill) | FR-3 |
| **OD-2** | Protocol version transport | **(rec)** response header + `/health` field · new route prefix (breaks existing clients) | FR-1 |
| **OD-3** | `PackageStore` for non-path stores | **(rec)** `open_path` returns a local path; non-filesystem impls download-to-tempfile · change `fe_format::HexonArchiveIndex` to accept `Read+Seek` (bigger, cross-cutting change) | FR-2 |
| **OD-4** | `encrypted_key` wrap scheme | **(rec)** X25519 + XChaCha20-Poly1305 recipient-wrap to the issuer's key, re-wrapped per licensee in the token · a symmetric key delivered inside the token (simpler, no per-device revocation) | FR-5 |
| **OD-5** | Entitlement token format | **(rec)** EdDSA JWT mirroring `ApiClaims` (reuses `jsonwebtoken`, already a dependency) · a bespoke binary/CBOR format | FR-4 |
| **OD-6** | Issuer key pinning | **(rec)** a static config list of `(kid, pubkey)` pairs, mirrors `IntegrityPolicy::RequireSealed { pin }` · a DID-document-style discovery mechanism (more moving parts, no clear win yet) | FR-4 |
| **OD-7** | `payment_verification_url` UI exposure | **(rec)** render as a plain outward hyperlink, never fetched/rendered inline · hide the field entirely | FR-7 |
| **OD-8** | Who may mint tokens locally | **(rec)** ship `LocalIssuer` (any self-hoster generates an issuer keypair and mints their own tokens) as part of the open crate — this is what makes "any operator can run an issuer" true · ship verification only, defer issuance tooling | FR-4 |
| **OD-9** | Format-spec bump scope | **(rec)** 1.0.0 → 1.1.0 (additive: new optional sections, no existing field removed/renamed) · defer the version bump until a downstream consumer ships (rejected — the open seam must be documented before anyone depends on it) | FR-8 |

## 7. Risks

- **Crypto surface.** New ed25519 token verification + X25519 unwrap + AEAD decrypt is exactly
  the kind of code the standing rule flags for mandatory `/security-review` (auth/crypto/user
  input) — budget for it explicitly in the plan, not as an afterthought.
- **Two `License` types.** §2.1's naming collision (`fe_format::license::License` vs
  `fe_hexon::manifest::License`) is easy to wire to the wrong one; S3's tests must exercise the
  format-level type by its full path, not a bare `License` import, to keep the two from silently
  aliasing in a future refactor.
- **Registry statelessness assumption.** NFR-3 (registry never enforces access, only serves
  bytes) is a deliberate trust model, not an oversight — if a future requirement needs
  server-side content gating, that is a different, bigger change (the registry would need to
  parse `license.json` and check tokens itself), not a small addition to this track.
- **`PackageStore`'s path leak (OD-3).** Returning a local path from an async trait method
  assumes every implementation can produce one; this is true for the filesystem reference impl
  and workable (via tempfile) for an object-storage impl, but is a real constraint a reviewer
  should re-confirm before any downstream store builds against it.

## 8. Dependencies

- **depends_on:** none (this track is foundational to the licensing seam).
- **blocks:** `plugin_host_revival_20261031` Phase 4 only (install-from-registry reuses registry
  protocol v1 and `fe_format::entitlement::EntitlementVerifier`).
- **related:** `relay_operability_20261031`, `plugin_host_revival_20261031` (its signed
  revocation list is the package-level kill switch; licence revocation stays TTL-based, §3),
  `agent_input_surface_20261031`, `release_ci_20260429`.
- **Downstream consumers (one-way):** services built on the `PackageStore` trait and the
  token format need both stable; per `conductor/open-core.md` this repo never names them.
