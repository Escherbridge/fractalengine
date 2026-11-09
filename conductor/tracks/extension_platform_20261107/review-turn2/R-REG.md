---
type: review
track: extension_platform_20261107
title: R-REG — registry supply chain (store-client v1) security review, swarm turn 2
resource: ../metadata.json
---

# R-REG — registry supply chain (turn 2, over F-REG `11599965` and F-OPS `f03795c4`)

Security review-and-fix, code-first (nothing built or run). One verdict line per finding:
**FIXED** (edited here), **KEPT** (checked, left as is, reason given), **HANDED-OFF** (owner named),
**DEFERRED** (trigger named). Refutation targets come from the slice task; the store-client contract
v1 is the 2026-11-08 20:45 steer (c), the credential rule is the 20:20 steer, and id binding is the
20:10 steer.

## Credentials to catalogue URLs

- R-REG-01 KEPT `fractalengine/src/extension_host/sources.rs` `AnonymousHttpFetcher`: no auth header, no cookie store, `no_proxy`, `Policy::none()` redirects, userinfo in any location refused, and the connection pinned to the addresses it checked (`resolve_to_addrs`), so DNS rebinding cannot move it.
- R-REG-02 FIXED `fe-hexon/src/registry_source.rs` `StaticIndexSource::resolve`: a signed detail document's absolute location was read as-is. For a folder source on Windows, `download_url: \\host\share\x.hexon` made the OS authenticate (NTLM) to a host the document chose. Now a document may name only an http(s) URL or a relative, non-climbing path; absolute, UNC, drive and scheme paths are `DetailMismatch` (journey row: UNC download url).
- R-REG-03 FIXED `fe-hexon/src/remote.rs`, `sources.rs`: the protocol-v1 Extensions source was anonymous only because the caller passed `None`. The new `remote_extension_source(label, base, trust)` has no token parameter, so it is anonymous by construction, and `sources.rs` uses it.
- R-REG-04 KEPT `remote.rs` `RemoteRegistryClient` token (tileset path; env-only operator token): it goes only to URLs built from `base_url`, and reqwest 0.12.28 strips `Authorization` on a cross-host redirect (`redirect.rs:239`). Redirects are still followed, so an untrusted registry can cause a blind GET; the body is verified and never echoed. Trigger to revisit: any route that echoes a fetched body.

## Index key provenance

- R-REG-05 KEPT `registry_source.rs` `check_protocol` and `verify_index`: `config.json` `index_key_id` is never read for trust. `signer_key_id` resolves only in the per-endpoint pinned set, which `sources.rs` builds from the operator's `extension_sources.json`.
- R-REG-06 FIXED `sources.rs` `parse_pins` and `build_one`: a duplicated pinned `key_id` silently used the first entry, and is now refused. A pin that is a package trust root is now refused when the source is built, with a plain reason; `verify_index` still re-checks `IndexKeyMisused` on every load.
- R-REG-07 HANDED-OFF I-BIN (`fractalengine/src/plugin_bridge.rs`): `IndexKeyMisused` also covers revocation signers only if the shared `SourceTrust.is_trust_root` answers true for the did:key of every key in `trusted_publishers.toml`, revocation-capable keys included (extensions-ui.md calls this "the bridge's configuration rule").
- R-REG-08 FIXED `registry_source.rs` `verify_index`: the index signature used ed25519 `verify`; it now uses `verify_strict`, as the revocation list does (`trust_roots.rs`).

## detail_hash, package_id, kind, requested id

- R-REG-09 KEPT `registry_source.rs` `vouched_version`: it checks the BLAKE3 of `packages/{id}.json` as served against the signed `detail_hash`, then `package_id` and `kind` against the signed entry, the latest version's hash against `latest_content_hash`, and refuses a yanked version. The download url comes only from that document.
- R-REG-10 FIXED `remote.rs` protocol-v1 listings had no kind check, so Extensions Browse offered tilesets and models, which were refused only at fetch as `not-a-plugin`. `remote_extension_source` now asks for `type=plugin` and re-filters client-side (journey: the tileset fixtures are not offered).
- R-REG-11 FIXED `registry_source.rs` `ProtocolV1Source::{header, fetch, fetch_hexon}`: the request uri (`entry.url`) was not bound to `entry.package_id` or `version`. `requested_uri_matches` now refuses a mismatch before any request (journey row: renamed uri).

