---
type: review
gate: R-STORE
track: extension_platform_20261107
turn: 3
reviewed: [S-STORE, S-ENT, S-ENT-GATE, S-PSTORE, S-INDEX, S-CONTRACT, I-HEXON, S-HOST]
security_review: mandatory (fresh context, prompted to refute)
---

# R-STORE: store-client trust chain, entitlements, reference registry

Verdict: PASS-WITH-FIXES. Two mandatory security steers (2026-11-13) fixed; one compile break fixed; no money or leak terms in
owned files. Nothing was built or run (code-first directive); the journeys below are extended, not executed.

| # | Finding | Verdict |
|---|---|---|
| R-STORE-01 | Key-unwrap oracle: the wrap AAD/KDF bound only the two public keys, so `LocalIssuer::mint` re-wrapped any issuer wrap (package A) to the caller's device in a request for package B. | FIXED: `WrapBinding::{Archive{package_id}, Device{package_id, content_hash}}` in KDF input and AAD (`fe-format/src/entitlement.rs`); `mint` unwraps under the requested package; format byte 2; journey adds a lifted cross-package mint that fails and a device wrap re-labelled to another build that fails. |
| R-STORE-02 | Paid entries were bound to the entry id only, so a sealed entry could be moved between packages sharing a content key. | FIXED: `seal_paid_entry`/`open_paid_entry` take `package_id` (length-prefixed in the AAD); the gate opens with `claims.package_id`; journey checks entry-id and package moves both fail. |
| R-STORE-03 | Steer asked for `content_hash` in the archive wrap and the paid-entry AAD. | KEPT (contract change): both would hash themselves, since the manifest `content_hash` commits `license.json` and every sealed member. The device wrap carries the build; the archive wrap and entries carry the package. Documented in fe-format AGENTS §entitlement and hexon-format-spec. |
| R-STORE-04 | Issuer must take `encrypted_key` only from its own catalogue record for the requested package. | FIXED (docs + reference): contract-v1 §4.10, `MintRequest::encrypted_key` doc; the fake endpoint answers `unknown_package` for a package it holds no record for. |
| R-STORE-05 | Reference registry `/blobs/{hash}.hexon` served any stored archive by hash, unauthenticated, including unlisted packages. | FIXED: anonymous only when `index.json` lists the package and its detail names that `content_hash`; else bearer token or `FE_REGISTRY_PUBLIC_BLOBS=1` (`RegistryState::with_public_blobs`, default off); unknown/unlisted/unauthorised hashes give the same `404 not_found`. Journey extended (unlisted = unknown 404, token 200, listed anonymous 200). |
| R-STORE-06 | `tools/fe-ext/src/index.rs` `SourceTrust` literal lacked the new `is_killed` field (compile break). | FIXED: `is_killed: no_kill_list()` (publisher tooling; the host applies the kill list at install). |
| R-STORE-07 | `fe-hexon/tests/remote_registry_test.rs:186,225` `SourceTrust` literals lack `is_killed` (compile break of that test target). | HANDED-OFF (owner of the file / R-Z): add `is_killed: fe_hexon::registry_source::no_kill_list(),` to both literals. |
| R-STORE-08 | `fractalengine/src/extension_host/governance.rs:1835` (feature `entitlement` journey) calls `encrypted_key_for(&content_key)`. | HANDED-OFF (governance.rs owner / R-Z): `by.encrypted_key_for(&content_key, &licensed.package_id)`. |
| R-STORE-09 | Private registry entitlement custody/service/routes call 2-argument `wrap_content_key`/`unwrap_content_key`. | HANDED-OFF (private M-ENT; R-MENT-3 trigger has now fired): pass `&WrapBinding::Archive { package_id }` in `check_archive_key`, tests, and route fixtures. |
| R-STORE-10 | Index trust: pinned per endpoint, default lookup returns none, a trust-root key as index signer refused (`IndexKeyMisused`), `verify_strict`, integers-only canonical bytes. | KEPT (verified correct, `registry_source.rs:776-861`; `sources.rs:191-198` refuses trust-root pins at config). |
| R-STORE-11 | detail_hash pin, publisher/kind/latest-hash cross-checks, attestation recompute + publisher pin + trust root, download seal + hash + publisher + kind. | KEPT (verified, `registry_source.rs:1206-1258, 1321-1397, 1791-1817`). |
| R-STORE-12 | Rollback: live index older than the persisted/cached/in-process floor is ignored; equal `issued_at` with different content is accepted. | KEPT: same-instant re-signing needs the index key, which can already reorder listings; packages and kill list carry security (contract 5.5). |
| R-STORE-13 | Kill list at offer, detail, attest, fetch (all revocation keys, web bundle hash) and protocol-v1 pre-download. | KEPT (verified); plugin_bridge injects the governance predicate, terrain bridge `no_kill_list()` per steer. |
| R-STORE-14 | Anonymous fetch: static fetcher sends no credentials, no proxy, no redirects, pinned resolved addresses, internal targets only when the base is internal; v1 client uses same-origin redirects and no proxy. | KEPT (verified `sources.rs:262-341`, `remote.rs:30-55`). Plain-http remote bases stay allowed: contract rule 1.6 permits http, integrity comes from pins. |
| R-STORE-15 | Token forgery/replay: header alg EdDSA only, kid must be pinned, claim kid = header kid, lifetime cap, leeway, package/hash/licensee checks; a replayed token request yields a token wrapped to the requester's own device. | KEPT (verified `entitlement.rs` verify_claims, verify_token_request). |
| R-STORE-16 | `require_token` and `bearer_matches` compare the token with `==` (not constant time). | DEFERRED (trigger: the reference registry is exposed beyond dev/fixture use; a hosted wrapper authenticates itself). |
| R-STORE-17 | `fe-ext index verify` re-implements the web-bundle chain instead of `StaticIndexSource::fetch_listed` and checks only the latest version. | DEFERRED (trigger: next fe-ext index change; route web bundles through `search` + `fetch_listed`). |
| R-STORE-18 | Leak scan of every owned public file and of turn-3 added lines: clean, except `AZURE_TENANT_ID` (Azure code-signing secret name) added by commit 6d90b241 in `.github/workflows/{build-artifacts,release}.yml` and `docs/release-signing.md`. | HANDED-OFF (R-Z): third-party secret name, not commercial content; R-Z decides whether to rename the repository secret. |
| R-STORE-19 | `fe-plugin/src/AGENTS.md:331` describes `open_paid_entry(content_key, payload)`. | HANDED-OFF (I-PLUGIN doc owner): now `open_paid_entry(content_key, package_id, entry_id, payload)`. |
