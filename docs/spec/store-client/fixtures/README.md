# Store-client v1 fixtures

Templates for a complete static endpoint (contract §6) plus the loose documents only a hosted endpoint serves. Every hash, signature, DID and base64 member is a **placeholder**: fixtures are materialised by the public canonical-JSON signer, never filled by hand (contract §6).

## Tree

```
static-endpoint/                                        the endpoint root ({base})
  api/v1/catalog/config.json                            features: index, detail, attestations, revocations
  api/v1/catalog/index.json                             signed by index-test-1
  api/v1/catalog/packages/<package_id>.json             one per package
  api/v1/catalog/packages/<package_id>/<version>/attestation.json
  api/v1/catalog/revocations.json                       signed by revocation-test-1
  blobs/<content_hash>.hexon                            generated archives (not committed)
documents/
  hosted-config.json                                    every optional feature advertised
  hosted-licensed-detail.json                           a licensed detail document with an acquire_url
  search-page.json                                      a SearchPage (tag "readings")
  token-request.json                                    an entitlement token request
  token-response.json                                   its 200 body
  errors.json                                           every token-request error envelope
vectors.json                                            the test vectors
```

| Package | Kind | Access | Versions | Exercises |
|---|---|---|---|---|
| `org.example.fleet-sim` | extension | free | 1.1.0, 1.0.0 | the happy path, detail pin, header before download, every mutation vector |
| `org.example.site-viewer` | web-bundle | free | 1.0.0 | a web bundle through the same client; operator-verified publisher |
| `org.example.analytics-pack` | extension | licensed | 1.0.0 | `license.json` in the header; no acquire on a static endpoint; the entitlement exchange |
| `org.example.scene-pack` | `scene-pack` | free | 1.0.0 | an unknown kind, skipped without failing the index |
| `org.example.yank-demo` | extension | free | 2.0.0 (yanked), 1.0.0 | `latest_version` skips a yanked version; a yanked install is refused |
| `org.example.revoked-sample` | extension | free | 1.0.0 | kill-listed by `content_hash` under another id (`org.example.renamed-copy`) |

## Placeholders

A placeholder is a whole JSON string `{{name:argument}}`. The generator replaces the string with the value's JSON form; `size_bytes` becomes an integer, everything else stays a string.

| Placeholder | Value |
|---|---|
| `{{content_hash:<id>@<ver>}}` | `fe_format::integrity::content_hash` of the generated archive's manifest |
| `{{component_digest:<id>@<ver>}}` | `members.json["plugin/component.wasm"]` of that archive |
| `{{size_bytes:<id>@<ver>}}` | the archive's exact length |
| `{{manifest_b64:<id>@<ver>}}` | standard base64 of its exact `manifest.json` bytes |
| `{{member_b64:<id>@<ver>:<path>}}` | standard base64 of that header member. The template lists the expected members; the generator emits exactly the members the manifest's integrity root commits in its header, adding or dropping keys to match |
| `{{detail_hash:<id>}}` | BLAKE3 hex of the materialised `packages/<id>.json` bytes |
| `{{did:<key>}}` | `did:key` of that test key's ed25519 public half |
| `{{x25519_did:<key>}}` | `did:key` of that test X25519 public key |
| `{{signature:<key>}}` | the document's signature by that key: rule 1.10 for `index.json`, the S-PLUGIN form for `revocations.json`; lowercase hex |
| `{{proof:<key>}}` | the token-request proof (domain `fractalengine/entitlement-token-request/v1`), standard base64 |
| `{{jwt:<issuer>:<id>@<ver>}}` | an EdDSA JWT with `EntitlementClaims` for that `content_hash`, by that issuer key |

Test keys are derived deterministically from their names (32-byte seed = BLAKE3 of `fe-store-client/1 test key:<name>`), so every implementation materialises byte-identical fixtures. They are test keys only; no client profile pins them.

## Generation order

Each step depends on the previous one, so a generator runs them in this order:

1. Build and seal one archive per `<id>@<ver>` with the publisher key named by the package's `publisher.did`, write it to `blobs/<content_hash>.hexon`.
2. Fill `content_hash`, `component_digest`, `size_bytes`, `manifest_b64` and `member_b64` everywhere.
3. Fill the DIDs, then write every detail document's final bytes.
4. Fill `detail_hash` in `index.json` from those bytes.
5. Sign `index.json` and `revocations.json`; sign the token request; issue the token.

A vector's `mutate` list applies after materialisation: `patch` is RFC 6902 JSON Patch against the named file; `rehash: true` recomputes the index's `detail_hash` for a changed detail document; `resign` is `true` (re-sign with the document's own key), `false` (keep the now-stale signature), a key name (re-sign with that key), or `raw` (sign the canonical bytes without the integers-only check, to test the verifier's check); `signature_encoding` picks `hex` (default) or `base64`; `delete: true` removes the file.

## Vectors

[`vectors.json`](vectors.json) holds 56 vectors: `SC-01`..`SC-22` positive flows (discover, list, local search semantics, detail pin, header before download, installs of both kinds, yank, kill list, licensed listing, acquire link, token exchange, forward compatibility) and `SC-N01`..`SC-N34` refusals and edge cases (signature, unknown and misused signer, schema, floats, id grammar and case collision, build metadata, detail pin and kind, rollback, expiry, protocol, advisory config, attestation binding, missing attestation, size cap, hash mismatch, URL climbing and schemes, publisher and kind disagreement, kill-list rollback, outage and signer separation, component-digest match, delisting, unsigned search hits, token proof replay and domain, no credentials, document-relative URLs, signature encodings). `expect.refusal` names the `fe_hexon::registry_source::RegistryError` variant, or the governance refusal `revoked`; `null` means the refusal is required but its name is the implementation's.
