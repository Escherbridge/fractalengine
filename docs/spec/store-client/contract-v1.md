# Store-client contract v1

**Contract id:** `fe-store-client/1` (advertised as `registry_protocol: 1`). **Status:** normative, 2026-11-13. Track `extension_platform_20261107`, seams S-REGISTRY, S-PLUGIN and S-ENTITLEMENT (`conductor/open-core.md`).
**Normative words:** MUST, MUST NOT, SHOULD and MAY are normative. Numbered rules are the contract; prose between them is rationale.
**Never changed in place.** A clarification that changes no behaviour may land here; anything a conforming v1 client or endpoint would notice ships as `fe-store-client/2` beside this file.

This is how a FractalEngine client finds, verifies and installs sealed `.hexon` packages (extensions and web bundles) from a catalogue endpoint. A directory of plain files on any static host is a complete endpoint; a hosted registry serves the same documents and adds optional operations. The fixture tree in [`fixtures/`](fixtures/README.md) is a complete static endpoint, and [`fixtures/vectors.json`](fixtures/vectors.json) lists the test vectors every client and endpoint implementation runs.

## 1. Transport

1.1. **Base.** An endpoint is a base URL `{base}`. Every catalogue path sits under `{base}/api/v1/catalog/`, beside the existing `/api/v1/hexons/*` content routes, which this contract does not change.

1.2. **Static first.** Every GET path except `search` is a plain file path, so a directory on any static host is a complete endpoint. A hosted registry MUST serve the same paths and byte-identical documents, and MAY add features (§3).

1.3. **No credentials.** Catalogue GETs and downloads need none. A client MUST NOT send credentials (no `Authorization` header, no cookies, no client certificate) to `{base}` or to any URL a catalogue document names, and MUST NOT follow a redirect that would add them. Only the token request (§3, `entitlement check`) authenticates, and it does so through its own `proof`, never a header.

1.4. **Features, never probes.** `config.json` `features` advertise optional operations. A client MUST NOT probe for an operation that is not advertised. A missing feature means "do it locally" (`search`) or "not offered" (`entitlements`, `revocations` at this endpoint).

1.5. **Bodies and errors.** Success bodies are bare JSON documents, identical whether static or hosted. Hosted errors use the registry protocol v1 envelope `{ "ok": false, "data": null, "error": "<code>" }` (`fe-hexon-registry/src/routes.rs`) with a stable snake_case code. Hosted responses carry `X-Hexon-Registry-Protocol: 1`; a client takes the protocol version from `config.json` `registry_protocol` and MUST NOT require the header, which static hosts cannot set.

1.6. **Encoding.** JSON in UTF-8. Signed documents carry **integer numbers only**: a float anywhere in a signed document makes it malformed. Times are RFC 3339 UTC, except inside the revocation list, which keeps its own wire format (§4.7). Path segments are percent-encoded (`fe-hexon/src/remote.rs`). Relative URLs resolve against the document that contains them and MUST NOT resolve outside `{base}`'s origin and path prefix when the document is served from a static endpoint. URLs are `http` or `https`; `acquire_url` is `https` only.

1.7. **Size caps.** A client caps every read. Reference caps (`fe-hexon/src/registry_source.rs`): `index.json` 1 MiB; `config.json` and package detail 256 KiB; attestation 1 MiB; package 64 MiB and never more than the version's `size_bytes`.

1.8. **Identifiers are portable file names.** A `package_id` matches `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$` (the `hexon_id` charset of `docs/hexon-format-spec.md`, with a letter or digit first) and does not end in `.json` (compared ASCII case-insensitively). No two ids on one endpoint differ only in ASCII case. A `version` is SemVer 2.0.0 without build metadata. A client skips, and shows as a refusal line, an index entry or a version that breaks these rules; the rest of the document still loads.

1.9. **Forward compatibility.** Clients ignore unknown members and skip index entries whose `kind` they do not know, without failing the document. A document's `schema_version` bumps only on a removal, a rename or a change of meaning, and a client refuses a document whose `schema_version` is newer than it knows.

