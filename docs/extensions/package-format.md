---
type: reference
title: Extension package format (sealed plugin hexon)
track: extension_platform_20261107
status: draft (P0.4; built in P2.1 / slice E9)
---

# Extension package format

A third-party extension ships as ONE sealed `.hexon` archive of a new type, `plugin` (D-X1;
plugin_host_revival OD-9 closed by D-X1). Everything that already applies to sealed hexons applies
unchanged: the D-76 integrity root, the ed25519 manifest signature, registry protocol v1,
`PackageStore` and S-ENTITLEMENT. This page defines only what is plugin-specific.

The `plugin` family has two content kinds (manifest v2 `kind`). The container, identity, signature
and revocation rules below are shared by both, so web bundles pass the same verifier (D-X5). The
body members differ: extension kinds carry one Wasm component (this page); `kind: web-bundle`
carries `web/` members and never a component, as `portal_surface_20261107` specifies
(`docs/spec/portal-bundle/README.md` §1, which also admits unsealed bundles for the static sandbox).

## Type

`HexonType::Plugin`, serialized `"hexon_type": "plugin"` (fe-format `manifest.rs`, `snake_case`).
The doc edit to `docs/hexon-format-spec.md` lands with slice E9, after hexon_entitlements S5's
1.1.0 bump.

## Members (closed set)

| Member | Phase | Committed by | Required | Content |
|---|---|---|---|---|
| `manifest.json` | header | the signature | yes | `HexonManifest`: `hexon_type: "plugin"`, `hexon_id`, `version`, `publisher_did`, `integrity`, `signature`. |
| `entries.json` | header | `integrity.header` | yes | An empty fe-format `EntryCatalog`, exactly `{"entries": []}`. fe-format's lazy reader (`HexonArchiveIndex::open`, which the registry's header and download routes use) refuses an archive without it, so a package without it cannot ride registry protocol v1. |
| `members.json` | header | `integrity.header` | yes | Body digests, written by `seal`. |
| `plugin/extension.json` | header | `integrity.header` | yes | Manifest v2 ([manifest-v2.md](manifest-v2.md)). Joins `HEADER_MEMBER_PATHS` so consent renders before the body is fetched. ≤ 64 KiB. |
| `license.json` | header | `integrity.header` | no | fe-format `License`; a `paid` licence makes the component an encrypted entry (below). |
| `plugin/component.wasm` | body | `members.json` | yes (extension kinds) | One Wasm **component** (not a core module) targeting the manifest's `contract`. ≤ 16 MiB (host ceiling; fe-format's own entry cap is 64 MiB). A web bundle must not carry it. |
| `plugin/countersignatures.json` | header | **nothing** | no | Counter-signatures (below). Besides `manifest.json` itself, the only member the integrity root never commits. |

For an extension kind, any other member is refused (`package-invalid`, reason `uncommitted-member`
or `unexpected-member`); a web bundle's `web/` members follow the portal contract.
`HexonArchive::export` is not a plugin packer: it writes no plugin members. `fe-ext pack` and
fe-hexon's `PluginHexonBuilder` write the members above directly, then `seal`.
`seal` is the last transform, as for every sealed hexon (fe-format `src/AGENTS.md` §integrity).

## Identity

| Value | Definition |
|---|---|
| `package_id` | `manifest.json` `hexon_id`. Must equal `plugin/extension.json` `id`. |
| `version` | `manifest.json` `version`. Must equal `plugin/extension.json` `version`. |
| `content_hash` | `lowercase_hex(blake3(signing_bytes))` |
| `component_digest` | `members.json["plugin/component.wasm"]`: lowercase blake3 hex of the component bytes as stored (ciphertext for a licensed package). |
| `plaintext_digest` | Licensed packages only: lowercase blake3 hex of the decrypted component, computed by the host in the body phase. Never in the archive, never trusted from a registry. |
| `signing_bytes` | The canonical JSON of `manifest.json` with `"signature": ""` — exactly the bytes `fe_format::signature::sign_manifest` signs (sorted keys, no whitespace). |

The manifest commits `integrity`, which commits `plugin/extension.json` and `members.json`, which
commits `plugin/component.wasm`. So `content_hash` names one sealed build: change any byte of any
committed member and it changes. Counter-signing does not touch `manifest.json`, so it never
changes `content_hash`. `(package_id, content_hash)` is the key for revocation, admission, consent,
entitlement tokens and provenance.

**The id is signed, and must match what was asked for** (orchestrator steer 2026-11-08 20:10,
revocation bypass by renaming). `hexon_id` sits inside `signing_bytes`, so no registry, mirror or
file copy can rename a package without breaking its signature. On top of that, the id the install
names — the REST install body, the registry document's `package_id`, the requested registry URI —
must equal the signed `hexon_id` (and `plugin/extension.json` `id`); any mismatch is
`package-invalid` (`id-mismatch`) before anything else is trusted.

**Revocation matches by hash, under any id.** An accepted revocation entry revokes an installed
or candidate package when the entry's `content_hash` equals the package's `content_hash`, its
`component_digest` (`members.json["plugin/component.wasm"]`, the blake3 of the component bytes),
or, for a licensed package, its `plaintext_digest`. The entry's `package_id` is informational and
never narrows the match. A list publisher may name any of these values; naming the
`component_digest` also catches the same component re-sealed under a new id or version, and naming
the `plaintext_digest` also catches a licensed component re-encrypted under a new content key
(whose ciphertext, and so `component_digest`, changes). The header phase checks `content_hash`
and `component_digest`; the body phase checks `plaintext_digest` after decryption. The list's wire
shape is unchanged (plugin_host_revival §4).

