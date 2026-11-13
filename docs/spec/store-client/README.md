# Store-client contract

The public contract between a FractalEngine client and a package catalogue endpoint: how a client discovers an endpoint, lists and searches packages, pins detail documents, verifies a package's header before download, downloads sealed bytes, honours a kill list and asks for an entitlement token. Every rule is written so a directory of static files is a complete endpoint.

| File | What it is |
|---|---|
| [`contract-v1.md`](contract-v1.md) | The normative text, `fe-store-client/1`: transport, identity, operations, payloads, client obligations, static stub, reserved paths. |
| [`fixtures/README.md`](fixtures/README.md) | The fixture tree (a complete static endpoint plus loose documents), its placeholder grammar and the generation order. |
| [`fixtures/vectors.json`](fixtures/vectors.json) | Test vectors: each names fixtures, an operation and the expected outcome. Client and endpoint implementations both run them. |

## Who implements it

- **Clients**: `fe-hexon/src/registry_source.rs` (Browse, Details, Install, Update in the Extensions panel, `docs/extensions/extensions-ui.md` §Registry sources) and the binary's fetcher and install path (`fractalengine/src/extension_host/`).
- **Endpoints**: any static host (built with `fe-ext index build`), and the hosted registry server `fe-hexon-registry`. Any other implementation that passes the vectors is conforming.
- **Seams**: S-REGISTRY (catalogue), S-PLUGIN (publisher signatures, trust roots, kill list), S-ENTITLEMENT (token request and verifier), as listed in `conductor/open-core.md`.

## Design notes

- **One signed document.** Only `index.json` carries a catalogue signature. Detail documents are pinned by the BLAKE3 hash the index signs; attestations and archives are bound by the package's own publisher signature and `content_hash`. That keeps the static host dumb (it serves bytes) and the trust chain short.
- **Separate keys, separate powers.** The index key only orders and hides listings. Code is shipped by publisher keys and revoked by revocation-capable trust roots. A compromised index key therefore cannot ship or revoke code (contract §5.1).
- **Leaving the index is not revocation.** Takedown and yank stop new installs; only the kill list stops running code, so a catalogue outage or a delisting never breaks an installed extension (§5.5).
- **No catalogue credentials.** Downloads are anonymous and licensed payloads are encrypted at rest, so CDNs and mirrors can serve everything; the only authenticated call carries its own DID proof (§1.3, §4.9).
- **Licence terms are not catalogue data.** The contract carries `access` and an `acquire_url`; everything else about obtaining a licence happens on that page, in the system browser (§5.7).

## Conformance status at HEAD (2026-11-13)

Differences between `contract-v1.md` and the code it names, for the owning slices. Each is a code gap, not a contract choice.

| Contract rule | HEAD | Owner |
|---|---|---|
| §2.2 `web-bundle` entries listed and installed through the same client | `registry_source.rs` `verify_index` skips every kind except `extension` | S-STORE / S-HOST / P-STORE |
| §4.1 `config.json` at `{base}/api/v1/catalog/config.json` on the hosted server | `fe-hexon-registry/src/routes.rs` routes `/config.json` at the root | S-PSTORE / I-HEXON |
| §4.7 kill-list match on `component_digest` and `plaintext_digest` as well as `content_hash` | `fe-plugin/src/trust_roots.rs` `RevocationSet` keeps `content_hash` values only; the install path must also query the component digest | S-HOST / X-GOV |
| §1.6 relative URLs resolve against the containing document (vector SC-N33) | `StaticIndexSource::resolve` joins every relative location to the catalogue directory and refuses any `..`, so the fixture's `../../../../blobs/…` is refused | S-STORE |
| §1.4 `features` read and honoured (`search` server-side, `revocations`, `entitlements`) | `StaticIndexSource` reads `registry_protocol` only and filters locally | S-STORE |
| §4.9 / §4.10 token request helper and verifier | `fe_format::entitlement` does not exist at HEAD | S-ENT |
| §5.4 rendering refusal for kill-listed web bundles in the static sandbox | portal admission path | P-RES / P-STORE |