1.10. **Canonical JSON and signed bytes.** Canonical JSON is the form fe-format signs hexon manifests with (`fe-format/src/signature.rs`): object keys sorted by byte order at every level, no whitespace, strings escaped as serde_json escapes them, integers in their shortest decimal form. A catalogue signature is ed25519 over the UTF-8 bytes of

```
<domain string> 0x0A <canonical JSON of the whole document minus its signature member>
```

where the signature member is `signature` (index) or `proof` (token request). Domain strings:

| Document | Domain string | Signature member |
|---|---|---|
| `index.json` | `fractalengine/catalog-index/v1` | `signature` |
| token request | `fractalengine/entitlement-token-request/v1` | `proof` |

A verifier canonicalises the document **as received**, parsed as generic JSON with unknown members kept, never a re-serialised typed struct, so members added later stay covered. Verification is strict (`verify_strict`: no small-order keys, no non-canonical signature scalars).

1.11. **Signature encoding.** A verifier MUST accept a 64-byte ed25519 signature as lowercase or uppercase hex (128 characters) or as standard padded base64. A signer SHOULD emit lowercase hex, the encoding of the revocation list and of the reference signer (`registry_source::sign_index`). The two cannot be confused: valid hex is tried first, and a 128-character hex string is never a valid base64 encoding of 64 bytes.

## 2. Identity and kinds

2.1. `package_id` is the package's signed `hexon_id`. There is one id namespace across kinds, and an id's `kind` never changes on an endpoint.

2.2. `kind` is the store family and is extensible. Both v1 families are sealed `.hexon` packages: `web-bundle` is a package whose manifest v2 `kind` (`plugin/extension.json`) is `web-bundle` (`docs/spec/portal-bundle/README.md` §1); `extension` is a package with any other manifest v2 kind (`simulation`, `device-adapter`, `bi-connector`, `general`). A client MUST refuse a package whose verified manifest disagrees with its listing's `kind`.

2.3. `content_hash` is lowercase hex BLAKE3 (`fe-format/src/integrity.rs` `content_hash`) of the canonical manifest bytes with `signature` set to `""`: exactly the bytes the publisher signature and every counter-signature cover. Those bytes commit the integrity root and so every member, which gives one value per sealed build. A client recomputes it from `manifest_b64` and from the downloaded archive and never takes it on trust from a document.

2.4. `requires` maps an interface id to a SemVer requirement in Cargo syntax (`"2.0"` means `>=2.0.0, <3.0.0`):

| Kind | Interface id | v1 value | Meaning |
|---|---|---|---|
| `extension` | `fractalengine:plugin` | `"2.0"` | the manifest v2 `contract` `fractalengine:plugin@2.x` (`docs/extensions/manifest-v2.md`) |
| `web-bundle` | `fractalengine:portal-bundle` | `"1"` | the `portal.schema` `fe-portal-bundle/1` (`docs/spec/portal-bundle/README.md`) |

A client skips versions it cannot satisfy. `requires` only filters: the host re-checks the verified package.

## 3. Operations

| Op | Request | Response | Seam | Served by |
|---|---|---|---|---|
| discover | `GET {base}/api/v1/catalog/config.json` | Config (§4.1) | S-REGISTRY | every endpoint |
| list | `GET {base}/api/v1/catalog/index.json` | SignedIndex (§4.2) | S-REGISTRY | every endpoint |
| search | `GET {base}/api/v1/catalog/search?q=&kind=&category=&tag=&sort=&cursor=` | SearchPage (§4.8) | S-REGISTRY | feature `search`; otherwise the client filters the verified `index.json` by the same rules |
| detail | `GET {base}/api/v1/catalog/packages/{package_id}.json` | PackageDetail (§4.4), pinned by the index entry's `detail_hash` | S-REGISTRY | every endpoint |
| download | `GET {version.download_url}` | sealed `.hexon` bytes | S-REGISTRY | every endpoint, at any URL that serves the bytes without credentials |
| verify | `GET {base}/api/v1/catalog/packages/{package_id}/{version}/attestation.json` | Attestation (§4.6) | S-REGISTRY + S-PLUGIN | every endpoint |
| kill-list | `GET` the `revocation_url` configured locally beside a `revocation = true` trust root | revocation list (§4.7) | S-PLUGIN | feature `revocations`, at `{base}/api/v1/catalog/revocations.json` |
| entitlement check | `POST {base}/api/v1/catalog/entitlements/token` | TokenResponse (§4.10) | S-ENTITLEMENT | feature `entitlements` |