## Older index, older kill list

- R-REG-12 FIXED `registry_source.rs` and `sources.rs`: the rollback guard compared only against a cache file, and `cache` defaults to none, so any older, validly signed index was accepted. Now:
  - the newest index verified in this process is a floor;
  - `load_source_configs` defaults a static source's cache to `<sources dir>/extension-index-cache/index-<id>.json`, so the guard survives a restart;
  - the cache's parent directory is created.

  Journey row: an uncached source ignores an older index.
- R-REG-13 HANDED-OFF R-GOV: kill-list freshness. `trust_roots.rs` `verify_revocation_list_fresh` refuses an older list per signer against an in-memory high-water mark. `extension_host/revocation_pull.rs`, which must keep that mark across restarts, is not in the tree yet.

## size_bytes and transport bounds

- R-REG-14 KEPT: a static fetch is capped at the detail's `size_bytes` (at most `PACKAGE_MAX_BYTES`), enforced by `FileFetcher`'s metadata check and by `AnonymousHttpFetcher`'s Content-Length check plus a streaming cap. A protocol-v1 fetch is capped at the listed `size` (at most 64 MiB, `download_cap`).
- R-REG-15 FIXED `remote.rs` `RemoteRegistryClient::download` read an unbounded body (`resp.bytes()`); it now goes through `download_capped` at `MAX_ARCHIVE_UNCOMPRESSED_BYTES`.
- R-REG-16 FIXED `remote.rs` `RemoteRegistryClient` had no timeouts, so a stalled registry held the single extension ops worker and every enable and consent waited behind it. It now has a 10 s connect timeout and a 60 s read-idle timeout; the idle form is what lets a large tileset keep streaming.

## Tilesets

- R-REG-17 KEPT `registry_source.rs` `fetch_hexon` dispatches on the manifest's `hexon_type`. Plugins go through `verify_fetched`. Content hexons go through the seal, a trust root (or the dev flag), id, version and listed `content_hash`. `fetch` refuses a tileset as `not-a-plugin`; the journey now pins both behaviours.
- R-REG-18 FIXED `registry_source.rs` `verify_fetched_hexon` read `manifest.json` with zip 0.6 (fe-hexon) while `verify_archive` verified with zip 2 (fe-format), so the id, version and hash checks could read a manifest other than the sealed one. It now reads through `fe_format::HexonArchive::read_member` (zip 2). Both versions resolve a duplicate name to the last entry (zip 0.6.6 `names_map`, zip 2.4.2 `IndexMap`), so no exploit is known.
- R-REG-19 HANDED-OFF I-BIN2 (`fractalengine/src/terrain_bridge.rs`): its task says InstallFromRegistry via `remote_source(..).fetch()`. `fetch` verifies as a plugin and refuses every tileset, and `fetch_hexon` is whole-archive and at most 64 MiB, so the chunked `FetchStrategy` path stays. That path verifies `RequireSealed { pin: None }`: any signer is accepted, because "the operator-chosen registry is the trust anchor". R32's "one trust model" (trust roots for tilesets) would break the Hexon Manager journeys, so it is a user decision (user question).
- R-REG-20 KEPT `fe-ui/src/dialogs/hexon_manager.rs`: the free-text uri is percent-encoded as a single path segment by the bridge client (`hexon_url`), and versions with `+build` stay installable; no edit.

## content_hash bytes, client and server

- R-REG-21 KEPT: every side hashes the raw manifest with `fe_format::integrity::content_hash`, which is blake3 of the canonical JSON with `"signature": ""` and equals package-format.md `signing_bytes`. The sides are: server `index_one`, remote `attestation`, `verify_fetched_hexon`, `verified_header`, `verify_plugin_package` and fe-plugin `manifest_hash`. `/meta`'s `manifest` is the member's raw text (`header_doc`).
- R-REG-22 FIXED protocol skew. fe-plugin `RegistryClient::download` required the protocol header on the download response, but fe-hexon-registry sent it only on `/health`, so every download failed with `ProtocolMismatch`.
  - The server (`routes.rs`) now stamps the header on every response.
  - The client (`registry_client.rs`) falls back to `config.json` `registry_protocol` and refuses only a header that is present and mismatched; the contract never requires the header.