## Signatures

**Publisher signature.** `manifest.json` `signature`: base64 ed25519 over `signing_bytes`, by the
key of `publisher_did` (`did:key:z6Mk…`). This IS plugin_host_revival Phase 2's publisher
signature, retargeted from `manifest_bytes ‖ blake3(payload)` to the sealed manifest, which
already commits the payload.

**Counter-signatures.** `plugin/countersignatures.json`:

```json
{
  "schema_version": 1,
  "content_hash": "9f2c…",
  "entries": [
    { "key_id": "did:key:z6MkReviewer…", "signature": "base64 ed25519 over signing_bytes" }
  ]
}
```

- Each entry signs the SAME `signing_bytes` as the publisher (plugin_host_revival's `{key_id,
  signature}` list), so a second party can counter-sign a published package without resealing it.
- `key_id` is the signer's `did:key`, so a signature is checkable without a lookup; whether that
  key is trusted is trust-root policy ([governance.md](governance.md#trust-roots)), not format.
- `signature` is base64, the same encoding as `manifest.json` `signature` (fe-format
  `sign_manifest`). plugin_host_revival's pre-hexon `SignatureEntry` carries hex; that encoding is
  not reused here, so one package never mixes two.
- `content_hash` must equal the package's; a mismatch is `package-invalid` (`countersignature-mismatch`).
- An entry whose `key_id` is a configured trust root and whose signature fails is `package-invalid`
  (a tampered counter-signature). Entries from unknown keys are ignored.
- ≤ 16 entries, ≤ 64 KiB.

**Trust decision** (governance R17): the publisher signature must verify; then the package is
trusted when the ed25519 key of `publisher_did`, or of the `key_id` of at least one verifying
counter-signature, equals a trust root's `public_key`. Roots match by key bytes, never by label:
the trust-roots file names keys with free-form `key_id` labels (plugin_host_revival S2,
`fe-plugin/src/trust_roots.rs`), while packages name signers by `did:key`. An unsigned package (no
`signature`) is refused unless the dev flag is set, and that flag is unreachable from release builds.

## Component checks at install

| Check | Refusal |
|---|---|
| `plugin/component.wasm` is a core module, or `contract` is not `fractalengine:plugin@2.x` | `unsupported-contract` |
| The component imports anything outside `fractalengine:plugin@2.x` and the WASI list in `fe-plugin/wit/AGENTS.md` §wasi-policy | `package-invalid` (`forbidden-import <name>`) |
| The component does not export all four interfaces of world `extension` | `package-invalid` (`missing-export <name>`) |
| Manifest v2 fails validation | `package-invalid` (the manifest reason, e.g. `empty-scope`) |

## Header phase, then body

1. **Header.** A registry source or a local file yields `manifest.json` plus the header members.
   The host runs `verify_sealed_header` (signature, integrity root, header digests), parses
   `plugin/extension.json`, checks identity equality, computes `content_hash`, checks the revocation
   list, and renders consent. Nothing executable has been fetched.
2. **Body.** The host fetches `plugin/component.wasm`, checks its digest against `members.json`,
   decrypts it if licensed (then checks the revocation list against `plaintext_digest`), runs the
   component checks, and stores the sealed bytes in the package store.

## Licensed packages

A `license.json` with `license_type: "paid"` makes `plugin/component.wasm` an encrypted entry,
exactly like a licensed hexon entry (hexon_entitlements FR-4/FR-5: recipient-wrapped content key,
`EntitlementVerifier` with pinned issuer keys). The entitlement token's `package_id` and
`content_hash` are the values above. No new verifier code: activation precondition
`entitlement-required` calls the existing verifier ([governance.md](governance.md#activation-preconditions)).

## Registry wire

Unchanged registry protocol v1 (`X-Hexon-Registry-Protocol: 1`). A registry serves the header
members for consent before download (spec R29 `header`), and the sealed bytes for `fetch`; the host
always re-verifies locally.

## Journey J5 (P2.1, extends `fe-hexon/src/package.rs::test_tampered_package_fails_signature`)

Pack and sign the fleet-sim example, then install each variant through the real install path.
Every row is refused with its own reason and the package store is unchanged afterwards.

| Variant | Reason |
|---|---|
| one byte of `plugin/component.wasm` flipped | `package-invalid` (body digest mismatch) |
| `plugin/extension.json` edited (one scope added) | `package-invalid` (header digest mismatch) |
| an extra member `plugin/extra.bin` not in `members.json` | `package-invalid` (`uncommitted-member`) |
| `signature` removed, dev flag absent | `untrusted-signer` (unsigned) |
| signed by a key that is no trust root, no counter-signature | `untrusted-signer` |
| a trusted counter-signature entry with a bad signature | `package-invalid` (`countersignature-mismatch`) |
| a 1.0 core module packaged as `plugin` | `unsupported-contract` |
| the valid package requested as `org.example.other` (REST body or registry document id) | `package-invalid` (`id-mismatch`) |
| a revocation list naming the package's `content_hash` under a different `package_id` | `revoked` |
| the same component re-sealed under a new id, with a list naming its `component_digest` | `revoked` |
| the valid package | installed; `content_hash` and `component_digest` printed by `fe-ext inspect` match, and a loopback fe-hexon-registry serves its header (`plugin/extension.json` readable before download) |