fe-hexon-registry's `/api/v1/hexons/*` routes, `/download` included, sit behind its bearer-token middleware whenever a token is configured, so a hosted `download_url` MUST name a path served without one (rule 1.3).

**Search semantics** (server and local fallback alike):
- `q`: case-insensitive (Unicode lowercase) substring over `package_id`, `name` and `summary`, mirroring v1 search (`fe-hexon-registry/src/routes.rs`). Empty or absent matches everything.
- `kind`, `category`: exact match.
- `tag`: comma-separated, all-of, exact and case-sensitive.
- `sort`: `updated` (default, newest `updated_at` first) or `name`; ties broken by `package_id` ascending (byte order). An unknown `sort` is `400 invalid_request` hosted and falls back to `updated` locally.
- `cursor`: opaque, hosted only.
- A search hit is **unsigned**. Before showing a badge, an acquire link or an install action, the client switches to that package's entry in the verified index; a hit with no verified index entry is shown without actions.

## 4. Payloads

### 4.1 Config (discover)

An unsigned bootstrap. It names keys but never confers trust.

```json
{
  "schema_version": 1,
  "registry_protocol": 1,
  "kinds": ["extension", "web-bundle"],
  "features": ["index", "detail", "attestations"],
  "index_key_id": "<advisory: the key id signing index.json now>",
  "revocation_url": "<advisory: where this endpoint publishes its kill list>"
}
```

- `registry_protocol` MUST be `1` (integer; the reference client also accepts the string `"1"`). Any other value is `ProtocolMismatch`, raised before the index is read.
- Features: `index`, `detail` and `attestations` on every endpoint; optional `search`, `revocations` and `entitlements`; reserved `featured`, `ratings` and `publishers` (§7). A client ignores feature names it does not know.
- `index_key_id` and `revocation_url` are advisory: a client MUST NOT select, widen or add trust from them (rule 5.1). They exist so an operator can show what a client should pin.

### 4.2 SignedIndex (list)

```json
{
  "schema_version": 1,
  "issued_at": "2026-11-07T00:00:00Z",
  "expires_at": "2026-11-14T00:00:00Z",
  "entries": ["IndexEntry, ..."],
  "signer_key_id": "<index key id>",
  "signature": "<ed25519, rule 1.10 with domain fractalengine/catalog-index/v1, encoded per rule 1.11>"
}
```

The signature covers every member except `signature`, `signer_key_id` included. Each `package_id` appears once, and a package is listed while it has at least one non-yanked version. The index is the one signed catalogue document: everything a client acts on is either signed here, pinned by a hash signed here, or signed by the package's own signers. An endpoint MUST include `expires_at` and re-sign before it passes; a client that meets an index without `expires_at` treats it as never stale.

### 4.3 IndexEntry

```json
{
  "package_id": "<hexon_id>",
  "kind": "extension",
  "name": "...",
  "summary": "...",
  "publisher": { "did": "did:key:...", "name": "...", "verified": false },
  "latest_version": "1.0.0",
  "latest_content_hash": "<64 lowercase hex>",
  "detail_hash": "<64 lowercase hex: BLAKE3 of packages/{package_id}.json exactly as served>",
  "access": "free",
  "categories": [],
  "tags": [],
  "icon_url": null,
  "updated_at": "2026-11-07T00:00:00Z"
}
```