## Server publish

- R-REG-23 FIXED `fe-hexon-registry/src/routes.rs` `publish`. Ids were only charset-checked, so a leading `.` or `-` and a `.json` suffix were accepted. Owner continuity was case-sensitive, so a second key could publish `Night-Sky` beside `night-sky`. Publish now enforces the contract id grammar and treats ids as one case-insensitive namespace (409); table rows added.
- R-REG-24 KEPT `routes.rs`: versions with `+build` still publish, for tileset compatibility. Protocol-v1 clients skip them in listings (`valid_version`).
- R-REG-25 KEPT `index.rs`: `content_hash` is empty when a manifest cannot be canonicalised, so a client refuses it with `ContentHashMismatch` (fail closed).

## G0-C25 hand-off (fe-format manifest/integrity, fe-hexon package/publisher)

- R-REG-26 KEPT G0-C1: `publisher.rs` writes `entries.json` = `{"entries":[]}` (`PLUGIN_ENTRIES_JSON`), and `verify_plugin_package` requires exactly that document.
- R-REG-27 FIXED G0-C15 `fe-hexon/src/package.rs`: the `fractalengine:plugin@2.` prefix admitted any minor. The minor must now be at most `HOST_CONTRACT_MINOR` (0; the WIT is `2.0.0`); J5 row added.
- R-REG-28 HANDED-OFF B3 / R-HOOKS (`fe-sdk/src/manifest_v2.rs`), the validator side of G0-C25:
  - C15: `check_contract` tests the major only.
  - C2: `check_endpoints` accepts all six schemes for any transport, so a `websocket-dial` with `https://` passes.
  - C3 (fields a slot does not render) and C4 (slot args the command rejects): not found. V13 in the code is scope conflicts.
- R-REG-29 KEPT fe-format `integrity.rs` and `manifest.rs`. `verify_archive` covers the signature, header digests, every member, asset self-certification and the chunk table, and `HexonType::Plugin` exists.

## Smaller items

- R-REG-30 FIXED `registry_source.rs` `StaticIndexSource::header`: a 404 attestation from the http fetcher surfaced as `NotFound` ("not offered") instead of `HeaderUnavailable`.
- R-REG-31 KEPT `verified_header` does not require a trust-root signer. No binary path calls `header()`: ops_drain stages from `fetch`, which checks trust, and consent is pinned to that verified build. An attestation also carries no counter-signatures, so the check would falsely refuse counter-signed packages.
- R-REG-32 FIXED `sources.rs` `is_internal` now also refuses:
  - IPv4: 0.0.0.0/8, 240.0.0.0/4, multicast, 192.0.0.0/24 and 198.18.0.0/15;
  - IPv6: fec0::/10 and multicast;
  - IPv4 embedded in NAT64 (`64:ff9b::/96`) or IPv4-compatible addresses.

  Internal targets stay allowed when the base itself is internal (loopback fixtures; the steer's "loopback ok").

## Docs owed (files R-REG does not own)

- `docs/extensions/extensions-ui.md` §Registry sources needs these updates:
  - the protocol-v1 row: `config.json` fallback, plugin-only listing, anonymous by construction;
  - the rollback guard: in-process floor plus default cache;
  - the location rule: http(s) or relative only;
  - `verify_strict`.
- `fe-hexon/src/AGENTS.md` §registry-source-remote is stale. It says the registry advertises no protocol, that search lacks `content_hash`, and that the Hexon Manager cannot use the trait.
- `fe-hexon-registry/AGENTS.md` §routes needs: the protocol header on every response, the publish id grammar, and the case-insensitive id namespace.
- `docs/extensions/package-format.md` §Registry wire: the header is optional; `config.json` is the authority.