`latest_version` and `latest_content_hash` name the newest non-yanked version. An entry whose `latest_content_hash` or `detail_hash` is not 64 lowercase hex is a refusal line.

### 4.4 PackageDetail (detail)

```json
{
  "schema_version": 1,
  "package_id": "<hexon_id>",
  "kind": "web-bundle",
  "name": "...",
  "description": "...",
  "publisher": { "did": "did:key:...", "name": "...", "verified": true },
  "access": "licensed",
  "acquire_url": "<https URL, or null>",
  "categories": [],
  "tags": [],
  "links": { "homepage": null, "source": null, "issues": null },
  "versions": ["Version, newest first"]
}
```

Served byte-for-byte. The client checks its BLAKE3 against `detail_hash` from the verified index, and its `package_id` and `kind` against that index entry (`DetailMismatch`), **before** it reads any other field.

### 4.5 Version

Newest first, in the SemVer order v1 `resolve` uses (`fe-hexon-registry/src/index.rs`).

```json
{
  "version": "1.0.0",
  "content_hash": "<64 lowercase hex>",
  "size_bytes": 123456,
  "published_at": "2026-11-07T00:00:00Z",
  "requires": { "fractalengine:plugin": "2.0" },
  "download_url": "<absolute, or relative to this document>",
  "yanked": false
}
```

The download URL comes from this document, never from a caller.

### 4.6 Attestation (verify)

```json
{
  "schema_version": 1,
  "package_id": "<hexon_id>",
  "version": "1.0.0",
  "content_hash": "<64 lowercase hex>",
  "manifest_b64": "<base64 of the archive's exact manifest.json bytes, publisher signature included>",
  "header_members_b64": { "<header member path>": "<base64 of its exact bytes>" },
  "signatures": [{ "key_id": "...", "signature": "<base64 or hex>" }]
}
```

The attestation is the header phase served as a file. `manifest_b64` and `header_members_b64` are exactly the inputs of fe-format's `verify_sealed_header` (`fe-format/src/integrity.rs`): `header_members_b64` carries **every** member the manifest's integrity root commits in its header (`HEADER_MEMBER_PATHS`), so for an extension at least `plugin/extension.json` and `members.json`, plus `license.json` for a licensed package. A client therefore verifies the publisher signature and renders consent before it downloads the body. The publisher signature is the manifest's own `signature`, by its `publisher_did`. `signatures` holds further signatures over the same canonical bytes (`content_hash`'s preimage), such as a counter-signature, and is `[]` until the endpoint counter-signs. A missing attestation is `HeaderUnavailable`: consent then renders after the download, from the verified package.

### 4.7 Revocation list (kill list)

The S-PLUGIN revocation list, byte-for-byte (`fe-plugin/src/trust_roots.rs` `RevocationList`):

```json
{
  "schema_version": 1,
  "issued_at": "2026-11-07T00:00:00Z",
  "entries": [{ "package_id": "...", "content_hash": "<64 lowercase hex>", "reason": "...", "revoked_at": "2026-11-07T00:00:00Z" }],
  "signer_key_id": "<revocation-capable trust root id>",
  "signature": "<lowercase hex ed25519>"
}
```

This contract redefines none of its fields. Its signature is the S-PLUGIN form, **not** rule 1.10: ed25519 (strict) over the canonical JSON of the object `{ "entries": ..., "issued_at": ... }` only, with no domain line, encoded as hex. One list covers every code-bearing kind.

**Matching is by hash, under any id** (`docs/extensions/package-format.md` §Identity). An entry revokes an installed or candidate package when its `content_hash` equals the package's `content_hash`, its `component_digest` (`members.json["plugin/component.wasm"]`), or, for a licensed package, its host-computed `plaintext_digest`. The entry's `package_id` is informational and never narrows the match, so a renamed copy stays revoked.

### 4.8 SearchPage (search)

```json
{ "items": ["IndexEntry, ..."], "next_cursor": null }
```

`next_cursor` is a string or `null`. Items are IndexEntry shapes and carry no trust (§3, search semantics).

### 4.9 Token request (entitlement check)

```json
{
  "package_id": "<hexon_id>",
  "content_hash": "<64 lowercase hex>",
  "licensee_did": "did:key:<ed25519 identity DID>",
  "device_x25519_did": "did:key:<X25519 device key DID>",
  "iat": 1762473600,
  "proof": "<base64 ed25519 signature>"
}
```

`proof` is the licensee DID's ed25519 signature under rule 1.10 with the domain `fractalengine/entitlement-token-request/v1`, over the other five members. `iat` is Unix seconds. The domain string is domain separation: the same identity key signs gossip frames and API tokens, and a request signature must never be valid as either. The reference helper is `fe_format::entitlement::token_request_proof` (feature `entitlement`).

### 4.10 TokenResponse and errors

| Status | Body | When |
|---|---|---|
| 200 | `{ "token": "<EdDSA JWT carrying fe_format::entitlement::EntitlementClaims>" }` | entitled |
| 400 | `invalid_request` | malformed body |
| 401 | `proof_invalid` | bad signature, or `iat` more than 300 s from server time |
| 403 | `not_entitled` | no licence, including while an acquisition is still completing |
| 403 | `reissue_refused` | the licence was revoked; tokens are short-lived and the issuer refuses to reissue |
| 403 | `device_limit_reached` | the licence admits no further devices |
| 404 | `unknown_package` | no such `(package_id, content_hash)` |
| 429 | `rate_limited` | too many requests |

Error bodies use the envelope of rule 1.5. A token covers exactly one `content_hash`, so after an update the client asks again. A client verifies every token locally with `EntitlementVerifier` against its pinned issuer keyring; the response status is never trust.

### 4.11 Field rules

- `access` is `free` or `licensed`, a listing hint. The verified `license.json` header member decides: `licensed` means the package's `License` block requires an entitlement for its non-free entries, and `free_entries` still load without one. An unknown `access` value means not installable from this listing.
- `publisher.did` MUST equal the verified manifest's `publisher_did`, or the client refuses the package. `publisher.verified` is a claim signed through the index: the endpoint's operator has verified this publisher. An endpoint without publisher verification sets it only for the operator's own publisher key.
- `acquire_url` is present (non-null) only for a `licensed` package on an endpoint that has the `entitlements` feature. Licence terms live on the page it opens, never in a catalogue document.
- `size_bytes` is the exact archive size; a client aborts a download that exceeds it.
- `yanked: true` hides a version from new installs. An installed yanked version keeps working.

## 5. Client obligations

5.1. **Trust comes only from keys pinned locally.** Verify `index.json` against the index key set pinned for this endpoint (`SourceTrust::index_signer`). The pin is set out of band (the client's registry profile, or the user adding an endpoint), never taken from `config.json`, and MAY hold more than one key id (current and next) so the operator can rotate on a schedule without a client release. An index key is never accepted as a package or kill-list signer, and a package trust root is never accepted as an index signer (`IndexKeyMisused`): a stolen index key can hide or reorder listings but cannot ship or revoke code. Verify package signatures against the trust roots (`trusted_publishers.toml`), accept a kill list only from a root flagged `revocation = true`, and verify tokens with `EntitlementVerifier`. There is deliberately no "is this valid?" endpoint: a client that asks the server has handed its trust to the transport.

5.2. **Badges come from signatures, never from fields.** "Reviewed" means a counter-signature in `signatures` verified against a trust root the host honours. UI copy says "passed automated checks and human review", never "audited" or "safe".

5.3. **One verified chain from index to bytes.** The verified index pins the detail document (`detail_hash`), which names the version's `content_hash`. The attestation MUST recompute to that `content_hash`, and its manifest's `hexon_id` and `version` MUST equal the requested `package_id` and `version`. It then passes `verify_sealed_header` with the listing's `publisher.did` as the pin, and consent renders from its verified header members. The download MUST pass fe-format's full seal check with the same `content_hash` and `publisher_did`. A break anywhere refuses the install; nothing is installed from an unverified document.

5.4. **Refuse kill-listed packages.** Never install or activate an extension, or render a web bundle, that a verified kill list revokes (§4.7). That holds even in the no-IPC static sandbox unadmitted bundles get, because killed content can be harmful without IPC. The only exception is the audited verse-Owner override on that Owner's own host (`docs/extensions/governance.md`), which this contract neither grants nor transports. Keep the last good list: an unreachable URL never un-revokes, and a list whose `issued_at` is older than the cached one never replaces it.

5.5. **Expiry means stale, not blocked.** An `index.json` past `expires_at` is shown as stale and refetched; it never blocks an install, because package signatures and the kill list carry the security. An index whose `issued_at` is older than the cached one is ignored (rollback guard). A package that leaves the index (every version yanked, or taken down) is **not** revoked: installed copies keep working, and only the kill list revokes.

5.6. **Downloads need no auth.** Licensed payloads are ciphertext at rest and decrypt in memory with a token (S-ENTITLEMENT), so static hosts and CDNs can serve them.

5.7. **Acquire in the system browser.** Open `acquire_url` in the system browser, never an embedded webview, so the user sees the real origin. Append `licensee_did` and `device_x25519_did` as percent-encoded query parameters (joined with `&` when the URL already has a query) so the licence binds to that identity and device. The client then gets its token through the entitlement check, which answers `not_entitled` until the acquisition completes.

5.8. **Credentials stay home.** Rule 1.3 binds every fetcher the client hands a catalogue URL to, including redirects and URLs named inside documents.

## 6. Static-index stub

A complete static endpoint is this tree on any static file server:

```
api/v1/catalog/config.json                                  features: index, detail, attestations
api/v1/catalog/index.json                                   signed with a test index key
api/v1/catalog/packages/<package_id>.json
api/v1/catalog/packages/<package_id>/<version>/attestation.json
api/v1/catalog/revocations.json                             optional; feature revocations
blobs/<content_hash>.hexon                                  any path; download_url points at it
```

Against this tree alone a client MUST browse, filter locally, verify, install free packages of both kinds and honour a kill list. Every file is served byte-for-byte, because the index pins detail documents by hash, and the index is re-signed before its `expires_at`. Fixture signatures and hashes are generated by the public canonical-JSON signer (`fe-ext index build`), never written by hand; the public side owns the test vectors ([`fixtures/`](fixtures/README.md)).

## 7. Reserved paths

`featured.json`, the `publishers/` prefix, `packages/{package_id}/ratings.json` (GET), and a ratings POST carrying a DID proof. They are feature-gated (`featured`, `publishers`, `ratings`) and shaped when those features ship. A DID's `:` is not a portable file-name character, so the publisher path form is fixed then. The paths are reserved now so no earlier document takes them; a v1 endpoint MUST NOT serve other documents at them.

## 8. Reference implementations

| Side | Code |
|---|---|
| Client: static index, detail pin, attestation, download | `fe-hexon/src/registry_source.rs` (`StaticIndexSource`, `verify_index`, `index_signed_bytes`, `sign_index`, `valid_package_id`, `valid_version`) |
| Client: protocol-v1 transport | `fe-hexon/src/remote.rs`, `registry_source.rs` (`ProtocolV1Source`) |
| Kill list | `fe-plugin/src/trust_roots.rs` (`RevocationList`, `RevocationSet`) |
| Package identity and header phase | `fe-format/src/integrity.rs` (`content_hash`, `verify_sealed_header`, `HEADER_MEMBER_PATHS`) |
| Entitlement | `fe-format` feature `entitlement` (`EntitlementClaims`, `IssuerKeyring`, `EntitlementVerifier`, `token_request_proof`) |
| Hosted server | `fe-hexon-registry` (`routes.rs`, `PackageStore`) |
| Static index tooling | `tools/fe-ext` (`fe-ext index build`, `fe-ext index verify`) |

Known gaps at HEAD are tracked in [`README.md`](README.md) §Conformance status, not here.
